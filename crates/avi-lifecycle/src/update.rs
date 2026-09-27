//! Traspaso de `self update` al binario nuevo y limpieza posterior: los pasos 9 y 10
//! de la actualización.
//!
//! El reemplazo lo ejecuta el binario nuevo (P3): este módulo lanza
//! `<staging>/ai-voice-interconnector self install --yes` heredando la consola,
//! espera y propaga el resultado. Las preferencias del recibo mandan: si la
//! instalación registrada no tocaba el `PATH` (`modify_path: false`), el
//! traspaso pasa `--no-modify-path`; `--no-setup` se hereda si se pidió, y
//! `--force` se propaga cuando el `update` lo recibió (decisión 5 del Ciclo 2).
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
use std::path::{Path, PathBuf};

/// Código que el `self install` del traspaso devuelve cuando la provisión
/// falló: éxito parcial (`setup_failed`, el 11 de la tabla cerrada).
const SETUP_FAILED_CODE: i32 = 11;

/// Código que el `self install` del traspaso devuelve cuando el reemplazo
/// falló con la versión anterior restaurada (`rolled_back`, el 13).
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
    /// `--force` del `update`: se propaga al `install` interno (decisión 5).
    pub force: bool,
}

/// Desenlace del traspaso.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandoverOutcome {
    /// El binario nuevo instaló (o reparó) y su `setup` completó.
    Installed,
    /// El binario nuevo instaló pero su provisión falló: éxito parcial con el
    /// mecanismo de veredicto del brazo `Install` (código 11).
    PartialSetupFailed,
}

/// Argumentos del `self install` interno, tras el ejecutable. Función pura
/// para que la composición se pueda afirmar sin lanzar procesos.
pub fn handover_args(request: &HandoverRequest) -> Vec<String> {
    let mut args = vec![
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
        Some(SETUP_FAILED_CODE) => Ok(HandoverOutcome::PartialSetupFailed),
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
    let status = tokio::process::Command::new(&request.staging_exe)
        .args(&args)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .await
        .map_err(|e| {
            anyhow::anyhow!(
                "no se pudo ejecutar el binario nuevo ({}): {e}",
                request.staging_exe.display()
            )
        })?;
    let outcome = interpret_handover_status(status.code())?;
    faults::trip(FaultPoint::DuringHandover)?;
    Ok(outcome)
}

/// Prosa del éxito parcial del traspaso: el programa
/// queda actualizado y basta reintentar con `setup`.
///
/// Es la contrapartida de `install::setup_failed_message` para el camino del
/// `update`: allí la causa viaja anidada (`models_cause`), aquí el traspaso
/// solo conoce el código 11 del hijo, así que el mensaje no nombra una causa
/// que no vio.
pub fn partial_setup_message() -> String {
    "la provisión de modelos no se completó durante el traspaso. El programa \
     queda actualizado y basta reintentar con setup"
        .to_string()
}

/// Borrado de una ruta del ciclo cuando el ejecutable en uso puede estar
/// dentro.
///
/// El **mecanismo** es de plataforma y lo implementa el binario —en Unix es
/// `remove_dir_all`, y en Windows un proceso auxiliar desacoplado—, porque el
/// motor no lo puede implementar sin arrastrar `avi-daemon`. Es la misma
/// división que `uninstall::ProgramDirRemover`, pero sobre rutas arbitrarias
/// del ciclo (staging, `.old-*`) en vez de solo el directorio de programa.
pub trait PathRemover {
    /// Borra la ruta ya. Una ruta ausente es éxito: el plan dice lo que tiene
    /// que dejar de existir, y si ya no existe el objetivo está cumplido.
    fn remove_now(&self, path: &Path) -> anyhow::Result<()>;
    /// Programa el borrado para cuando termine el proceso en curso y devuelve
    /// `true` si quedó programado. Es el caso diferido de Windows.
    fn schedule(&self, path: &Path, pid: u32) -> anyhow::Result<bool>;
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
    match remover.schedule(staging, std::process::id()) {
        Ok(true) => {
            eprintln!(
                "  el staging se borrará al terminar este proceso: {}",
                staging.display()
            );
            StagingCleanup::Scheduled
        }
        _ => {
            eprintln!(
                "  no se pudo borrar {}: lo recogerá la recuperación de la \
                 siguiente operación",
                staging.display()
            );
            StagingCleanup::Kept
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(handover_args(&base), vec!["self", "install", "--yes"]);
        let full = HandoverRequest {
            no_setup: true,
            no_modify_path: true,
            force: true,
            ..base.clone()
        };
        assert_eq!(
            handover_args(&full),
            vec![
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
            vec!["self", "install", "--yes", "--force"]
        );
    }

    /// El mapeo de estados de salida: 0 éxito, 11 parcial, 13 `rolled_back`
    /// con el 13 de la tabla, y el resto fallo del traspaso.
    #[test]
    fn update_handover_status_mapping() {
        assert_eq!(
            interpret_handover_status(Some(0)).unwrap(),
            HandoverOutcome::Installed
        );
        assert_eq!(
            interpret_handover_status(Some(11)).unwrap(),
            HandoverOutcome::PartialSetupFailed
        );
        let rolled = interpret_handover_status(Some(13)).unwrap_err();
        let failure = rolled
            .downcast_ref::<LifecycleError>()
            .expect("`rolled_back` viaja como `LifecycleError`");
        assert_eq!(failure.reason, "rolled_back");
        assert_eq!(failure.exit_code, 13);
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
            staging_exe: PathBuf::from("/ruta/inexistente/ai-voice-interconnector"),
            no_setup: false,
            no_modify_path: false,
            force: false,
        };
        assert!(handover(&request).await.is_err());
    }

    /// Doble para `PathRemover`: borrado programable y diferido programable.
    struct FakeRemover {
        remove_ok: bool,
        schedule: anyhow::Result<bool>,
    }

    impl PathRemover for FakeRemover {
        fn remove_now(&self, _path: &Path) -> anyhow::Result<()> {
            if self.remove_ok {
                Ok(())
            } else {
                Err(anyhow::anyhow!("borrado bloqueado"))
            }
        }

        fn schedule(&self, _path: &Path, _pid: u32) -> anyhow::Result<bool> {
            match &self.schedule {
                Ok(scheduled) => Ok(*scheduled),
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
                    schedule: Ok(false),
                }
            ),
            StagingCleanup::Removed
        );
        assert_eq!(
            cleanup_staging(
                staging,
                &FakeRemover {
                    remove_ok: false,
                    schedule: Ok(true),
                }
            ),
            StagingCleanup::Scheduled
        );
        for schedule in [Ok(false), Err(anyhow::anyhow!("x"))] {
            assert_eq!(
                cleanup_staging(
                    staging,
                    &FakeRemover {
                        remove_ok: false,
                        schedule,
                    }
                ),
                StagingCleanup::Kept
            );
        }
    }

    /// Los puntos del traspaso tienen nombre estable para las pruebas de
    /// interrupción.
    #[test]
    fn update_fault_points_have_stable_names() {
        assert_eq!(FaultPoint::BeforeHandover.as_str(), "before_handover");
        assert_eq!(FaultPoint::DuringHandover.as_str(), "during_handover");
    }
}
