//! Guardián del WAV temporal de `say`/`dub` y de los handlers del daemon.
//!
//! Quien crea el fichero lo borra al salir de ámbito, también en salidas de
//! error: el `Drop` ignora el fallo cuando el fichero ya no existe.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

/// Contador para unicidad entre hilos del mismo proceso con igual marca temporal.
static COUNTER: AtomicU64 = AtomicU64::new(0);

/// WAV temporal con dueño: lo borra al salir de ámbito.
///
/// El constructor reserva una ruta única con el prefijo dado dentro del
/// temporal del sistema y crea el fichero vacío; el `Drop` lo retira sin
/// fallar aunque ya se haya borrado fuera.
pub struct TempWav {
    path: PathBuf,
}

impl TempWav {
    /// Reserva un WAV temporal con el prefijo de producto dado (p. ej. `"avi_say_"`).
    pub fn new(prefix: &str) -> std::io::Result<Self> {
        loop {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let candidate = std::env::temp_dir().join(format!(
                "{prefix}{}_{}_{}.wav",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::SeqCst),
                nanos,
            ));
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&candidate)
            {
                Ok(_) => return Ok(Self { path: candidate }),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
    }

    /// Ruta del temporal, para sintetizar, escribir o reproducir.
    ///
    /// Es `&PathBuf` y no `&Path` porque el motor de síntesis la recibe como
    /// `Option<&PathBuf>`; devolver `&Path` obligaría a convertir en cada uno de
    /// los seis puntos de uso.
    pub fn path(&self) -> &PathBuf {
        &self.path
    }
}

impl Drop for TempWav {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::TempWav;

    /// Salida normal de ámbito: al soltarse el guardián el fichero desaparece.
    #[test]
    fn scope_exit_removes_the_file() {
        let path = {
            let guard = TempWav::new("avi_shared_test_").expect("reservar el temporal");
            assert!(guard.path().is_file(), "el constructor crea el fichero");
            guard.path().clone()
        };
        assert!(
            !path.exists(),
            "al salir de ámbito no debe quedar el temporal"
        );
    }

    /// Descarte explícito con `drop`: también borra el fichero.
    #[test]
    fn explicit_drop_removes_the_file() {
        let guard = TempWav::new("avi_shared_test_").expect("reservar el temporal");
        let path = guard.path().clone();
        assert!(path.is_file(), "el constructor crea el fichero");
        drop(guard);
        assert!(!path.exists(), "el descarte explícito debe borrarlo");
    }

    /// Borrado externo previo: soltarse después no falla y no recrea nada.
    #[test]
    fn drop_after_external_delete_does_not_fail() {
        let guard = TempWav::new("avi_shared_test_").expect("reservar el temporal");
        let path = guard.path().clone();
        std::fs::remove_file(&path).expect("borrado externo");
        drop(guard);
        assert!(
            !path.exists(),
            "tras el borrado externo no debe quedar nada"
        );
    }
}
