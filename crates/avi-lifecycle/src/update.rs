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
use crate::progress::PhaseProgress;
use crate::removal::{retire, PathRemover, Retirement};
use crate::{setup, LifecycleError};
use avi_process::{spawn_with_allowlist_env, RestrictedSpawnRequest, Scheduling, StdinSpec};
use std::path::{Path, PathBuf};

/// Variable interna con el PID del `self update` que lanza el traspaso.
///
/// El `self install` del traspaso la lee para esperar a ese proceso al retirar
/// el aparcado: es quien lo retiene, porque su imagen sigue cargada desde el
/// ejecutable aparcado.
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

/// Prosa del éxito parcial del traspaso: el programa queda actualizado, con el
/// consejo que [`setup::retry_advice`] decide según la causa de la provisión.
///
/// Es la contrapartida de `install::setup_failed_message` para el camino del
/// `update`: allí la causa viaja anidada (`models_cause`); aquí el traspaso
/// valida y conserva la misma causa estructurada del proceso hijo.
pub fn partial_setup_message(cause: &ModelsCause) -> String {
    format!(
        "la provisión de modelos no se completó durante el traspaso ({}: {}). El programa \
         queda actualizado y {}",
        cause.reason,
        cause.message,
        setup::retry_advice(&cause.reason)
    )
}

/// Limpieza post-traspaso: retira el staging con [`retire`]; lo que está en uso
/// por este proceso queda a diferido (Windows) o a la recuperación de la
/// siguiente operación.
///
/// Nunca falla el `update`: un staging que sobrevive es litter con prefijo
/// propio de la tabla de rutas, que es exactamente lo que el barrido recoge.
///
/// Cede la línea de fase de `progress` antes de escribir por stderr.
pub fn cleanup_staging(
    staging: &Path,
    remover: &dyn PathRemover,
    progress: &mut PhaseProgress,
) -> Retirement {
    let retirement = avi_process::current_owner_token()
        .map_or(Retirement::Kept, |own| retire(staging, remover, &own));
    match retirement {
        Retirement::Removed => {}
        Retirement::Scheduled(scheduling) => {
            progress.close_line();
            eprintln!("  {}", scheduled_staging_message(staging, scheduling));
        }
        Retirement::Kept => {
            progress.close_line();
            eprintln!(
                "  no se pudo borrar {}: lo recogerá la recuperación de la \
                 siguiente operación",
                staging.display()
            );
        }
    }
    retirement
}

/// Mensaje del borrado programado del staging. Si el limpiador quedó dependiente
/// del proceso anfitrión, añade que la recuperación de la siguiente operación
/// recoge el resto.
pub fn scheduled_staging_message(staging: &Path, scheduling: Scheduling) -> String {
    let base = format!(
        "el staging se borrará al terminar este proceso: {}",
        staging.display()
    );
    match scheduling {
        Scheduling::Detached => base,
        Scheduling::Attached => format!(
            "{base}. Si el proceso que ejecuta el comando se cierra antes, lo recoge la recuperación de la siguiente operación."
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use avi_core::exit_codes::ExitCode;
    use avi_process::OwnerToken;

    /// Un modelo rechazado por el Hub no se arregla reintentando: el mensaje no lo
    /// promete.
    #[test]
    fn partial_setup_message_without_retry_for_rejected_model() {
        let message = partial_setup_message(&ModelsCause {
            reason: "model_unavailable".to_string(),
            message: "modelo: 404".to_string(),
        });
        assert!(
            message.contains("model_unavailable: modelo: 404"),
            "{message}"
        );
        assert!(!message.contains("basta reintentar"), "{message}");
        assert!(message.contains("reintentar no lo resuelve"), "{message}");
    }

    /// Un fallo de transporte sí puede resolverse reintentando con `setup`.
    #[test]
    fn partial_setup_message_advises_retry_for_transport_failure() {
        let message = partial_setup_message(&ModelsCause {
            reason: "network_error".to_string(),
            message: "modelo: sin red".to_string(),
        });
        assert!(
            message.ends_with("y basta reintentar con setup"),
            "{message}"
        );
    }

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

        fn schedule(&self, _path: &Path, _wait: &OwnerToken) -> anyhow::Result<Scheduling> {
            match &self.schedule {
                Ok(()) => Ok(Scheduling::Detached),
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
                },
                &mut PhaseProgress::new()
            ),
            Retirement::Removed
        );
        assert_eq!(
            cleanup_staging(
                staging,
                &FakeRemover {
                    remove_ok: false,
                    schedule: Ok(()),
                },
                &mut PhaseProgress::new()
            ),
            Retirement::Scheduled(Scheduling::Detached)
        );
        assert_eq!(
            cleanup_staging(
                staging,
                &FakeRemover {
                    remove_ok: false,
                    schedule: Err(anyhow::anyhow!("x")),
                },
                &mut PhaseProgress::new()
            ),
            Retirement::Kept
        );
    }

    /// El mensaje del staging programado añade la recuperación solo si el
    /// limpiador quedó dependiente del proceso anfitrión.
    #[test]
    fn scheduled_staging_message_depends_on_scheduling() {
        let staging = Path::new("/staging/update-0.24.0");
        let detached = scheduled_staging_message(staging, Scheduling::Detached);
        assert!(detached.starts_with("el staging se borrará al terminar este proceso: "));
        assert!(!detached.contains("recuperación"));
        let attached = scheduled_staging_message(staging, Scheduling::Attached);
        assert!(attached.starts_with(&detached));
        assert!(attached.contains("recuperación de la siguiente operación"));
    }

    /// Los puntos del traspaso tienen nombre estable para las pruebas de
    /// interrupción.
    #[test]
    fn update_fault_points_have_stable_names() {
        assert_eq!(FaultPoint::BeforeHandover.as_str(), "before_handover");
        assert_eq!(FaultPoint::DuringHandover.as_str(), "during_handover");
    }
}
