// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! Creación de logs por proceso, con retención por familia.
//!
//! Cada log se llama `<familia>_<token>_<ms>.log`: `<token>` es el token de
//! propietario (`<pid>-<inicio>`) del proceso que lo escribe y `<ms>` los
//! milisegundos desde la época Unix. Al crear uno se poda su familia a los
//! `LOG_RETENTION` más recientes, de modo que el disco no crece con cada
//! arranque.
//!
//! La poda nunca borra el log de un propietario que sigue en marcha. Su token se
//! lee del nombre y se contrasta con la clasificación que inyecta el llamante: el
//! PID solo no basta, porque el sistema lo reutiliza y un PID de otro proceso
//! haría pasar por vivo un log huérfano. Se conservan los logs cuyo propietario
//! es `Ours` y los `Unknown` (no se puede observar, y no se debe dar por muerto
//! lo que no se sabe); solo los `Dead` (inexistente o con otro inicio) compiten
//! por la retención. Sin esa guarda, con diez o más arranques se podría borrar el
//! log de un daemon en marcha, porque el nombre solo ordena por antigüedad. La
//! exención no alcanza a los logs del propio propietario del proceso que poda:
//! esos sí se retiran, que es justo lo que la rotación necesita.

use crate::{OwnerState, OwnerToken};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

/// Logs que se conservan por familia.
pub const LOG_RETENTION: usize = 10;

/// Crea `<dir>/<family>_<token>_<ms>.log` con el token de `owner` (el proceso
/// que invoca), el directorio si falta, y poda la familia. Recibe el directorio
/// por parámetro para que las pruebas no toquen los logs reales, y la
/// clasificación de propietarios que decide qué logs ajenos la poda conserva.
pub fn create_log(
    dir: &Path,
    family: &str,
    owner: &OwnerToken,
    classify: &dyn Fn(&OwnerToken) -> OwnerState,
) -> std::io::Result<(PathBuf, File)> {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    create_log_at_with(dir, family, owner, millis, classify)
}

/// El cuerpo de la creación, con el milisegundo y la clasificación inyectados:
/// las pruebas fijan qué propietarios están vivos sin lanzar procesos reales, y
/// el resto de la poda (patrón del nombre, `keep`, retención) es el mismo.
///
/// Si el nombre ya existe (dos logs del mismo proceso en el mismo milisegundo) se
/// reintenta con el milisegundo siguiente, que conserva el orden por antigüedad.
fn create_log_at_with(
    dir: &Path,
    family: &str,
    owner: &OwnerToken,
    millis: u128,
    classify: &dyn Fn(&OwnerToken) -> OwnerState,
) -> std::io::Result<(PathBuf, File)> {
    std::fs::create_dir_all(dir)?;
    let mut millis = millis;
    let (path, file) = loop {
        let path = dir.join(log_file_name(family, owner, millis));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => break (path, file),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => millis += 1,
            Err(e) => return Err(e),
        }
    };
    prune(dir, family, owner, &path, classify);
    Ok((path, file))
}

fn log_file_name(family: &str, owner: &OwnerToken, millis: u128) -> String {
    format!("{family}_{}_{millis}.log", owner.format())
}

/// Renombra un log ya creado para que su nombre lleve el token indicado.
///
/// Lo usa el lanzador tras el arranque: el log se creó con el token del lanzador
/// y se renombra con el del hijo, de modo que el nombre identifica al proceso que
/// escribe. Devuelve la ruta nueva. Un nombre que no sigue el patrón de la
/// familia o un renombrado fallido son errores: devolver la ruta original haría
/// pasar el log por el del hijo, y la poda lo trataría como el de un proceso
/// distinto del que lo escribe.
///
/// El renombrado nunca pisa un fichero: `fs::rename` reemplaza el destino en
/// ambas plataformas, así que antes se reserva el nombre creándolo con
/// `create_new`. Un destino que ya existe es un error (`AlreadyExists`) y queda
/// intacto; si el renombrado falla tras la reserva, el marcador se retira.
pub fn rename_log_to_owner(
    path: &Path,
    family: &str,
    owner: &OwnerToken,
) -> std::io::Result<PathBuf> {
    let ms = path
        .file_name()
        .and_then(|n| log_id(&n.to_string_lossy(), family))
        .map(|(_, ms)| ms)
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "El nombre del log '{}' no sigue el patrón '{family}_<pid>-<inicio>_<ms>.log'",
                    path.display()
                ),
            )
        })?;
    let dir = path.parent().unwrap_or(Path::new("."));
    let target = dir.join(log_file_name(family, owner, ms));
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&target)?;
    if let Err(e) = std::fs::rename(path, &target) {
        crate::best_effort(
            std::fs::remove_file(&target),
            format_args!("no se pudo borrar el marcador {}", target.display()),
        );
        return Err(e);
    }
    Ok(target)
}

/// Extrae el token y `<ms>` de `<family>_<token>_<u128>.log`; `None` si el
/// nombre no sigue el patrón. El token no contiene `_`, así que el último `_`
/// separa los milisegundos.
fn log_id(name: &str, family: &str) -> Option<(OwnerToken, u128)> {
    let rest = name.strip_prefix(family)?.strip_prefix('_')?;
    let rest = rest.strip_suffix(".log")?;
    let (token, millis) = rest.rsplit_once('_')?;
    Some((OwnerToken::parse(token)?, millis.parse::<u128>().ok()?))
}

/// Borra los logs de la familia que excedan `LOG_RETENTION`, los más antiguos
/// primero, sin tocar nunca el log recién creado (`keep`), aunque el reloj haya
/// retrocedido. Un fallo de borrado (en Windows, un log abierto por otro proceso)
/// se ignora y la poda sigue con el resto.
///
/// `classify` dice qué propietarios siguen en marcha. Un log cuyo propietario,
/// distinto de `own`, sea `Ours` o `Unknown` se conserva siempre y no cuenta
/// para la retención: borrarlo dejaría al proceso vivo sin su traza justo cuando
/// más la necesita. `own` no se consulta (la exclusión vive aquí, no en la
/// clasificación), para que los logs antiguos del proceso que poda sigan siendo
/// podables.
fn prune(
    dir: &Path,
    family: &str,
    own: &OwnerToken,
    keep: &Path,
    classify: &dyn Fn(&OwnerToken) -> OwnerState,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut logs: Vec<(u128, String)> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            log_id(&name, family).map(|(token, ms)| (token, ms, name))
        })
        .filter(|(_, _, name)| dir.join(name) != keep)
        .filter(|(token, _, _)| token == own || classify(token) == OwnerState::Dead)
        .map(|(_, ms, name)| (ms, name))
        .collect();
    logs.sort();
    // El log recién creado cuenta como uno de los conservados.
    let excess = logs.len().saturating_sub(LOG_RETENTION - 1);
    for (_, name) in logs.into_iter().take(excess) {
        let path = dir.join(name);
        crate::best_effort(
            std::fs::remove_file(&path),
            format_args!("no se pudo podar el log {}", path.display()),
        );
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

    fn token(pid: u32, start: &str) -> OwnerToken {
        OwnerToken::new(pid, start).unwrap()
    }

    /// Propietario del proceso que poda en las pruebas.
    fn own() -> OwnerToken {
        token(std::process::id(), "100")
    }

    /// Propietario sembrado como vivo: no corresponde a ningún proceso real, la
    /// respuesta la fija la clasificación inyectada.
    fn live() -> OwnerToken {
        token(424_242, "200")
    }

    /// Propietario sembrado como muerto, tampoco real.
    fn dead() -> OwnerToken {
        token(424_243, "300")
    }

    fn log_name(owner: &OwnerToken, ms: u128) -> String {
        format!("daemon_{}_{ms}.log", owner.format())
    }

    /// Clasificación fija: ningún propietario ajeno vive.
    fn none_alive(_owner: &OwnerToken) -> OwnerState {
        OwnerState::Dead
    }

    /// Doce creaciones dejan solo los diez logs más recientes de la familia.
    #[test]
    fn create_log_keeps_ten_most_recent_per_family() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        for ms in 1..=12u128 {
            create_log_at_with(&dir, "daemon", &own(), ms, &none_alive).unwrap();
        }
        let present = names(&dir);
        assert_eq!(present.len(), LOG_RETENTION, "{present:?}");
        for ms in 1..=2u128 {
            assert!(!present.contains(&log_name(&own(), ms)));
        }
        for ms in 3..=12u128 {
            assert!(present.contains(&log_name(&own(), ms)));
        }
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
            create_log_at_with(&dir, "qwen3-tts", &own(), ms, &none_alive).unwrap();
        }
        for ms in 1..=12u128 {
            create_log_at_with(&dir, "daemon", &own(), 100 + ms, &none_alive).unwrap();
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
        let (first, _a) = create_log_at_with(&dir, "daemon", &own(), 7, &none_alive).unwrap();
        let (second, _b) = create_log_at_with(&dir, "daemon", &own(), 7, &none_alive).unwrap();
        assert_ne!(first, second);
        assert!(second.to_string_lossy().ends_with("_8.log"), "{second:?}");
    }

    /// El nombre es `<familia>_<pid>-<inicio>_<ms>.log`.
    #[test]
    fn create_log_names_family_token_and_millis() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        let (path, _file) = create_log_at_with(&dir, "daemon", &own(), 42, &none_alive).unwrap();
        assert_eq!(
            path.file_name().unwrap().to_string_lossy(),
            format!("daemon_{}-100_42.log", std::process::id())
        );
    }

    /// La poda conserva el log de un propietario vivo aunque sea el más
    /// antiguo, y poda el resto hasta la retención: es la razón de la guarda de
    /// viveza, que sin ella dejaría al proceso vivo sin traza al undécimo
    /// arranque.
    #[test]
    fn prune_skips_logs_of_live_owners() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        let live_owner = live();
        let classify = |t: &OwnerToken| {
            if *t == live_owner {
                OwnerState::Ours
            } else {
                OwnerState::Dead
            }
        };
        // El log del propietario vivo es el más antiguo de la familia.
        let (live_path, _live_file) =
            create_log_at_with(&dir, "daemon", &live(), 1, &classify).unwrap();
        for ms in 2..=12u128 {
            create_log_at_with(&dir, "daemon", &dead(), ms, &classify).unwrap();
        }
        let present = names(&dir);
        assert!(present.contains(&log_name(&live(), 1)), "{present:?}");
        assert!(live_path.exists());
        // El vivo no consume plaza de retención: los once logs del muerto se
        // podan como si el vivo no existiera, y solo cae el más antiguo.
        let dead_count = present
            .iter()
            .filter(|n| n.starts_with(&format!("daemon_{}_", dead().format())))
            .count();
        assert_eq!(dead_count, LOG_RETENTION, "{present:?}");
        assert!(!present.contains(&log_name(&dead(), 2)), "{present:?}");
        assert!(present.contains(&log_name(&dead(), 3)), "{present:?}");
    }

    /// Un propietario que no se puede observar (`Unknown`) se conserva igual que
    /// uno vivo: no se debe dar por muerto lo que no se sabe.
    #[test]
    fn prune_keeps_logs_of_unknown_owners() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        let unknown_owner = live();
        let classify = |t: &OwnerToken| {
            if *t == unknown_owner {
                OwnerState::Unknown
            } else {
                OwnerState::Dead
            }
        };
        create_log_at_with(&dir, "daemon", &live(), 1, &classify).unwrap();
        for ms in 2..=14u128 {
            create_log_at_with(&dir, "daemon", &dead(), ms, &classify).unwrap();
        }
        let present = names(&dir);
        assert!(present.contains(&log_name(&live(), 1)), "{present:?}");
        assert_eq!(present.len(), LOG_RETENTION + 1, "{present:?}");
    }

    /// PID reutilizado: el mismo PID con otro inicio no es el propietario del
    /// log, así que se clasifica como muerto y su log se poda.
    #[test]
    fn prune_removes_logs_of_a_reused_pid() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        let original = token(424_242, "200");
        let reused_owner = token(424_242, "999");
        // Hoy vive el PID con el inicio nuevo; el del log original ya no existe.
        let classify = |t: &OwnerToken| {
            if *t == reused_owner {
                OwnerState::Ours
            } else {
                OwnerState::Dead
            }
        };
        create_log_at_with(&dir, "daemon", &original, 1, &classify).unwrap();
        for ms in 2..=11u128 {
            create_log_at_with(&dir, "daemon", &own(), ms, &classify).unwrap();
        }
        let present = names(&dir);
        assert!(!present.contains(&log_name(&original, 1)), "{present:?}");
        assert_eq!(present.len(), LOG_RETENTION, "{present:?}");
    }

    /// El recién creado es `keep`: sobrevive a su propia poda aunque el reloj
    /// haya retrocedido y sea el más antiguo de la familia.
    #[test]
    fn prune_never_deletes_the_just_created_log() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        for ms in (1..=12u128).rev() {
            let (keep, _keep_file) =
                create_log_at_with(&dir, "daemon", &own(), ms, &none_alive).unwrap();
            assert!(keep.exists(), "el log recién creado no debe podarse");
        }
        assert_eq!(names(&dir).len(), LOG_RETENTION);
    }

    /// El propietario propio nunca se consulta a la clasificación: aunque esta lo
    /// declare vivo (el proceso que poda siempre lo está), sus logs antiguos
    /// siguen siendo podables y la retención se cumple.
    #[test]
    fn prune_ignores_the_classification_for_the_own_owner() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        let all_ours = |_t: &OwnerToken| OwnerState::Ours;
        for ms in 1..=12u128 {
            create_log_at_with(&dir, "daemon", &own(), ms, &all_ours).unwrap();
        }
        let present = names(&dir);
        assert_eq!(present.len(), LOG_RETENTION, "{present:?}");
        assert!(!present.contains(&log_name(&own(), 1)), "{present:?}");
    }

    /// El renombrado deja el nombre con el token del hijo; el contenido del log
    /// no cambia porque el proceso escribe por su handle, no por su nombre.
    #[test]
    fn rename_log_to_owner_puts_the_child_token_in_the_name() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        let (path, _file) = create_log_at_with(&dir, "daemon", &own(), 77, &none_alive).unwrap();
        let renamed = rename_log_to_owner(&path, "daemon", &token(555, "abc")).unwrap();
        assert_eq!(
            renamed.file_name().unwrap().to_string_lossy(),
            "daemon_555-abc_77.log"
        );
        assert!(renamed.exists());
        assert!(!path.exists());
    }

    /// Un nombre que no sigue el patrón es un error y el fichero no se toca.
    #[test]
    fn rename_log_to_owner_rejects_unparsable_names() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        std::fs::create_dir_all(&dir).unwrap();
        let odd = dir.join("daemon.log");
        std::fs::write(&odd, "x").unwrap();
        let err = rename_log_to_owner(&odd, "daemon", &token(555, "abc")).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        assert!(odd.exists());
    }

    /// Un renombrado que el sistema rechaza (el origen no existe) es un error,
    /// no la ruta original.
    #[test]
    fn rename_log_to_owner_propagates_rename_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("daemon_1-a_77.log");
        let err = rename_log_to_owner(&missing, "daemon", &token(555, "abc")).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
        assert!(
            !tmp.path().join("daemon_555-abc_77.log").exists(),
            "el marcador reservado se retira"
        );
    }

    /// Un destino que ya existe no se pisa: el renombrado falla y tanto el
    /// fichero existente como el log de origen quedan intactos.
    #[test]
    fn rename_log_to_owner_never_overwrites_the_target() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        let source = dir.join("daemon_1-a_77.log");
        let taken = dir.join("daemon_555-abc_77.log");
        std::fs::write(&source, "origen").unwrap();
        std::fs::write(&taken, "ajeno").unwrap();
        let err = rename_log_to_owner(&source, "daemon", &token(555, "abc")).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read_to_string(&taken).unwrap(), "ajeno");
        assert_eq!(std::fs::read_to_string(&source).unwrap(), "origen");
    }
}
