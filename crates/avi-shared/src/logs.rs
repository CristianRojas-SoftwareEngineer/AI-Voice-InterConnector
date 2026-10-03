//! Creación de logs por proceso, con retención por familia.
//!
//! Cada log se llama `<familia>_<pid>_<ms>.log`: `<pid>` es el del proceso que
//! lo crea y `<ms>` los milisegundos desde la época Unix. Al crear uno se poda
//! su familia a los `LOG_RETENTION` más recientes, de modo que el disco no crece
//! con cada arranque.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

/// Logs que se conservan por familia.
pub const LOG_RETENTION: usize = 10;

/// Crea `<dir>/<family>_<pid>_<ms>.log`, el directorio si falta, y poda la
/// familia. Recibe el directorio por parámetro para que las pruebas no toquen
/// los logs reales.
pub fn create_log(dir: &Path, family: &str) -> std::io::Result<(PathBuf, File)> {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    create_log_at(dir, family, millis)
}

/// Si el nombre ya existe (dos logs del mismo proceso en el mismo milisegundo) se
/// reintenta con el milisegundo siguiente, que conserva el orden por antigüedad.
fn create_log_at(dir: &Path, family: &str, millis: u128) -> std::io::Result<(PathBuf, File)> {
    std::fs::create_dir_all(dir)?;
    let mut millis = millis;
    let (path, file) = loop {
        let path = dir.join(format!("{family}_{}_{millis}.log", std::process::id()));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => break (path, file),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => millis += 1,
            Err(e) => return Err(e),
        }
    };
    prune(dir, family, &path);
    Ok((path, file))
}

/// Extrae `<ms>` de `<family>_<u32>_<u128>.log`; `None` si el nombre no sigue el patrón.
fn log_millis(name: &str, family: &str) -> Option<u128> {
    let rest = name.strip_prefix(family)?.strip_prefix('_')?;
    let rest = rest.strip_suffix(".log")?;
    let (pid, millis) = rest.split_once('_')?;
    pid.parse::<u32>().ok()?;
    millis.parse::<u128>().ok()
}

/// Borra los logs de la familia que excedan `LOG_RETENTION`, los más antiguos
/// primero, sin tocar nunca el log recién creado (`keep`), aunque el reloj haya
/// retrocedido. Un fallo de borrado (en Windows, un log abierto por otro proceso)
/// se ignora y la poda sigue con el resto.
fn prune(dir: &Path, family: &str, keep: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut logs: Vec<(u128, String)> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            log_millis(&name, family).map(|ms| (ms, name))
        })
        .filter(|(_, name)| dir.join(name) != keep)
        .collect();
    logs.sort();
    // El log recién creado cuenta como uno de los conservados.
    let excess = logs.len().saturating_sub(LOG_RETENTION - 1);
    for (_, name) in logs.into_iter().take(excess) {
        let _ = std::fs::remove_file(dir.join(name));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn unique_dir() -> PathBuf {
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "avi_logs_test_{}_{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    /// Doce creaciones dejan solo los diez logs más recientes de la familia.
    #[test]
    fn create_log_keeps_ten_most_recent_per_family() {
        let dir = unique_dir();
        for ms in 1..=12u128 {
            create_log_at(&dir, "daemon", ms).unwrap();
        }
        let pid = std::process::id();
        let present = names(&dir);
        assert_eq!(present.len(), LOG_RETENTION, "{present:?}");
        for ms in 1..=2u128 {
            assert!(!present.contains(&format!("daemon_{pid}_{ms}.log")));
        }
        for ms in 3..=12u128 {
            assert!(present.contains(&format!("daemon_{pid}_{ms}.log")));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// La poda no toca otras familias ni ficheros ajenos al patrón.
    #[test]
    fn create_log_leaves_other_families_and_foreign_files() {
        let dir = unique_dir();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("daemon.log"), "x").unwrap();
        std::fs::write(dir.join("notas.txt"), "x").unwrap();
        for ms in 1..=12u128 {
            create_log_at(&dir, "qwen3-tts", ms).unwrap();
        }
        for ms in 1..=12u128 {
            create_log_at(&dir, "daemon", 100 + ms).unwrap();
        }
        let present = names(&dir);
        assert!(present.contains(&"daemon.log".to_string()));
        assert!(present.contains(&"notas.txt".to_string()));
        let tts = present
            .iter()
            .filter(|n| n.starts_with("qwen3-tts_"))
            .count();
        let daemon = present.iter().filter(|n| n.starts_with("daemon_")).count();
        assert_eq!((tts, daemon), (LOG_RETENTION, LOG_RETENTION), "{present:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Dos logs de la misma familia en el mismo milisegundo no colisionan.
    #[test]
    fn create_log_retries_when_the_name_is_taken() {
        let dir = unique_dir();
        let (first, _a) = create_log_at(&dir, "daemon", 7).unwrap();
        let (second, _b) = create_log_at(&dir, "daemon", 7).unwrap();
        assert_ne!(first, second);
        assert!(second.to_string_lossy().ends_with("_8.log"), "{second:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn create_log_names_family_pid_and_millis() {
        let dir = unique_dir();
        let (path, _file) = create_log_at(&dir, "daemon", 42).unwrap();
        assert_eq!(
            path.file_name().unwrap().to_string_lossy(),
            format!("daemon_{}_42.log", std::process::id())
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
