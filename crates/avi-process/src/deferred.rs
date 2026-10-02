use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::{CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW};

/// Reintentos de borrado del auxiliar, con espera entre ellos. Acotados: un
/// archivo en uso se libera al morir el proceso, y lo que siga bloqueado
/// después lo recoge la recuperación de la siguiente operación.
const DEFERRED_REMOVAL_ATTEMPTS: u32 = 10;

/// Espera entre reintentos del auxiliar.
const DEFERRED_REMOVAL_RETRY_MS: u64 = 500;

/// Plazo para que el auxiliar demuestre que su script se ejecuta.
const READY_DEADLINE: Duration = Duration::from_secs(5);

/// Sondeo de la marca de arranque.
const READY_POLL: Duration = Duration::from_millis(50);

/// Borrado diferido de una ruta en Windows (staging, `.old-*`, directorio de
/// programa).
///
/// Escribe un `.ps1` en `%TEMP%` que espera la muerte del `pid`, borra `path`
/// con reintentos acotados y se borra a sí mismo. La primera instrucción del
/// script crea una marca `.ready`: `Ok` solo se devuelve cuando la marca
/// aparece, o cuando el auxiliar sigue vivo al vencer el plazo. La marca la
/// borra el propio auxiliar al terminar, junto con su script: quien la crea es
/// quien la borra. Si el auxiliar muere sin marca, devuelve `Err` y borra el
/// script y la marca. Ambos llevan el prefijo `avi-` que barre la recuperación.
pub fn spawn_deferred_removal(path: &Path, pid: u32) -> anyhow::Result<PathBuf> {
    let literal = path.to_string_lossy().replace('\'', "''");
    let attempts = DEFERRED_REMOVAL_ATTEMPTS;
    let retry_ms = DEFERRED_REMOVAL_RETRY_MS;

    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let temp = std::env::temp_dir();
    let helper = temp.join(format!("avi-deferred-{pid}-{stamp}.ps1"));
    let ready = temp.join(format!("avi-deferred-{pid}-{stamp}.ready"));
    let ready_literal = ready.to_string_lossy().replace('\'', "''");

    let script = format!(
        "Set-Content -LiteralPath '{ready_literal}' -Value ''; \
         Wait-Process -Id {pid} -ErrorAction SilentlyContinue; \
         Start-Sleep -Milliseconds {retry_ms}; \
         for ($i = 1; $i -le {attempts}; $i++) {{ \
           if (-not (Test-Path -LiteralPath '{literal}')) {{ break }}; \
           Remove-Item -LiteralPath '{literal}' -Recurse -Force -ErrorAction SilentlyContinue; \
           if (-not (Test-Path -LiteralPath '{literal}')) {{ break }}; \
           Start-Sleep -Milliseconds {retry_ms} \
         }}; \
         Remove-Item -LiteralPath '{ready_literal}', $PSCommandPath -Force -ErrorAction SilentlyContinue\n"
    );
    std::fs::write(&helper, script)?;

    let mut cmd = Command::new("powershell.exe");
    cmd.args([
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-File",
        helper.to_string_lossy().as_ref(),
    ]);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // Con `DETACHED_PROCESS`, `powershell.exe` termina en milisegundos sin
    // ejecutar el script. `CREATE_NO_WINDOW` le da una consola oculta propia; el
    // grupo propio lo aísla de la señal de cierre del padre. La herencia de
    // handles se corta en la raíz con `SetHandleInformation`, no con una flag.
    cmd.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            let _ = std::fs::remove_file(&helper);
            return Err(e.into());
        }
    };
    if let Err(e) = await_ready(&mut child, &ready, READY_DEADLINE) {
        let _ = std::fs::remove_file(&helper);
        let _ = std::fs::remove_file(&ready);
        return Err(e);
    }
    Ok(helper)
}

/// Espera la marca de arranque del auxiliar sondeando cada 50 ms.
///
/// Con la marca presente devuelve `Ok` sin tocarla: la borra el auxiliar al
/// terminar. Si el hijo termina antes de crearla, devuelve `Err` con su estado. Si vence el plazo con el hijo vivo,
/// devuelve `Ok`: sigue en marcha aunque aún no haya llegado a la marca.
pub(crate) fn await_ready(
    child: &mut Child,
    ready: &Path,
    deadline: Duration,
) -> anyhow::Result<()> {
    let start = Instant::now();
    loop {
        if ready.exists() {
            return Ok(());
        }
        if let Some(status) = child.try_wait()? {
            // La marca pudo aparecer justo antes de que el hijo terminara.
            if ready.exists() {
                return Ok(());
            }
            anyhow::bail!("el auxiliar de borrado terminó sin ejecutar su script ({status})");
        }
        if start.elapsed() >= deadline {
            return Ok(());
        }
        std::thread::sleep(READY_POLL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_ready(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("avi-test-{tag}-{}.ready", std::process::id()))
    }

    #[test]
    fn await_ready_fails_when_child_exits_without_marker() {
        let ready = unique_ready("nomarker");
        let _ = std::fs::remove_file(&ready);
        let mut child = Command::new("cmd")
            .args(["/C", "exit 0"])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .unwrap();
        let result = await_ready(&mut child, &ready, Duration::from_secs(5));
        assert!(result.is_err());
    }

    #[test]
    fn await_ready_succeeds_when_marker_appears() {
        let ready = unique_ready("marker");
        let _ = std::fs::remove_file(&ready);
        let mut child = Command::new("cmd")
            .raw_arg(format!("/C type nul > \"{}\"", ready.display()))
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .unwrap();
        let result = await_ready(&mut child, &ready, Duration::from_secs(5));
        assert!(result.is_ok());
        assert!(
            ready.exists(),
            "await_ready no borra la marca: es del auxiliar"
        );
        let _ = std::fs::remove_file(&ready);
    }
}
