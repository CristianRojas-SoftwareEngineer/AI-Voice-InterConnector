//! Creación de logs por proceso, con retención por familia.
//!
//! Cada log se llama `<familia>_<pid>_<ms>.log`: `<pid>` es el del proceso que
//! lo escribe y `<ms>` los milisegundos desde la época Unix. Al crear uno se poda
//! su familia a los `LOG_RETENTION` más recientes, de modo que el disco no crece
//! con cada arranque.
//!
//! La poda nunca borra el log de un proceso vivo ajeno: su PID se lee del nombre
//! y se contrasta con la viveza exacta del sistema. Sin esa guarda, con diez o
//! más arranques se podría borrar el log de un daemon en marcha, porque el nombre
//! solo ordena por antigüedad. La exención es solo para PID distintos del propio:
//! los logs antiguos del proceso que poda sí se retiran, que es justo lo que la
//! rotación necesita.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

/// Logs que se conservan por familia.
pub const LOG_RETENTION: usize = 10;

/// Crea `<dir>/<family>_<pid>_<ms>.log` con el PID del proceso propio, el
/// directorio si falta, y poda la familia. Recibe el directorio por parámetro
/// para que las pruebas no toquen los logs reales, y el sondeo de viveza de
/// PID que decide qué logs ajenos la poda conserva.
pub fn create_log(
    dir: &Path,
    family: &str,
    pid_alive: &dyn Fn(u32) -> bool,
) -> std::io::Result<(PathBuf, File)> {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    create_log_at_with(dir, family, std::process::id(), millis, pid_alive)
}

/// El cuerpo de la creación, con el sondeo de viveza de la poda inyectado: las
/// pruebas fijan qué PID están vivos sin lanzar procesos reales, y el resto de la
/// poda (patrón del nombre, `keep`, retención) es el mismo.
///
/// Si el nombre ya existe (dos logs del mismo proceso en el mismo milisegundo) se
/// reintenta con el milisegundo siguiente, que conserva el orden por antigüedad.
fn create_log_at_with(
    dir: &Path,
    family: &str,
    pid: u32,
    millis: u128,
    pid_alive: &dyn Fn(u32) -> bool,
) -> std::io::Result<(PathBuf, File)> {
    std::fs::create_dir_all(dir)?;
    let mut millis = millis;
    let (path, file) = loop {
        let path = dir.join(format!("{family}_{pid}_{millis}.log"));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => break (path, file),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => millis += 1,
            Err(e) => return Err(e),
        }
    };
    prune(dir, family, &path, pid_alive);
    Ok((path, file))
}

/// Renombra un log ya creado para que su nombre lleve el PID indicado.
///
/// Lo usa el lanzador tras el arranque: el log se creó con el PID del lanzador y
/// se renombra con el del hijo, de modo que el nombre identifica al proceso que
/// escribe. Devuelve la ruta nueva, o la original si el renombrado falla (por
/// ejemplo, si el sistema no admite renombrar un fichero abierto), sin fallar:
/// el nombre con el PID del lanzador sigue siendo válido para la poda.
pub fn rename_log_to_pid(path: &Path, family: &str, pid: u32) -> PathBuf {
    let Some(ms) = path
        .file_name()
        .and_then(|n| log_millis(&n.to_string_lossy(), family))
    else {
        return path.to_path_buf();
    };
    let dir = path.parent().unwrap_or(Path::new("."));
    let target = dir.join(format!("{family}_{pid}_{ms}.log"));
    match std::fs::rename(path, &target) {
        Ok(()) => target,
        Err(_) => path.to_path_buf(),
    }
}

/// Extrae `<pid>` y `<ms>` de `<family>_<u32>_<u128>.log`; `None` si el nombre
/// no sigue el patrón.
fn log_id(name: &str, family: &str) -> Option<(u32, u128)> {
    let rest = name.strip_prefix(family)?.strip_prefix('_')?;
    let rest = rest.strip_suffix(".log")?;
    let (pid, millis) = rest.split_once('_')?;
    Some((pid.parse::<u32>().ok()?, millis.parse::<u128>().ok()?))
}

/// Extrae `<ms>` de `<family>_<u32>_<u128>.log`; `None` si el nombre no sigue el patrón.
fn log_millis(name: &str, family: &str) -> Option<u128> {
    log_id(name, family).map(|(_, ms)| ms)
}

/// Borra los logs de la familia que excedan `LOG_RETENTION`, los más antiguos
/// primero, sin tocar nunca el log recién creado (`keep`), aunque el reloj haya
/// retrocedido. Un fallo de borrado (en Windows, un log abierto por otro proceso)
/// se ignora y la poda sigue con el resto.
///
/// `pid_alive` decide qué PID sigue en marcha. Un log cuyo PID distinto del
/// propio esté vivo se conserva siempre y no cuenta para la retención: borrarlo
/// dejaría al proceso vivo sin su traza justo cuando más la necesita. El PID
/// propio no se consulta (la exclusión vive aquí, no en el sondeo), para que los
/// logs antiguos del proceso que poda sigan siendo podables.
fn prune(dir: &Path, family: &str, keep: &Path, pid_alive: &dyn Fn(u32) -> bool) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let own_pid = std::process::id();
    let mut logs: Vec<(u128, String)> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            log_id(&name, family).map(|(pid, ms)| (pid, ms, name))
        })
        .filter(|(_, _, name)| dir.join(name) != keep)
        .filter(|(pid, _, _)| *pid == own_pid || !pid_alive(*pid))
        .map(|(_, ms, name)| (ms, name))
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

    fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    /// PID sembrado como vivo: no corresponde a ningún proceso real, la
    /// respuesta la fija el control inyectado.
    const LIVE_PID: u32 = 424_242;
    /// PID sembrado como muerto, tampoco real.
    const DEAD_PID: u32 = 424_243;

    /// Doce creaciones dejan solo los diez logs más recientes de la familia.
    #[test]
    fn create_log_keeps_ten_most_recent_per_family() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        for ms in 1..=12u128 {
            create_log_at_with(&dir, "daemon", std::process::id(), ms, &none_alive).unwrap();
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
    }

    /// Sondeo fijo: ningún PID ajeno está vivo.
    fn none_alive(_pid: u32) -> bool {
        false
    }

    /// La poda no toca otras familias ni ficheros ajenos al patrón.
    #[test]
    fn create_log_leaves_other_families_and_foreign_files() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("daemon.log"), "x").unwrap();
        std::fs::write(dir.join("notes.txt"), "x").unwrap();
        for ms in 1..=12u128 {
            create_log_at_with(&dir, "qwen3-tts", std::process::id(), ms, &none_alive).unwrap();
        }
        for ms in 1..=12u128 {
            create_log_at_with(&dir, "daemon", std::process::id(), 100 + ms, &none_alive).unwrap();
        }
        let present = names(&dir);
        assert!(present.contains(&"daemon.log".to_string()));
        assert!(present.contains(&"notes.txt".to_string()));
        let tts = present
            .iter()
            .filter(|n| n.starts_with("qwen3-tts_"))
            .count();
        let daemon = present.iter().filter(|n| n.starts_with("daemon_")).count();
        assert_eq!((tts, daemon), (LOG_RETENTION, LOG_RETENTION), "{present:?}");
    }

    /// Dos logs de la misma familia en el mismo milisegundo no colisionan.
    #[test]
    fn create_log_retries_when_the_name_is_taken() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        let pid = std::process::id();
        let (first, _a) = create_log_at_with(&dir, "daemon", pid, 7, &none_alive).unwrap();
        let (second, _b) = create_log_at_with(&dir, "daemon", pid, 7, &none_alive).unwrap();
        assert_ne!(first, second);
        assert!(second.to_string_lossy().ends_with("_8.log"), "{second:?}");
    }

    #[test]
    fn create_log_names_family_pid_and_millis() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        let (path, _file) =
            create_log_at_with(&dir, "daemon", std::process::id(), 42, &none_alive).unwrap();
        assert_eq!(
            path.file_name().unwrap().to_string_lossy(),
            format!("daemon_{}_42.log", std::process::id())
        );
    }

    /// La poda conserva el log de un PID sembrado vivo aunque sea el más
    /// antiguo, y poda el resto hasta la retención: es la razón de la guarda de
    /// viveza, que sin ella dejaría al proceso vivo sin traza al undécimo
    /// arranque.
    #[test]
    fn prune_skips_logs_of_live_pids() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        let alive = |pid: u32| pid == LIVE_PID;
        // El log del PID vivo es el más antiguo de la familia.
        let (live_path, _live_file) =
            create_log_at_with(&dir, "daemon", LIVE_PID, 1, &alive).unwrap();
        for ms in 2..=12u128 {
            create_log_at_with(&dir, "daemon", DEAD_PID, ms, &alive).unwrap();
        }
        let present = names(&dir);
        // El log del PID vivo sobrevive siendo el más antiguo: sin la guarda,
        // el undécimo arranque se lo llevaría por delante.
        assert!(present.contains(&log_name(LIVE_PID, 1)), "{present:?}");
        assert!(live_path.exists());
        // Los del PID muerto se podan según la retención de siempre.
        let dead: Vec<&String> = present
            .iter()
            .filter(|n| n.starts_with(&format!("daemon_{DEAD_PID}_")))
            .collect();
        // El vivo no consume plaza de retención: los once logs del PID muerto se
        // podan como si el vivo no existiera, y solo cae el más antiguo.
        assert_eq!(dead.len(), LOG_RETENTION, "{present:?}");
        assert!(!present.contains(&log_name(DEAD_PID, 2)), "{present:?}");
        assert!(present.contains(&log_name(DEAD_PID, 3)), "{present:?}");
    }

    /// El recién creado es `keep`: sobrevive a su propia poda aunque no sea el
    /// más reciente, y aunque su PID figura como vivo en el control (que es lo
    /// que pasa con el proceso que poda, siempre en marcha).
    #[test]
    fn prune_never_deletes_the_just_created_log() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        let alive = |pid: u32| pid == LIVE_PID || pid == DEAD_PID;
        // Doce logs del PID que el control declara vivo: sin `keep` y sin la
        // guarda de viveza, cada creación podaría el recién creado por ser el
        // más antiguo en el siguiente arranque.
        for ms in 1..=12u128 {
            let (keep, _keep_file) =
                create_log_at_with(&dir, "daemon", DEAD_PID, ms, &alive).unwrap();
            assert!(keep.exists(), "el log recién creado no debe podarse");
        }
        // Con la guarda de viveza todos sobreviven; el control los declara vivos.
        assert_eq!(names(&dir).len(), 12);
    }

    /// El PID propio nunca se consulta al sondeo: aunque este lo declare vivo
    /// (el proceso que poda siempre lo está), sus logs antiguos siguen siendo
    /// podables y la retención se cumple.
    #[test]
    fn prune_ignores_the_probe_for_the_own_pid() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        let all_alive = |_pid: u32| true;
        for ms in 1..=12u128 {
            create_log_at_with(&dir, "daemon", std::process::id(), ms, &all_alive).unwrap();
        }
        let present = names(&dir);
        assert_eq!(present.len(), LOG_RETENTION, "{present:?}");
        assert!(
            !present.contains(&log_name(std::process::id(), 1)),
            "{present:?}"
        );
    }

    /// El renombrado deja el nombre con el PID del hijo; el contenido del log no
    /// cambia porque el proceso escribe por su handle, no por su nombre.
    #[test]
    fn rename_log_to_pid_puts_the_child_pid_in_the_name() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        let (path, _file) =
            create_log_at_with(&dir, "daemon", std::process::id(), 77, &none_alive).unwrap();
        let renamed = rename_log_to_pid(&path, "daemon", 555);
        assert_eq!(
            renamed.file_name().unwrap().to_string_lossy(),
            "daemon_555_77.log"
        );
        assert!(renamed.exists());
        assert!(!path.exists());
    }

    /// Un nombre que no sigue el patrón se devuelve tal cual, sin fallar: el
    /// renombrado es una mejora del nombre, nunca un requisito del arranque.
    #[test]
    fn rename_log_to_pid_leaves_unparsable_names_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        std::fs::create_dir_all(&dir).unwrap();
        let odd = dir.join("daemon.log");
        std::fs::write(&odd, "x").unwrap();
        assert_eq!(rename_log_to_pid(&odd, "daemon", 555), odd);
    }

    fn log_name(pid: u32, ms: u128) -> String {
        format!("daemon_{pid}_{ms}.log")
    }
}
