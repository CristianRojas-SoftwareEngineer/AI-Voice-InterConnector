// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! Registro del daemon en disco: el esquema de `daemon.pid`, su escritura
//! atómica y su observación, y el parser único de `daemon.ready`.
//!
//! Vive en este crate porque es el dueño de [`ProcessIdentity`] y porque lo
//! comparten el daemon, el motor de voz y el ciclo de vida, que no dependen
//! entre sí. Un solo formato con un solo intérprete evita que cada lector lo
//! reinterprete por su cuenta.

use crate::ProcessIdentity;
use std::path::{Path, PathBuf};

/// Nombre del pidfile dentro de la raíz de datos.
pub const PID_FILE: &str = "daemon.pid";

/// Nombre del fichero ready dentro de la raíz de datos: el daemon publica en él
/// su dirección real y su PID cuando está listo.
pub const READY_FILE: &str = "daemon.ready";

/// Ruta del pidfile bajo la raíz de datos.
pub fn pid_path(data_dir: &Path) -> PathBuf {
    data_dir.join(PID_FILE)
}

/// Contenido del pidfile tal como está en disco. Las claves de identidad y de
/// puerto del residente son obligatorias en la observación: un fichero sin
/// ellas no cumple el esquema y se descarta.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PidFile {
    pub pid: u32,
    /// Dirección **real** publicada por el daemon: con puerto efímero difiere
    /// de la literal.
    #[serde(default)]
    pub addr: Option<String>,
    #[serde(default)]
    pub started_at: Option<String>,
    /// PID del residente; `0` mientras no hay ninguno registrado.
    #[serde(default)]
    pub resident_pid: u32,
    /// Identidad del proceso del daemon observada al registrarlo. Sin ella el
    /// PID no se trata como nuestro, y la clave tiene que estar (`null`).
    pub identity: Option<ProcessIdentity>,
    /// Identidad del residente observada al registrarlo, con el mismo criterio.
    pub resident_identity: Option<ProcessIdentity>,
    /// Puerto de loopback en el que escucha el residente registrado, o `null`
    /// si no hay ninguno.
    pub resident_port: Option<u16>,
}

/// Escribe el pidfile completo de forma atómica: temporal hermano y renombrado,
/// de modo que un lector nunca ve medio JSON.
pub fn write_pid_file(data_dir: &Path, file: &PidFile) -> anyhow::Result<()> {
    let path = pid_path(data_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = serde_json::to_string_pretty(file)?;
    let temp = path.with_extension("pid.tmp");
    std::fs::write(&temp, content.as_bytes())?;
    match std::fs::rename(&temp, &path) {
        Ok(()) => Ok(()),
        Err(e) => {
            avi_shared::best_effort(
                std::fs::remove_file(&temp),
                format_args!("no se pudo borrar el temporal {}", temp.display()),
            );
            Err(e.into())
        }
    }
}

/// Clasificación de un PID leído de un fichero de estado.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Registered {
    /// No hay proceso con ese PID (o el PID no está registrado).
    Absent,
    /// Hay un proceso con ese PID pero no es el registrado: el número se reasignó.
    /// Cuenta como muerto y nunca se mata.
    Foreign,
    /// El proceso es el registrado.
    Ours,
}

/// Decide si un PID registrado es nuestro. `alive` es la viveza que el llamante
/// observó con la primitiva que corresponde al proceso, y `observe` obtiene la
/// identidad actual del PID solo cuando hace falta compararla. Sin identidad
/// registrada no hay clasificación `Ours`: solo es nuestro si la identidad
/// observada coincide con la que se registró. Un fichero sin identidad es
/// ilegible y se propaga como ausencia, no como «PID solo».
pub fn classify_registered(
    pid: u32,
    alive: bool,
    recorded: Option<&ProcessIdentity>,
    observe: impl FnOnce() -> Option<ProcessIdentity>,
) -> Registered {
    if pid == 0 || !alive {
        return Registered::Absent;
    }
    match recorded {
        None => Registered::Absent,
        Some(recorded) if observe().as_ref() == Some(recorded) => Registered::Ours,
        Some(_) => Registered::Foreign,
    }
}

/// Instante actual en RFC 3339 UTC (`2026-09-25T18:00:00Z`). Sin `chrono` ni
/// `time` en el árbol, y añadir una dependencia por una fecha no lo compensa: el
/// cálculo es de día juliano a fecha civil. Lo comparten el pidfile del daemon
/// (`started_at`) y el recibo de instalación.
pub fn now_rfc3339() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default();
    rfc3339_from_unix(secs)
}

/// Formatea un instante Unix como RFC 3339 UTC.
fn rfc3339_from_unix(secs: i64) -> String {
    let (year, month, day, hour, minute, second) = civil_from_unix(secs);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Descompone un instante Unix en fecha civil UTC. Algoritmo days-from-civil,
/// que es exacto para cualquier fecha del calendario gregoriano.
fn civil_from_unix(secs: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let day_of_year = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let d = day_of_year - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    (
        year,
        m as u32,
        d as u32,
        (rem / 3600) as u32,
        ((rem % 3600) / 60) as u32,
        (rem % 60) as u32,
    )
}

/// Desenlace de observar el pidfile sin efectos.
#[derive(Debug)]
pub enum PidFileObservation {
    /// No hay fichero: ausencia normal.
    Absent,
    /// El fichero está ilegible o su esquema no se entiende; lleva el motivo
    /// listo para avisar.
    Discarded(String),
    /// Fichero válido.
    Present(PidFile),
}

/// Observa el pidfile sin emitir nada: devuelve el desenlace y deja el aviso a
/// quien decide cuándo darlo.
pub fn observe_pid_file(data_dir: &Path) -> PidFileObservation {
    let Ok(text) = std::fs::read_to_string(pid_path(data_dir)) else {
        return PidFileObservation::Absent;
    };
    // Como `Option<T>` se parsea igualmente ausente, la presencia de las claves
    // obligatorias se afirma sobre el JSON crudo.
    let value: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            return PidFileObservation::Discarded(format!("no se entiende su esquema ({e})"));
        }
    };
    let has_identity_keys =
        value.get("identity").is_some() && value.get("resident_identity").is_some();
    if !has_identity_keys {
        return PidFileObservation::Discarded("falta la identidad obligatoria".to_string());
    }
    if value.get("resident_port").is_none() {
        return PidFileObservation::Discarded(
            "falta el puerto del residente obligatorio".to_string(),
        );
    }
    match serde_json::from_value(value) {
        Ok(file) => PidFileObservation::Present(file),
        Err(e) => PidFileObservation::Discarded(format!("no se entiende su esquema ({e})")),
    }
}

/// Campos de `daemon.ready` que identifican al daemon listo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadyRecord {
    /// Dirección real en la que escucha el daemon.
    pub addr: String,
    /// PID publicado; `None` si falta, es ilegible o vale `0`.
    pub pid: Option<u32>,
    /// Identidad publicada; solo si vienen las dos líneas `start=` e `image=`.
    pub identity: Option<ProcessIdentity>,
}

/// Valor de la línea `clave=valor` del texto, recortado.
pub fn ready_field(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|line| {
        line.strip_prefix(key)
            .and_then(|rest| rest.strip_prefix('='))
            .map(|v| v.trim().to_string())
    })
}

/// Interpreta el registro de éxito de `daemon.ready`. `None` si no trae una
/// línea `addr=` no vacía: el registro de fallo nunca la lleva, así que un
/// lector que solo conoce el éxito lo ve como aún-no-listo.
pub fn parse_ready_record(text: &str) -> Option<ReadyRecord> {
    let addr = ready_field(text, "addr").filter(|a| !a.is_empty())?;
    let pid = ready_field(text, "pid")
        .and_then(|p| p.parse::<u32>().ok())
        .filter(|&p| p != 0);
    let identity = match (ready_field(text, "start"), ready_field(text, "image")) {
        (Some(start), Some(image)) => Some(ProcessIdentity { start, image }),
        _ => None,
    };
    Some(ReadyRecord {
        addr,
        pid,
        identity,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity_of(start: &str, image: &str) -> ProcessIdentity {
        ProcessIdentity {
            start: start.to_string(),
            image: image.to_string(),
        }
    }

    fn sample(pid: u32, resident_pid: u32) -> PidFile {
        PidFile {
            pid,
            addr: Some("127.0.0.1:7001".to_string()),
            started_at: None,
            resident_pid,
            identity: Some(identity_of("111", "daemon.exe")),
            resident_identity: None,
            resident_port: None,
        }
    }

    /// El pidfile guarda y devuelve las dos identidades y el puerto del
    /// residente.
    #[test]
    fn pid_file_roundtrips_identities_and_port() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut file = sample(4242, 77);
        write_pid_file(dir.path(), &file).unwrap();
        let PidFileObservation::Present(back) = observe_pid_file(dir.path()) else {
            panic!("el fichero escrito se observa como presente");
        };
        assert_eq!(back.identity, Some(identity_of("111", "daemon.exe")));
        assert_eq!(back.resident_identity, None);
        assert_eq!(back.resident_port, None);

        file.resident_identity = Some(identity_of("222", "qwen_tts.exe"));
        file.resident_port = Some(40_123);
        write_pid_file(dir.path(), &file).unwrap();
        let PidFileObservation::Present(back) = observe_pid_file(dir.path()) else {
            panic!("el fichero reescrito se observa como presente");
        };
        assert_eq!(back, file);
    }

    /// Un pidfile sin alguna clave obligatoria se descarta con su motivo; con
    /// todas las claves en `null` se lee.
    #[test]
    fn pid_file_requires_identity_and_port_keys() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = pid_path(dir.path());
        assert!(matches!(
            observe_pid_file(dir.path()),
            PidFileObservation::Absent
        ));

        std::fs::write(&path, r#"{"pid": 4242}"#).unwrap();
        assert!(matches!(
            observe_pid_file(dir.path()),
            PidFileObservation::Discarded(reason) if reason.contains("identidad")
        ));

        std::fs::write(
            &path,
            r#"{"pid": 4242, "addr": "127.0.0.1:7001", "resident_pid": 77, "identity": null, "resident_identity": null}"#,
        )
        .unwrap();
        assert!(matches!(
            observe_pid_file(dir.path()),
            PidFileObservation::Discarded(reason) if reason.contains("puerto")
        ));

        std::fs::write(
            &path,
            r#"{"pid": 4242, "addr": "127.0.0.1:7001", "resident_pid": 77, "identity": null, "resident_identity": null, "resident_port": null}"#,
        )
        .unwrap();
        let PidFileObservation::Present(file) = observe_pid_file(dir.path()) else {
            panic!("con todas las claves presentes se lee");
        };
        assert_eq!(file.identity, None);
        assert_eq!(file.resident_port, None);
    }

    /// Un fichero a medio escribir o que no es JSON se descarta con motivo, sin
    /// confundirse con un PID.
    #[test]
    fn truncated_or_garbage_pid_file_is_discarded() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = pid_path(dir.path());
        for garbage in [
            "{\"pid\": 4242, \"addr\": \"127.0.0.1:876",
            "{}",
            r#"{"pid": "x"}"#,
            "[]",
            "no soy json",
            "",
        ] {
            std::fs::write(&path, garbage).unwrap();
            assert!(
                matches!(
                    observe_pid_file(dir.path()),
                    PidFileObservation::Discarded(reason) if !reason.is_empty()
                ),
                "basura: {garbage:?}"
            );
        }
    }

    /// La escritura es atómica: lo que hay en disco es siempre el registro
    /// completo más reciente y no queda temporal.
    #[test]
    fn write_is_atomic_and_leaves_no_temporary() {
        let dir = tempfile::TempDir::new().unwrap();
        for round in 0..64u32 {
            let file = sample(1000 + round, round);
            write_pid_file(dir.path(), &file).unwrap();
            let PidFileObservation::Present(back) = observe_pid_file(dir.path()) else {
                panic!("cada lectura ve un registro completo");
            };
            assert_eq!(back, file, "se lee el último registro escrito");
        }
        assert!(!pid_path(dir.path()).with_extension("pid.tmp").exists());
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            1,
            "solo queda el pidfile"
        );
    }

    /// La clasificación solo da `Ours` con proceso vivo e identidad coincidente;
    /// la identidad actual no se observa si el PID no está vivo o no hay registro.
    #[test]
    fn classification_requires_live_matching_identity() {
        let recorded = identity_of("111", "daemon.exe");
        let same = || Some(identity_of("111", "daemon.exe"));
        let other = || Some(identity_of("999", "daemon.exe"));
        let never = || -> Option<ProcessIdentity> { panic!("no debe observarse") };

        assert_eq!(
            classify_registered(7, true, Some(&recorded), same),
            Registered::Ours
        );
        assert_eq!(
            classify_registered(7, true, Some(&recorded), other),
            Registered::Foreign
        );
        assert_eq!(
            classify_registered(7, true, Some(&recorded), || None),
            Registered::Foreign
        );
        assert_eq!(
            classify_registered(7, false, Some(&recorded), never),
            Registered::Absent
        );
        assert_eq!(
            classify_registered(0, true, Some(&recorded), never),
            Registered::Absent
        );
        assert_eq!(
            classify_registered(7, true, None, never),
            Registered::Absent
        );
    }

    /// El formato RFC 3339 coincide con instantes conocidos.
    #[test]
    fn rfc3339_formats_known_instants() {
        assert_eq!(rfc3339_from_unix(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339_from_unix(1_000_000_000), "2001-09-09T01:46:40Z");
        assert_eq!(rfc3339_from_unix(1_709_164_800), "2024-02-29T00:00:00Z");
        assert_eq!(now_rfc3339().len(), 20);
    }

    /// El registro de éxito del ready se interpreta con y sin identidad, y el de
    /// fallo o el vacío no cuentan como listo.
    #[test]
    fn ready_record_parsing() {
        let full = "addr=127.0.0.1:8765\nwarm=ok\npid=42\nstart=111\nimage=daemon.exe\n";
        assert_eq!(
            parse_ready_record(full),
            Some(ReadyRecord {
                addr: "127.0.0.1:8765".to_string(),
                pid: Some(42),
                identity: Some(identity_of("111", "daemon.exe")),
            })
        );

        let partial = "addr=127.0.0.1:8765\nwarm=ok\npid=0\nstart=111\n";
        assert_eq!(
            parse_ready_record(partial),
            Some(ReadyRecord {
                addr: "127.0.0.1:8765".to_string(),
                pid: None,
                identity: None,
            }),
            "un pid cero y una identidad a medias no se interpretan"
        );

        assert_eq!(
            parse_ready_record("error=startup_failed\nmessage=x\n"),
            None
        );
        assert_eq!(parse_ready_record("addr=\npid=42\n"), None);
        assert_eq!(parse_ready_record(""), None);
    }
}
