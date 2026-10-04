//! Recuperación al empezar una operación de ciclo de vida.
//!
//! Toda operación de ciclo de vida, y `doctor` en modo informe, hace dos cosas al
//! empezar, y en este orden: completar la transacción que una operación anterior
//! dejó a medias, y barrer lo que quedó huérfano. Sin esto, una instalación
//! interrumpida deja el directorio de programa en un estado que el usuario no
//! puede distinguir del bueno, y cada litter de aparcaos, stagings y temporales
//! se acumula sin que nada lo recoja.
//!
//! 1. **Transacción pendiente.** Si hay diario, se revierte restaurando lo
//!    aparcado, o se completa el commit si el diario estaba confirmado. La
//!    diferencia entre las dos cosas es el estado del diario, no una heurística:
//!    por eso el diario se marca confirmado *antes* de borrar el aparcado.
//! 2. **Barrido.** Aparcados `.old-*` que ya no estén en uso, stagings huérfanos
//!    hermanos del directorio de programa y temporales propios con los prefijos
//!    de la tabla de rutas, sin proceso vivo.
//!
//! **Se llama con el bloqueo tomado** (paso 1 de la instalación): si no, el barrido
//! podría
//! llevarse por delante el staging que otra operación está usando. Por eso este
//! módulo no toma el bloqueo por su cuenta.
//!
//! Lo que no se puede borrar **no es un fallo**: son archivos en uso, y el
//! borrado diferido los recoge después. El resultado enumera lo que se
//! quitó y lo que se quedó, para que `doctor` pueda informarlo, que es lo que
//! la operación exige explícitamente.

use crate::transaction;
use anyhow::Result;
use std::path::{Path, PathBuf};

/// Raíces sobre las que actúa la recuperación. `temp_root` es un parámetro —como
/// la clave de registro en la integración del `PATH`— para que las pruebas no barran
/// el directorio temporal de la máquina.
#[derive(Debug, Clone, Copy)]
pub struct Roots<'a> {
    /// Directorio de programa, donde vive el diario y los aparcados.
    pub program_dir: &'a Path,
    /// Directorio de temporales del sistema, donde viven los temporales propios.
    pub temp_root: &'a Path,
    /// Staging que la operación en curso está usando, si lo hay.
    ///
    /// El barrido recoge "stagings **huérfanos**", y un staging del que se va a
    /// instalar no lo es aunque todavía no haya nada dentro: es el bundle que el
    /// paso 2 de la instalación valida y el paso 6 coloca. Sin este campo,
    /// `self install` se borraría a sí
    /// mismo el bundle entre el paso 1 y el paso 2. `None` en `doctor` y en
    /// `cleanup`, que no vienen a instalar nada.
    pub in_use: Option<&'a Path>,
}

/// Lo que el barrido **tocaría ahora mismo**, calculado sin tocar nada.
///
/// Es la forma de informe: `doctor` en modo informe y el plan de
/// `cleanup --dry-run` muestran esta lista, y el barrido real usa la misma
/// decisión. Derivar las dos de funciones distintas es lo que produjo el defecto
/// que T14 absorbe en el planificador de borrado: el plan anunciaba `xet` y
/// `.locks` sin mirar si la raíz era compartida mientras el ejecutor devolvía
/// `Ok(false)`, de modo que la simulación mentía.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SweepPreview {
    /// Aparcados `.old-*` del directorio de programa.
    pub parked: Vec<PathBuf>,
    /// Stagings huérfanos hermanos del directorio de programa.
    pub stagings: Vec<PathBuf>,
    /// Temporales propios huérfanos del directorio de temporales.
    pub temporaries: Vec<PathBuf>,
    /// Temporales propios que **no** se pueden decidir: los de un proceso vivo.
    /// No es un fallo ni un residuo que se pueda forzar.
    pub temporaries_kept: Vec<PathBuf>,
}

impl SweepPreview {
    /// Todo lo que el barrido tocaría, en el orden en que lo recorrería.
    pub fn all(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        out.extend(self.parked.iter().cloned());
        out.extend(self.stagings.iter().cloned());
        out.extend(self.temporaries.iter().cloned());
        out
    }
}

/// Estado final de la recuperación, observable por `doctor`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecoveryOutcome {
    /// Hubo diario y se revirtió: la versión anterior volvió al sitio.
    pub rolled_back: bool,
    /// Hubo diario confirmado y se completó el commit.
    pub committed: bool,
    /// Aparcados `.old-*` borrados.
    pub removed_parked: Vec<PathBuf>,
    /// Stagings huérfanos borrados.
    pub removed_stagings: Vec<PathBuf>,
    /// Temporales propios huérfanos borrados.
    pub removed_temporaries: Vec<PathBuf>,
    /// Lo que no se pudo borrar: en uso, o sin PID con el que decidir. No es un
    /// fallo.
    pub kept: Vec<PathBuf>,
}

impl RecoveryOutcome {
    /// `true` si no queda nada pendiente de una operación anterior.
    pub fn is_clean(&self) -> bool {
        self.kept.is_empty()
    }
}

/// Recuperación con las raíces del producto: directorio de programa según la
/// reubicación y directorio de temporales del sistema.
pub fn recover_default() -> Result<RecoveryOutcome> {
    let program_dir = crate::install_dir();
    let temp_root = std::env::temp_dir();
    recover(Roots {
        program_dir: &program_dir,
        temp_root: &temp_root,
        in_use: None,
    })
}

/// Completa la transacción pendiente y barre lo huérfano.
pub fn recover(roots: Roots<'_>) -> Result<RecoveryOutcome> {
    let mut outcome = RecoveryOutcome::default();

    // 1. Transacción pendiente.
    if let Some(journal) = transaction::read_journal(roots.program_dir)? {
        if journal.state.is_committed() {
            // La confirmación se escribió antes de borrar el aparcado: si el
            // diario está confirmado, lo que queda por hacer es el commit.
            if let Some(parked) = journal.parked_dir.as_ref().filter(|p| p.is_dir()) {
                match std::fs::remove_dir_all(parked) {
                    Ok(()) => outcome.removed_parked.push(parked.clone()),
                    Err(_) => outcome.kept.push(parked.clone()),
                }
            }
            outcome.committed = true;
        } else {
            transaction::rollback(roots.program_dir, &journal)?;
            outcome.rolled_back = true;
        }
        let journal_path = transaction::journal_path(roots.program_dir);
        match std::fs::remove_file(&journal_path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => outcome.kept.push(journal_path),
        }
    }

    // 2. Barrido de aparcados, stagings y temporales, con la decisión del
    //    informe: una sola clasificación para el plan y para el barrido.
    let preview = preview(roots);
    sweep_parked(&preview, &mut outcome);
    sweep_stagings(&preview, &mut outcome);
    sweep_temporaries(&preview, &mut outcome);

    Ok(outcome)
}

/// Qué haría el barrido sin tocar nada. `doctor` lo usa como informe
/// en modo informe y `cleanup --dry-run` lo anuncia; el barrido real usa
/// la misma decisión. Derivar el plan y la ejecución de funciones distintas es lo
/// que produjo el defecto que T14 absorbe en el planificador de borrado: el plan
/// anunciaba `xet` y `.locks` sin mirar si la raíz era compartida mientras el
/// ejecutor devolvía `Ok(false)`, de modo que la simulación mentía.
pub fn preview(roots: Roots<'_>) -> SweepPreview {
    let parent = roots.program_dir.parent().unwrap_or(Path::new(""));
    SweepPreview {
        parked: entries_with_prefix(roots.program_dir, crate::PARKED_DIR_PREFIX),
        stagings: entries_with_prefix(parent, crate::STAGING_DIR_PREFIX)
            .into_iter()
            .filter(|path| roots.in_use.is_none_or(|using| using != path))
            .collect(),
        temporaries: temporaries_decision(roots.temp_root).0,
        temporaries_kept: temporaries_decision(roots.temp_root).1,
    }
}

/// Aparcados `.old-*` del directorio de programa que ya no estén en uso. Los que
/// el SO no deja borrar quedan en `kept` para el borrado diferido.
fn sweep_parked(preview: &SweepPreview, outcome: &mut RecoveryOutcome) {
    for path in &preview.parked {
        match std::fs::remove_dir_all(path) {
            Ok(()) => outcome.removed_parked.push(path.clone()),
            Err(_) => outcome.kept.push(path.clone()),
        }
    }
}

/// Stagings huérfanos: hermanos del directorio de programa con el prefijo de la
// tabla de rutas,
/// que es donde los deja `self install` y `self update`. Solo hermanos, nunca
/// otras rutas del padre (R1).
///
/// `in_use` queda fuera del informe: es el staging del que la operación en curso va
/// a instalar, que no es huérfano por definición aunque esté vacío.
fn sweep_stagings(preview: &SweepPreview, outcome: &mut RecoveryOutcome) {
    for path in &preview.stagings {
        match std::fs::remove_dir_all(path) {
            Ok(()) => outcome.removed_stagings.push(path.clone()),
            Err(_) => outcome.kept.push(path.clone()),
        }
    }
}

/// Temporales propios con los prefijos de la tabla de rutas.
///
/// Un archivo con el PID en el nombre solo se borra si ese proceso ya no existe.
/// Uno **sin** PID reconocible también se barre: cualquier invocación
/// barre los temporales propios huérfanos, y un temporal sin PID no pertenece a
/// ningún proceso vivo, luego está huérfano por definición. Conservarlos para
/// siempre era lo que hacía que el barrido no recogiera nunca lo que el producto
/// deja al caer una clonación o una síntesis.
///
/// Lo que protege al producto de borrarse a sí mismo no es la ausencia de PID sino el
/// **resultado** del borrado: en Windows el SO rechaza borrar un archivo que otro
/// proceso tiene abierto, y la ruta cae en `kept` para el borrado diferido. En Unix
/// el `unlink` se resuelve sin error, pero el escritor conserva su descriptor y su
/// escritura sigue yendo al inodo ya desenlazado: lo que se pierde es el nombre del
/// archivo, no el proceso.
fn sweep_temporaries(preview: &SweepPreview, outcome: &mut RecoveryOutcome) {
    for path in &preview.temporaries {
        let removed = if path.is_dir() {
            std::fs::remove_dir_all(path)
        } else {
            std::fs::remove_file(path)
        };
        match removed {
            Ok(()) => outcome.removed_temporaries.push(path.clone()),
            Err(_) => outcome.kept.push(path.clone()),
        }
    }
    outcome
        .kept
        .extend(preview.temporaries_kept.iter().cloned());
}

/// Clasificación de los temporales propios: `(los que se barren, los que se
/// conservan)`. Es la decisión única de la que salen el informe y el barrido.
fn temporaries_decision(temp_root: &Path) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut remove_path = Vec::new();
    let mut kept = Vec::new();
    let entries = match std::fs::read_dir(temp_root) {
        Ok(entries) => entries,
        Err(_) => return (remove_path, kept),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if !crate::TEMP_PREFIXES
            .iter()
            .any(|prefix| name.starts_with(prefix))
        {
            continue;
        }
        match owner_pid(&name) {
            // Proceso vivo: no se toca.
            Some(pid) if process_is_alive(pid) => kept.push(path),
            // PID muerto, o sin PID: huérfano por definición.
            _ => remove_path.push(path),
        }
    }
    remove_path.sort();
    kept.sort();
    (remove_path, kept)
}

/// Entradas de `dir` cuyo nombre empieza por `prefix`, ordenadas.
fn entries_with_prefix(dir: &Path, prefix: &str) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with(prefix))
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    out.sort();
    out
}

/// PID del proceso dueño de un temporal propio, si el nombre lo lleva.
///
/// Se busca la primera racha de dígitos de tres o más porque la convención de
/// los temporales con dueño es `<algo>-<pid>-<ms>`: las copias del limpiador
/// propio (`avi-cleaner-<pid>-<ms>.exe`) y, de versiones anteriores, los
/// auxiliares retirados (`avi-deferred-<pid>-<ms>` con extensión `.ps1` o
/// `.ready`). Un número de una cifra es más probablemente un contador que un
/// PID: con eso, un temporal como `avi_clone_x_1.qvoice` no se confunde con un
/// proceso.
fn owner_pid(name: &str) -> Option<u32> {
    let bytes: Vec<char> = name.chars().collect();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            let run: String = bytes[start..i].iter().collect();
            if run.len() >= 3 {
                return run.parse().ok();
            }
        } else {
            i += 1;
        }
    }
    None
}

/// `true` si el proceso sigue vivo.
///
/// En Unix se pregunta con `kill(pid, 0)`, que no necesita permiso para preguntar
/// por la existencia. En Windows se abre el proceso con `PROCESS_SYNCHRONIZE` y
/// se espera cero milisegundos: si sigue sin terminar, está vivo. El borrado por
/// sí solo no basta como prueba —un temporal de un proceso vivo no siempre está
/// abierto—, así que en las dos plataformas la pregunta se hace antes de borrar.
#[cfg(unix)]
fn process_is_alive(pid: u32) -> bool {
    let pid = pid as libc::pid_t;
    if pid <= 0 {
        return false;
    }
    // SAFETY: `kill` con señal 0 solo consulta la tabla de procesos.
    if unsafe { libc::kill(pid, 0) } == 0 {
        return true;
    }
    // El rechazo por permiso vive en el errno del hilo: se lee con la lectura
    // portátil en vez del accesorio por plataforma, que solo existe en Linux.
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM as i32)
}

#[cfg(windows)]
fn process_is_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
    };

    // SAFETY: se pide un permiso de solo sincronización y un tiempo de espera de
    // cero; el descriptor se cierra antes de devolver.
    unsafe {
        let handle = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if handle == INVALID_HANDLE_VALUE || handle == 0 {
            // El proceso no existe o no es accesible: no está vivo.
            return false;
        }
        let waited = WaitForSingleObject(handle, 0);
        CloseHandle(handle);
        waited != WAIT_OBJECT_0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{scratch, write_file};
    use crate::transaction::{Journal, JournalState};

    /// Escribe un diario como lo dejaría una operación que murió a mitad.
    fn write_journal(program_dir: &std::path::Path, journal: &Journal) {
        std::fs::write(
            transaction::journal_path(program_dir),
            serde_json::to_string(journal).unwrap(),
        )
        .unwrap();
    }

    /// El nombre de un temporal propio con PID se lee, y uno sin PID
    /// reconocible no se inventa.
    #[test]
    fn temp_owner_pid_is_read_from_the_name() {
        assert_eq!(
            owner_pid("avi-deferred-12345-1780000000000.ps1"),
            Some(12345)
        );
        assert_eq!(
            owner_pid("avi-deferred-12345-1780000000000.ready"),
            Some(12345)
        );
        assert_eq!(owner_pid("avi_12345.tmp"), Some(12345));
        assert_eq!(owner_pid("avi_clone_x_1.qvoice"), None);
        assert_eq!(owner_pid("avi-temp"), None);
        assert!(
            !process_is_alive(u32::MAX),
            "un PID que no existe no está vivo"
        );
        assert!(
            process_is_alive(std::process::id()),
            "este proceso está vivo"
        );
    }

    /// La recuperación barre aparcados, stagings y temporales huérfanos, y deja
    /// intacto lo que no es suyo.
    ///
    /// El temporal **sin** PID se barre: la regla exige que cualquier invocación barra
    /// los temporales propios huérfanos, y uno sin PID no pertenece a ningún proceso
    /// vivo. Conservarlo era lo que hacía que `avi_clone_x_1.qvoice` se acumulara sin
    /// que nada lo recogiera.
    #[test]
    fn sweep_removes_orphans_and_keeps_the_rest() {
        let dir = scratch("recovery-sweep");
        let program = dir.join("programa");
        std::fs::create_dir_all(&program).unwrap();
        let temp_root = dir.join("temp");
        std::fs::create_dir_all(&temp_root).unwrap();

        let parked = program.join(format!("{}12345", crate::PARKED_DIR_PREFIX));
        write_file(&parked.join("viejo"), "v1");
        let staging = dir.join(format!("{}999", crate::STAGING_DIR_PREFIX));
        write_file(&staging.join("descargado"), "bundle");
        let dead_temp = temp_root.join("avi-deferred-4294967000-1780000000000.ps1");
        write_file(&dead_temp, "borrador");
        let live_temp = temp_root.join(format!("avi-deferred-{}.ps1", std::process::id()));
        write_file(&live_temp, "en uso");
        let our = temp_root.join("lifecycle-test-de-otra-prueba.txt");
        write_file(&our, "ajeno");
        let without_pid = temp_root.join("avi_clone_x_1.qvoice");
        write_file(&without_pid, "audio");

        let outcome = recover(Roots {
            program_dir: &program,
            temp_root: &temp_root,
            in_use: None,
        })
        .unwrap();

        assert!(
            !outcome.rolled_back && !outcome.committed,
            "no había diario"
        );
        assert_eq!(outcome.removed_parked, vec![parked.clone()]);
        assert_eq!(outcome.removed_stagings, vec![staging.clone()]);
        assert_eq!(
            outcome.removed_temporaries,
            vec![dead_temp.clone(), without_pid.clone()],
            "un temporal sin PID está tan huérfano como uno con PID muerto"
        );
        assert!(!parked.exists() && !staging.exists() && !dead_temp.exists());
        assert!(
            !without_pid.exists(),
            "el temporal sin PID también se barre, porque la regla pide los huérfanos"
        );
        assert!(
            live_temp.exists(),
            "el temporal de este proceso se conserva"
        );
        assert!(our.exists(), "lo ajeno al producto no se toca");
        assert_eq!(
            outcome.kept,
            vec![live_temp],
            "solo se queda el que pertenece a un proceso vivo, y no es un fallo"
        );
        assert!(!outcome.is_clean());

        std::fs::remove_dir_all(&dir).ok();
    }

    /// El informe del barrido y el barrido coinciden, y el informe no toca nada:
    /// es lo que permite que `doctor` informe y que `cleanup --dry-run` anuncie sin
    /// tener dos implementaciones que puedan divergir.
    #[test]
    fn preview_matches_the_sweep_and_touches_nothing() {
        let dir = scratch("recovery-preview");
        let program = dir.join("programa");
        let temp_root = dir.join("temp");
        std::fs::create_dir_all(&temp_root).unwrap();
        let parked = program.join(format!("{}1", crate::PARKED_DIR_PREFIX));
        write_file(&parked.join("viejo"), "v1");
        let staging = dir.join(format!("{}2", crate::STAGING_DIR_PREFIX));
        write_file(&staging.join("descargado"), "bundle");
        let mine = dir.join(format!("{}3", crate::STAGING_DIR_PREFIX));
        let orphan = temp_root.join("avi-huerfano.tmp");
        write_file(&orphan, "x");
        let alive = temp_root.join(format!("avi-vivo-{}.tmp", std::process::id()));
        write_file(&alive, "x");

        let roots = Roots {
            program_dir: &program,
            temp_root: &temp_root,
            in_use: Some(&mine),
        };
        let before = crate::test_support::snapshot(&dir);

        let findings = preview(roots);
        assert_eq!(findings.parked, vec![parked.clone()]);
        assert_eq!(
            findings.stagings,
            vec![staging.clone()],
            "el staging en uso no se anuncia ni se barre"
        );
        assert_eq!(findings.temporaries, vec![orphan.clone()]);
        assert_eq!(findings.temporaries_kept, vec![alive.clone()]);
        assert_eq!(
            crate::test_support::snapshot(&dir),
            before,
            "el informe no modifica el disco"
        );

        let outcome = recover(roots).unwrap();
        assert_eq!(outcome.removed_parked, findings.parked);
        assert_eq!(outcome.removed_stagings, findings.stagings);
        assert_eq!(outcome.removed_temporaries, findings.temporaries);
        assert!(alive.exists(), "lo que el informe conservaba se conserva");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// El staging que la operación en curso va a instalar **no** es un staging
    /// huérfano, aunque esté vacío: barrerse a sí mismo entre el paso 1 y el paso 2 de
    /// la instalación dejaría a `self install` instalando un bundle vacío.
    ///
    /// El resto del barrido no cambia: el hermano huérfano sí se va.
    #[test]
    fn staging_in_use_is_not_swept() {
        let dir = scratch("recovery-in-use");
        let program = dir.join("programa");
        let temp_root = dir.join("temp");
        std::fs::create_dir_all(&temp_root).unwrap();
        let mine = dir.join(format!("{}yo", crate::STAGING_DIR_PREFIX));
        let orphan = dir.join(format!("{}otro", crate::STAGING_DIR_PREFIX));
        write_file(&mine.join("descargado"), "bundle");
        write_file(&orphan.join("descargado"), "bundle");

        let outcome = recover(Roots {
            program_dir: &program,
            temp_root: &temp_root,
            in_use: Some(&mine),
        })
        .unwrap();

        assert!(
            mine.exists(),
            "el staging del que se instala sobrevive a su propia recuperación"
        );
        assert!(!orphan.exists(), "el hermano huérfano sí se barre");
        assert_eq!(outcome.removed_stagings, vec![orphan]);
        assert!(
            !outcome.kept.contains(&mine),
            "y el propio tampoco se reporta como pendiente"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Un diario confirmado se completa como commit y uno sin confirmar se
    /// revierte: el estado del diario es lo que decide, y ambos dejan cero
    /// residuo. Es el estado en el que una operación que murió a mitad deja el
    /// directorio de programa.
    #[test]
    fn journal_state_decides_commit_or_rollback() {
        // Confirmado: se completa el commit.
        let dir = scratch("recovery-commit");
        let program = dir.join("programa");
        let temp_root = dir.join("temp");
        std::fs::create_dir_all(&temp_root).unwrap();
        let parked = program.join(format!("{}77", crate::PARKED_DIR_PREFIX));
        write_file(&parked.join("anterior"), "v1");
        write_file(&program.join("nuevo"), "v2");
        write_journal(
            &program,
            &Journal {
                schema_version: transaction::JOURNAL_SCHEMA_VERSION,
                txid: "77".to_string(),
                state: JournalState::Committed,
                parked_dir: Some(parked.clone()),
                placed: vec!["nuevo".to_string()],
                source_dir: None,
            },
        );

        let outcome = recover(Roots {
            program_dir: &program,
            temp_root: &temp_root,
            in_use: None,
        })
        .unwrap();
        assert!(outcome.committed, "el diario confirmado se completa");
        assert!(!outcome.rolled_back);
        assert!(!parked.exists(), "el aparcado se borra en el commit");
        assert!(program.join("nuevo").is_file(), "lo colocado se conserva");
        assert!(
            transaction::read_journal(&program).unwrap().is_none(),
            "el diario desaparece"
        );
        assert!(outcome.is_clean());
        std::fs::remove_dir_all(&dir).ok();

        // Sin confirmar: se revierte y la versión anterior vuelve a ser la
        // operativa, que es lo que promete la recuperación.
        let dir = scratch("recovery-rollback");
        let program = dir.join("programa");
        let temp_root = dir.join("temp");
        let staging = dir.join("staging");
        std::fs::create_dir_all(&temp_root).unwrap();
        let parked = program.join(format!("{}88", crate::PARKED_DIR_PREFIX));
        write_file(&parked.join("anterior"), "v1");
        write_file(&parked.join("motor"), "motor v1");
        // Mitad colocada: el ejecutable nuevo ya está en el programa, el motor
        // sigue en el origen.
        write_file(&program.join("ejecutable"), "v2");
        write_file(&staging.join("motor"), "motor v2");
        write_journal(
            &program,
            &Journal {
                schema_version: transaction::JOURNAL_SCHEMA_VERSION,
                txid: "88".to_string(),
                state: JournalState::Placed,
                parked_dir: Some(parked.clone()),
                placed: vec!["ejecutable".to_string(), "motor".to_string()],
                source_dir: Some(staging.clone()),
            },
        );

        let outcome = recover(Roots {
            program_dir: &program,
            temp_root: &temp_root,
            in_use: None,
        })
        .unwrap();
        assert!(outcome.rolled_back, "el diario sin confirmar se revierte");
        assert!(!outcome.committed);
        assert_eq!(
            std::fs::read_to_string(program.join("anterior")).unwrap(),
            "v1",
            "la versión anterior vuelve al sitio"
        );
        assert_eq!(
            std::fs::read_to_string(program.join("motor")).unwrap(),
            "motor v1",
            "su motor también, y por fusión de árboles"
        );
        assert!(
            !program.join("ejecutable").exists(),
            "lo colocado se retira del directorio de programa"
        );
        assert_eq!(
            std::fs::read_to_string(staging.join("ejecutable")).unwrap(),
            "v2",
            "y vuelve al origen, que es de donde la colocación lo movió"
        );
        assert!(!parked.exists(), "el aparcado desaparece tras restaurarlo");
        assert!(transaction::read_journal(&program).unwrap().is_none());
        assert!(outcome.is_clean());
        std::fs::remove_dir_all(&dir).ok();
    }
}
