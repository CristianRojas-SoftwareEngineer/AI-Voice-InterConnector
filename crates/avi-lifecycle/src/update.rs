// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! Traspaso de `self update` al binario nuevo y limpieza posterior: los pasos 9 y 10
//! de la actualización.
//!
//! El reemplazo lo ejecuta el binario nuevo: este módulo lanza
//! `<staging>/ai-voice-interconnector --json self install --yes`, captura stdout
//! estructurado y mantiene stderr separado. Valida la causa del resultado parcial
//! y propaga el resultado. Las preferencias del recibo mandan: si la
//! instalación registrada no tocaba el `PATH` (`modify_path: false`), el
//! traspaso pasa `--no-modify-path`; `--no-setup` se hereda si se pidió, y
//! `--force` se propaga cuando el `update` lo recibió.
//!
//! Garantías: un fallo antes del traspaso deja el staging borrado y la
//! instalación intacta (lo hace quien llama, con `fetch`, que ya borra el
//! staging ante cualquier fallo); un fallo durante el traspaso revierte la
//! transacción nueva (la del `self install` del binario nuevo, que es
//! transaccional); una interrupción queda recuperable en la
//! siguiente operación (`recovery.rs` barre el staging huérfano).
//!
//! La limpieza post-traspaso borra siempre el staging: lo que está en uso queda
//! a borrado diferido (Windows) o a la recuperación de la siguiente operación.
//! Nunca se toca nada fuera del staging que trajo la operación.

use crate::faults::{self, FaultPoint};
use crate::LifecycleError;
use avi_process::{spawn_with_allowlist_env, OwnerToken, RestrictedSpawnRequest, StdinSpec};
use std::path::{Path, PathBuf};

/// Variable interna con el PID del `self update` que lanza el traspaso.
///
/// El `self install` del traspaso la lee para esperar a ese proceso al
/// programar los restos del aparcado: es quien retiene los ficheros
/// aparcados, porque su imagen sigue cargada desde el directorio de programa.
/// Interna: no figura en ninguna ayuda ni contrato.
pub const HANDOVER_PARENT_PID_ENV: &str = "AVI_HANDOVER_PARENT_PID";

/// Código que el `self install` del traspaso devuelve cuando la provisión
/// falló: éxito parcial (`setup_failed`, código de salida 11).
const SETUP_FAILED_CODE: i32 = 11;

/// Código que el `self install` del traspaso devuelve cuando el reemplazo
/// falló con la versión instalada restaurada (`rolled_back`, código de salida 13).
const ROLLED_BACK_CODE: i32 = 13;

/// Entrada del traspaso: el ejecutable nuevo ya verificado en staging y las
/// banderas que se heredan o propagan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandoverRequest {
    /// Ejecutable nuevo en staging (`<staging>/…`, ya verificado con
    /// `--version` por `update_fetch::verify_boot`).
    pub staging_exe: PathBuf,
    /// `--no-setup` del `update`: no provisionar modelos tras el reemplazo.
    pub no_setup: bool,
    /// La instalación registrada no tocaba el `PATH`: el traspaso tampoco.
    pub no_modify_path: bool,
    /// `--force` del `update`: se propaga al `install` interno.
    pub force: bool,
}

/// Desenlace del traspaso.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HandoverOutcome {
    /// El binario nuevo instaló (o reparó) y su `setup` completó.
    Installed,
    /// El binario nuevo instaló pero su provisión falló.
    PartialSetupFailed { cause: ModelsCause },
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct ModelsCause {
    pub reason: String,
    pub message: String,
}

/// Argumentos del `self install` interno, tras el ejecutable. Función pura
/// para que la composición se pueda afirmar sin lanzar procesos.
pub fn handover_args(request: &HandoverRequest) -> Vec<String> {
    let mut args = vec![
        "--json".to_string(),
        "self".to_string(),
        "install".to_string(),
        "--yes".to_string(),
    ];
    if request.no_modify_path {
        args.push("--no-modify-path".to_string());
    }
    if request.no_setup {
        args.push("--no-setup".to_string());
    }
    if request.force {
        args.push("--force".to_string());
    }
    args
}

/// Interpreta el estado de salida del `self install` interno. Función pura
/// para que el mapeo se pueda afirmar sin lanzar procesos: 0 es éxito, 11 es
/// el éxito parcial de `setup_failed`, 13 es `rolled_back` con la versión
/// anterior restaurada, y cualquier otro estado es un fallo del traspaso.
pub fn interpret_handover_status(code: Option<i32>) -> anyhow::Result<HandoverOutcome> {
    match code {
        Some(0) => Ok(HandoverOutcome::Installed),
        Some(SETUP_FAILED_CODE) => Err(anyhow::anyhow!(
            "el traspaso terminó con setup_failed sin recibir el envelope del hijo"
        )),
        Some(ROLLED_BACK_CODE) => Err(LifecycleError::rolled_back(format!(
            "el traspaso al binario nuevo falló y restauró la versión anterior \
             (código {ROLLED_BACK_CODE}): reintenta `self update`"
        ))
        .into()),
        other => Err(anyhow::anyhow!(
            "el traspaso al binario nuevo terminó con estado {}: la instalación \
             anterior sigue en su sitio",
            other.map_or("desconocido".to_string(), |code| code.to_string()),
        )),
    }
}

/// Ejecuta el traspaso: lanza el `self install` del binario nuevo heredando
/// la consola, espera y propaga el resultado (éxito, `setup_failed` parcial
/// con el mecanismo de veredicto, `rolled_back`).
pub async fn handover(request: &HandoverRequest) -> anyhow::Result<HandoverOutcome> {
    faults::trip(FaultPoint::BeforeHandover)?;
    let args = handover_args(request);
    let staging_exe = request.staging_exe.clone();
    let (status, mut stdout_capture) = tokio::task::spawn_blocking(move || {
        // El hijo necesita saber a quién esperar al programar los restos del
        // aparcado: este proceso, cuya imagen retiene los ficheros aparcados.
        let stdout_capture = avi_process::scratch::tempfile("avi_handover_", ".json")
            .map_err(|e| anyhow::anyhow!("no se pudo crear la captura del traspaso: {e:#}"))?;
        let stdout_file = stdout_capture
            .reopen()
            .map_err(|e| anyhow::anyhow!("no se pudo abrir la captura del traspaso: {e}"))?;
        let stderr_file = inherited_stdio_file(StdioEnd::Stderr)
            .map_err(|e| anyhow::anyhow!("no se pudo preparar stderr del traspaso: {e}"))?;
        let parent_pid = std::ffi::OsString::from(std::process::id().to_string());
        let mut child = spawn_with_allowlist_env(
            RestrictedSpawnRequest {
                program: staging_exe.clone(),
                args,
                stdin: StdinSpec::Inherit,
                stdout_file,
                stderr_file,
                creation_flags: 0,
                extra_allowed: Vec::new(),
            },
            &[(HANDOVER_PARENT_PID_ENV, parent_pid.as_os_str())],
        )
        .map_err(|e| {
            anyhow::anyhow!(
                "no se pudo ejecutar el binario nuevo ({}): {e}",
                staging_exe.display()
            )
        })?;
        let status = child
            .wait_timeout(std::time::Duration::from_secs(60))
            .map_err(|e| anyhow::anyhow!("la espera del binario nuevo falló: {e}"))?
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "el binario nuevo no terminó en 60 s y se terminó: la instalación \
                     anterior sigue en su sitio"
                )
            })?;
        Ok::<_, anyhow::Error>((status, stdout_capture))
    })
    .await
    .map_err(|e| anyhow::anyhow!("el traspaso se interrumpió: {e}"))??;
    let outcome = match status.code() {
        Some(0) => HandoverOutcome::Installed,
        Some(SETUP_FAILED_CODE) => {
            use std::io::{Read, Seek, SeekFrom};
            stdout_capture.seek(SeekFrom::Start(0))?;
            let mut output = String::new();
            stdout_capture.read_to_string(&mut output)?;
            let envelope: serde_json::Value = serde_json::from_str(&output).map_err(|e| {
                anyhow::anyhow!("el hijo terminó setup_failed con stdout JSON inválido: {e}")
            })?;
            if envelope.get("reason").and_then(serde_json::Value::as_str) != Some("setup_failed") {
                anyhow::bail!("el hijo terminó con código 11 sin reason setup_failed válido");
            }
            let cause: ModelsCause =
                serde_json::from_value(envelope.get("models_cause").cloned().ok_or_else(|| {
                    anyhow::anyhow!("el hijo terminó setup_failed sin models_cause")
                })?)
                .map_err(|e| anyhow::anyhow!("models_cause del hijo es inválido: {e}"))?;
            if cause.reason.is_empty() || cause.message.is_empty() {
                anyhow::bail!("models_cause del hijo no contiene reason y message válidos");
            }
            HandoverOutcome::PartialSetupFailed { cause }
        }
        code => interpret_handover_status(code)?,
    };
    faults::trip(FaultPoint::DuringHandover)?;
    Ok(outcome)
}

/// Extremo de salida del padre cuyo destino hereda el traspaso.
#[derive(Debug, Clone, Copy)]
enum StdioEnd {
    Stderr,
}

impl StdioEnd {
    fn name(self) -> &'static str {
        match self {
            StdioEnd::Stderr => "stderr",
        }
    }
}

/// Fichero que envuelve el extremo indicado de la consola del invocador:
/// duplicado propio de ese extremo del padre, de modo que el `self install`
/// del traspaso escribe en el mismo destino y su handle queda en la
/// allowlist. Cada extremo del hijo hereda el suyo, sin fusionarlos.
///
/// En Windows el duplicado marca el handle como heredable y se cierra solo
/// al soltarse el `File`; en Unix es `dup(1)` para stdout y `dup(2)` para
/// stderr.
fn inherited_stdio_file(end: StdioEnd) -> anyhow::Result<std::fs::File> {
    #[cfg(windows)]
    {
        use std::os::windows::io::{AsRawHandle, FromRawHandle};
        use windows_sys::Win32::Foundation::{DuplicateHandle, DUPLICATE_SAME_ACCESS};
        use windows_sys::Win32::System::Threading::GetCurrentProcess;
        let source = std::io::stderr().as_raw_handle() as isize;
        let mut target: isize = 0;
        // SAFETY: duplica el handle del extremo del padre; el duplicado es
        // propio y su vida queda acotada por el `File` que lo envuelve.
        unsafe {
            if DuplicateHandle(
                GetCurrentProcess(),
                source,
                GetCurrentProcess(),
                &mut target,
                0,
                1,
                DUPLICATE_SAME_ACCESS,
            ) == 0
            {
                return Err(anyhow::anyhow!(
                    "no se pudo duplicar el {} del traspaso: {}",
                    end.name(),
                    std::io::Error::last_os_error()
                ));
            }
        }
        Ok(unsafe { std::fs::File::from_raw_handle(target as _) })
    }
    #[cfg(unix)]
    {
        use std::os::unix::io::FromRawFd;
        let source_fd = 2;
        let fd = unsafe { libc::dup(source_fd) };
        if fd < 0 {
            return Err(anyhow::anyhow!(
                "no se pudo duplicar el {} del traspaso: {}",
                end.name(),
                std::io::Error::last_os_error()
            ));
        }
        Ok(unsafe { std::fs::File::from_raw_fd(fd) })
    }
}

/// Prosa del éxito parcial del traspaso: el programa
/// queda actualizado y basta reintentar con `setup`.
///
/// Es la contrapartida de `install::setup_failed_message` para el camino del
/// `update`: allí la causa viaja anidada (`models_cause`); aquí el traspaso
/// valida y conserva la misma causa estructurada del proceso hijo.
pub fn partial_setup_message() -> String {
    "la provisión de modelos no se completó durante el traspaso. El programa \
     queda actualizado y basta reintentar con setup"
        .to_string()
}

/// Borrado de una ruta del ciclo cuando el ejecutable en uso puede estar
/// dentro.
///
/// El **mecanismo** es de plataforma y lo implementa el binario —en Unix es
/// `remove_dir_all`, y en Windows el limpiador propio desacoplado—, porque el
/// motor no lo puede implementar sin arrastrar `avi-daemon`. Es la misma
/// división que `uninstall::ProgramDirRemover`, pero sobre rutas arbitrarias
/// del ciclo (staging, `.old-*`) en vez de solo el directorio de programa.
pub trait PathRemover {
    /// Borra la ruta ya. Una ruta ausente es éxito: el plan dice lo que tiene
    /// que dejar de existir, y si ya no existe el objetivo está cumplido.
    fn remove_now(&self, path: &Path) -> anyhow::Result<()>;
    /// Programa el borrado para cuando termine el proceso `wait`, identificado por
    /// PID e inicio. `Ok` significa que el limpiador está en marcha. Es el caso
    /// diferido de Windows.
    fn schedule(&self, path: &Path, wait: &OwnerToken) -> anyhow::Result<()>;
}

/// Cómo quedó el staging tras la limpieza post-traspaso.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StagingCleanup {
    /// Borrado ya, de forma síncrona.
    Removed,
    /// Programado para cuando termine el proceso en curso (Windows).
    Scheduled,
    /// No se pudo borrar ni programar: lo barre la recuperación de la
    /// siguiente operación. No es un fallo del `update`.
    Kept,
}

/// Limpieza post-traspaso: borra siempre el staging; lo que está en uso queda
/// a diferido (Windows) o a la recuperación de la siguiente operación.
///
/// Nunca falla el `update`: un staging que sobrevive es litter con prefijo
/// propio de la tabla de rutas, que es exactamente lo que el barrido recoge.
pub fn cleanup_staging(staging: &Path, remover: &dyn PathRemover) -> StagingCleanup {
    if remover.remove_now(staging).is_ok() {
        return StagingCleanup::Removed;
    }
    let scheduled =
        avi_process::current_owner_token().and_then(|own| remover.schedule(staging, &own));
    match scheduled {
        Ok(()) => {
            eprintln!(
                "  el staging se borrará al terminar este proceso: {}",
                staging.display()
            );
            StagingCleanup::Scheduled
        }
        Err(_) => {
            eprintln!(
                "  no se pudo borrar {}: lo recogerá la recuperación de la \
                 siguiente operación",
                staging.display()
            );
            StagingCleanup::Kept
        }
    }
}

/// Resultado de programar los restos del aparcado con el limpiador.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScheduledLeftovers {
    /// Rutas programadas con el limpiador, en el orden recibido.
    pub scheduled: Vec<PathBuf>,
}

/// Programa los restos del aparcado que sobrevivieron al commit con el
/// limpiador propio, en vez de descartarlos.
///
/// `wait` es el proceso cuya muerte libera los restos (el viejo en el
/// traspaso, el propio en el staging). Lo que el limpiador no borre queda para
/// el barrido de la siguiente operación y su red (`doctor --repair`).
pub fn schedule_leftovers(
    leftovers: &[PathBuf],
    remover: &dyn PathRemover,
    wait: &OwnerToken,
) -> ScheduledLeftovers {
    // Programar no falla la operación: lo que el limpiador no acepte queda
    // para el barrido de la siguiente operación, como el staging `Kept`.
    let mut scheduled = Vec::with_capacity(leftovers.len());
    for leftover in leftovers {
        if remover.schedule(leftover, wait).is_ok() {
            scheduled.push(leftover.clone());
        }
    }
    ScheduledLeftovers { scheduled }
}

#[cfg(test)]
mod tests {
    use super::*;
    use avi_core::exit_codes::ExitCode;

    /// Los argumentos del `install` interno llevan `--yes` siempre y heredan
    /// `--no-modify-path`, `--no-setup` y `--force` solo cuando tocan.
    #[test]
    fn update_handover_args_compose_flags() {
        let base = HandoverRequest {
            staging_exe: PathBuf::from("/staging/ai-voice-interconnector"),
            no_setup: false,
            no_modify_path: false,
            force: false,
        };
        assert_eq!(
            handover_args(&base),
            vec!["--json", "self", "install", "--yes"]
        );
        let full = HandoverRequest {
            no_setup: true,
            no_modify_path: true,
            force: true,
            ..base.clone()
        };
        assert_eq!(
            handover_args(&full),
            vec![
                "--json",
                "self",
                "install",
                "--yes",
                "--no-modify-path",
                "--no-setup",
                "--force"
            ]
        );
        let partial = HandoverRequest {
            force: true,
            ..base
        };
        assert_eq!(
            handover_args(&partial),
            vec!["--json", "self", "install", "--yes", "--force"]
        );
    }

    /// El mapeo de estados de salida: 0 éxito, 11 parcial, 13 `rolled_back`
    /// (código 13 de la tabla), y el resto fallo del traspaso.
    #[test]
    fn update_handover_status_mapping() {
        assert_eq!(
            interpret_handover_status(Some(0)).unwrap(),
            HandoverOutcome::Installed
        );
        assert!(interpret_handover_status(Some(11)).is_err());
        let rolled = interpret_handover_status(Some(13)).unwrap_err();
        let failure = rolled
            .downcast_ref::<LifecycleError>()
            .expect("`rolled_back` viaja como `LifecycleError`");
        assert_eq!(failure.reason, "rolled_back");
        assert_eq!(ExitCode::from_reason(failure.reason).code(), 13);
        for code in [Some(1), Some(2), Some(101), None] {
            assert!(
                interpret_handover_status(code).is_err(),
                "el estado {code:?} es un fallo del traspaso"
            );
        }
    }

    /// Un ejecutable inexistente no se puede lanzar: el traspaso falla sin
    /// tocar nada. Hermético: no necesita red ni instalación.
    #[tokio::test]
    async fn update_handover_missing_exe_fails() {
        let request = HandoverRequest {
            staging_exe: PathBuf::from("/nonexistent/path/ai-voice-interconnector"),
            no_setup: false,
            no_modify_path: false,
            force: false,
        };
        assert!(handover(&request).await.is_err());
    }

    /// Doble para `PathRemover`: borrado programable y diferido programable.
    struct FakeRemover {
        remove_ok: bool,
        schedule: anyhow::Result<()>,
    }

    impl PathRemover for FakeRemover {
        fn remove_now(&self, _path: &Path) -> anyhow::Result<()> {
            if self.remove_ok {
                Ok(())
            } else {
                Err(anyhow::anyhow!("borrado bloqueado"))
            }
        }

        fn schedule(&self, _path: &Path, _wait: &OwnerToken) -> anyhow::Result<()> {
            match &self.schedule {
                Ok(()) => Ok(()),
                Err(_) => Err(anyhow::anyhow!("diferido no disponible")),
            }
        }
    }

    /// La limpieza borra el staging cuando puede, lo difiere cuando el
    /// borrado directo falla y el diferido procede, y lo deja a recuperación
    /// en otro caso, sin fallar nunca el `update`.
    #[test]
    fn update_staging_cleanup_never_fails_update() {
        let staging = Path::new("/staging/update-0.24.0");
        assert_eq!(
            cleanup_staging(
                staging,
                &FakeRemover {
                    remove_ok: true,
                    schedule: Ok(()),
                }
            ),
            StagingCleanup::Removed
        );
        assert_eq!(
            cleanup_staging(
                staging,
                &FakeRemover {
                    remove_ok: false,
                    schedule: Ok(()),
                }
            ),
            StagingCleanup::Scheduled
        );
        assert_eq!(
            cleanup_staging(
                staging,
                &FakeRemover {
                    remove_ok: false,
                    schedule: Err(anyhow::anyhow!("x")),
                }
            ),
            StagingCleanup::Kept
        );
    }

    /// Los restos del aparcado se programan con el limpiador en vez de
    /// descartarse: lo programado es exactamente la lista que la
    /// transacción no pudo borrar.
    #[test]
    fn transaction_leftovers_are_scheduled_not_discarded() {
        let leftovers = vec![std::path::PathBuf::from("/program/.old-tx/old.exe")];
        let scheduled = super::schedule_leftovers(
            &leftovers,
            &FakeRemover {
                remove_ok: false,
                schedule: Ok(()),
            },
            &OwnerToken::new(1234, "abc123").expect("token válido"),
        );
        assert_eq!(
            scheduled.scheduled, leftovers,
            "los restos deben programarse, no descartarse"
        );
    }

    /// Los puntos del traspaso tienen nombre estable para las pruebas de
    /// interrupción.
    #[test]
    fn update_fault_points_have_stable_names() {
        assert_eq!(FaultPoint::BeforeHandover.as_str(), "before_handover");
        assert_eq!(FaultPoint::DuringHandover.as_str(), "during_handover");
    }
}
