// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

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
//! 2. **Barrido por identidad.** Cada residuo lleva en el nombre el token de su
//!    propietario, y el barrido lo clasifica con la identidad exacta del proceso:
//!    `Dead` (no existe, o su PID lo ocupa otro proceso) se barre, `Ours` (sigue
//!    vivo) se conserva sin más, y `Unknown` (no se puede observar) se conserva y
//!    se informa. Los nombres sin token no son de este modelo y no se tocan. Las
//!    raíces son fijas:
//!    - el scratch (`<raíz de temporales>/<token>/`);
//!    - los temporales con token y los stagings de voz de los directorios de
//!      datos, de enlaces, de voces y de habla;
//!    - los stagings de instalación hermanos del directorio de programa y los
//!      aparcados `.old-<token>-<único>` de dentro;
//!    - los `<destino>.removal-failed.log` del borrado diferido, junto al
//!      directorio de programa y a sus aparcados: se retiran cuando el destino ya
//!      no existe, y mientras exista se informan como fallo pendiente.
//!
//! **Se llama con el bloqueo tomado** (paso 1 de la instalación): si no, el barrido
//! podría llevarse por delante el staging que otra operación está usando. Por eso este
//! módulo no toma el bloqueo por su cuenta.
//!
//! Lo que no se puede borrar **no es un fallo**: son archivos en uso, y el
//! borrado diferido los recoge después. El resultado enumera lo que se
//! quitó y lo que se quedó, para que `doctor` pueda informarlo, que es lo que
//! la operación exige explícitamente.

use crate::transaction;
use anyhow::Result;
use avi_shared::staging_names::{
    install_staging, parked, temp_file, voice_staging, REMOVAL_FAILED_SUFFIX,
};
use avi_shared::{OwnerState, OwnerToken};
use std::path::{Path, PathBuf};

/// Raíces sobre las que actúa la recuperación. Todas son parámetros —como la
/// clave de registro en la integración del `PATH`— para que las pruebas no barran
/// las carpetas de la máquina.
#[derive(Debug, Clone, Copy)]
pub struct Roots<'a> {
    /// Directorio de programa, donde viven el diario y los aparcados; su padre
    /// es donde viven los stagings de instalación.
    pub program_dir: &'a Path,
    /// Raíz de los scratch por proceso, un directorio por propietario.
    pub scratch_root: &'a Path,
    /// Raíz de datos: pidfile, fichero ready, y de ella cuelgan los almacenes
    /// `voices` y `speech`, cuyas voces contienen temporales con token.
    pub data_dir: &'a Path,
    /// Directorio del enlace del comando.
    pub bin_dir: &'a Path,
}

/// Lo que el barrido **tocaría ahora mismo**, calculado sin tocar nada.
///
/// Es la forma de informe: `doctor` en modo informe y el plan de
/// `cleanup --dry-run` muestran esta lista, y el barrido real usa la misma
/// decisión. Derivar las dos de funciones distintas permitiría que la
/// simulación anunciara un borrado que la ejecución no hace.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SweepPreview {
    /// Aparcados `.old-<token>-<único>` de dueño muerto.
    pub parked: Vec<PathBuf>,
    /// Stagings de instalación hermanos del directorio de programa, de dueño
    /// muerto.
    pub stagings: Vec<PathBuf>,
    /// Scratch de dueño muerto.
    pub scratch: Vec<PathBuf>,
    /// Temporales con token y stagings de voz de dueño muerto.
    pub temporaries: Vec<PathBuf>,
    /// Registros `*.removal-failed.log` cuyo destino ya no existe.
    pub failure_logs: Vec<PathBuf>,
    /// Residuos de cualquier familia cuyo dueño no se puede observar: se
    /// conservan y se informan. No es un fallo ni un residuo que se pueda
    /// forzar.
    pub unknown_owner: Vec<PathBuf>,
    /// Registros `*.removal-failed.log` cuyo destino sigue existiendo: el borrado
    /// diferido no pudo retirarlo y sigue pendiente.
    pub pending_failures: Vec<PathBuf>,
    /// Directorios que existen y no se pudieron listar, con su causa. El barrido no
    /// puede afirmar que no hay nada que recoger en ellos; un directorio inexistente
    /// no entra aquí, porque no hay nada que barrer.
    pub unreadable: Vec<(PathBuf, String)>,
}

impl SweepPreview {
    /// Todo lo que el barrido tocaría, en el orden en que lo recorrería.
    pub fn all(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        out.extend(self.parked.iter().cloned());
        out.extend(self.stagings.iter().cloned());
        out.extend(self.scratch.iter().cloned());
        out.extend(self.temporaries.iter().cloned());
        out.extend(self.failure_logs.iter().cloned());
        out
    }

    /// Lo que el barrido conserva y debe informarse: dueño no observable y
    /// fallos de borrado diferido aún pendientes.
    pub fn undecided(&self) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = self
            .unknown_owner
            .iter()
            .chain(self.pending_failures.iter())
            .cloned()
            .collect();
        out.sort();
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
    /// Stagings de instalación huérfanos borrados.
    pub removed_stagings: Vec<PathBuf>,
    /// Scratch de dueño muerto borrados.
    pub removed_scratch: Vec<PathBuf>,
    /// Temporales con token y stagings de voz huérfanos borrados.
    pub removed_temporaries: Vec<PathBuf>,
    /// Registros de borrado fallido retirados porque su destino ya no existe.
    pub removed_failure_logs: Vec<PathBuf>,
    /// Lo que queda pendiente: lo que no se pudo borrar por estar en uso, los
    /// residuos de dueño no observable y los fallos de borrado diferido cuyo
    /// destino sigue existiendo. No es un fallo.
    pub kept: Vec<PathBuf>,
    /// Directorios que existen y no se pudieron listar para el barrido. También están
    /// en `kept`: lo que contienen queda sin recoger.
    pub unreadable: Vec<PathBuf>,
}

impl RecoveryOutcome {
    /// `true` si no queda nada pendiente de una operación anterior.
    pub fn is_clean(&self) -> bool {
        self.kept.is_empty()
    }

    /// Todo lo que el barrido borró, en el orden del informe.
    pub fn removed(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        out.extend(self.removed_parked.iter().cloned());
        out.extend(self.removed_stagings.iter().cloned());
        out.extend(self.removed_scratch.iter().cloned());
        out.extend(self.removed_temporaries.iter().cloned());
        out.extend(self.removed_failure_logs.iter().cloned());
        out
    }
}

/// Completa la transacción pendiente y barre lo huérfano, clasificando a los
/// propietarios observando el sistema.
pub fn recover(roots: Roots<'_>) -> Result<RecoveryOutcome> {
    recover_with(roots, avi_process::classify_owner)
}

/// Igual que [`recover`] con la clasificación de propietarios inyectada.
pub fn recover_with(
    roots: Roots<'_>,
    classify: impl Fn(&OwnerToken) -> OwnerState,
) -> Result<RecoveryOutcome> {
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

    // 2. Barrido, con la decisión del informe: una sola clasificación para el
    //    plan y para el barrido.
    let preview = preview_with(roots, classify);
    sweep(
        &preview.parked,
        &mut outcome.removed_parked,
        &mut outcome.kept,
    );
    sweep(
        &preview.stagings,
        &mut outcome.removed_stagings,
        &mut outcome.kept,
    );
    sweep(
        &preview.scratch,
        &mut outcome.removed_scratch,
        &mut outcome.kept,
    );
    sweep(
        &preview.temporaries,
        &mut outcome.removed_temporaries,
        &mut outcome.kept,
    );
    sweep(
        &preview.failure_logs,
        &mut outcome.removed_failure_logs,
        &mut outcome.kept,
    );
    // Lo conservado a propósito queda pendiente y a la vista.
    outcome.kept.extend(preview.undecided());
    // Lo que no se pudo listar queda pendiente y a la vista: ni se da por limpio ni se
    // calla.
    for (dir, cause) in &preview.unreadable {
        eprintln!(
            "Aviso: no se pudo listar {} para el barrido: {cause}",
            dir.display()
        );
        if !outcome.kept.contains(dir) {
            outcome.kept.push(dir.clone());
        }
        if !outcome.unreadable.contains(dir) {
            outcome.unreadable.push(dir.clone());
        }
    }

    Ok(outcome)
}

/// Qué haría el barrido sin tocar nada. `doctor` lo usa como informe
/// en modo informe y `cleanup --dry-run` lo anuncia; el barrido real usa
/// la misma decisión. Derivar el plan y la ejecución de funciones distintas permitiría
/// que la simulación anunciara un borrado que la ejecución no hace.
pub fn preview(roots: Roots<'_>) -> SweepPreview {
    preview_with(roots, avi_process::classify_owner)
}

/// Igual que [`preview`] con la clasificación de propietarios inyectada. Es la
/// **única** función de decisión del barrido.
pub fn preview_with(
    roots: Roots<'_>,
    classify: impl Fn(&OwnerToken) -> OwnerState,
) -> SweepPreview {
    let mut found = SweepPreview::default();
    let parent = roots.program_dir.parent().unwrap_or(Path::new(""));

    // Aparcados de dentro del programa.
    for entry in list_entries(roots.program_dir, &mut found.unreadable) {
        let name = entry.file_name().to_string_lossy().to_string();
        if let Some(owner) = family_name(&name, parked::owner_of) {
            route(
                classify(&owner),
                entry.path(),
                &mut found.parked,
                &mut found.unknown_owner,
            );
        }
    }

    // Stagings de instalación, hermanos del programa.
    for entry in list_entries(parent, &mut found.unreadable) {
        let name = entry.file_name().to_string_lossy().to_string();
        if let Some(owner) = family_name(&name, install_staging::owner_of) {
            route(
                classify(&owner),
                entry.path(),
                &mut found.stagings,
                &mut found.unknown_owner,
            );
        }
    }

    // Scratch: un directorio por propietario; lo que no es un token no se toca.
    for entry in list_entries(roots.scratch_root, &mut found.unreadable) {
        let Some(owner) = entry.file_name().to_str().and_then(OwnerToken::parse) else {
            continue;
        };
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        route(
            classify(&owner),
            entry.path(),
            &mut found.scratch,
            &mut found.unknown_owner,
        );
    }

    // Temporales con token y stagings de voz, en los directorios donde se crean.
    for dir in family_dirs(roots, &mut found.unreadable) {
        for entry in list_entries(&dir, &mut found.unreadable) {
            let name = entry.file_name().to_string_lossy().to_string();
            let owner = family_name(&name, temp_file::owner_of)
                .or_else(|| family_name(&name, voice_staging::owner_of));
            if let Some(owner) = owner {
                route(
                    classify(&owner),
                    entry.path(),
                    &mut found.temporaries,
                    &mut found.unknown_owner,
                );
            }
        }
    }

    // El programa y sus aparcados dejan el registro en estos directorios.
    for dir in [roots.program_dir, parent] {
        for entry in list_entries(dir, &mut found.unreadable) {
            let name = entry.file_name().to_string_lossy().to_string();
            let Some(destination) = name.strip_suffix(REMOVAL_FAILED_SUFFIX) else {
                continue;
            };
            if destination.is_empty() {
                continue;
            }
            if exists(&dir.join(destination)) {
                found.pending_failures.push(entry.path());
            } else {
                found.failure_logs.push(entry.path());
            }
        }
    }

    // El borrado diferido puede apuntar al contenedor de caché o al contenedor
    // raíz cuando el programa reside dentro de ellos. Solo se inspeccionan los
    // registros hermanos de esos destinos conocidos, no los ajenos de su padre.
    if let Some(cache) = roots.scratch_root.parent() {
        let mut containers = vec![cache];
        if let (Some(data_parent), Some(cache_parent)) = (roots.data_dir.parent(), cache.parent()) {
            if cfg!(windows)
                && roots
                    .data_dir
                    .file_name()
                    .is_some_and(|name| name == "data")
                && cache.file_name().is_some_and(|name| name == "cache")
                && data_parent == cache_parent
                && cache_parent
                    .file_name()
                    .is_some_and(|name| name == crate::APP_NAME)
            {
                containers.push(cache_parent);
            }
        }
        for destination in containers {
            let Some(name) = destination.file_name() else {
                continue;
            };
            let log = destination
                .with_file_name(format!("{}{REMOVAL_FAILED_SUFFIX}", name.to_string_lossy()));
            if !exists(&log) {
                continue;
            }
            if exists(destination) {
                found.pending_failures.push(log);
            } else {
                found.failure_logs.push(log);
            }
        }
    }

    for list in [
        &mut found.parked,
        &mut found.stagings,
        &mut found.scratch,
        &mut found.temporaries,
        &mut found.failure_logs,
        &mut found.unknown_owner,
        &mut found.pending_failures,
    ] {
        list.sort();
        list.dedup();
    }
    found
}

/// Directorios donde viven temporales con token y stagings de voz: datos,
/// enlaces, y cada voz de los almacenes `voices` y `speech`.
fn family_dirs(roots: Roots<'_>, unreadable: &mut Vec<(PathBuf, String)>) -> Vec<PathBuf> {
    let mut dirs = vec![roots.data_dir.to_path_buf(), roots.bin_dir.to_path_buf()];
    for store in ["voices", "speech"] {
        for entry in list_entries(&roots.data_dir.join(store), unreadable) {
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                dirs.push(entry.path());
            }
        }
    }
    dirs
}

/// Propietario de `name` según el `parse` de su familia, salvo que `name` sea el
/// registro de un borrado fallido: ese nombre cuelga del destino y no es un
/// residuo de ninguna familia.
fn family_name(name: &str, parse: fn(&str) -> Option<OwnerToken>) -> Option<OwnerToken> {
    if name.ends_with(REMOVAL_FAILED_SUFFIX) {
        return None;
    }
    parse(name)
}

/// Reparte `path` según el estado de su dueño: `Dead` va a `dead`, `Unknown` a
/// `unknown`, y `Ours` no se anota porque se conserva sin informe.
fn route(state: OwnerState, path: PathBuf, dead: &mut Vec<PathBuf>, unknown: &mut Vec<PathBuf>) {
    match state {
        OwnerState::Dead => dead.push(path),
        OwnerState::Unknown => unknown.push(path),
        OwnerState::Ours => {}
    }
}

/// `true` si `path` existe como entrada, sea o no un enlace roto. Un fallo
/// distinto de la ausencia cuenta como existente: no se retira lo que no se
/// puede comprobar.
fn exists(path: &Path) -> bool {
    match std::fs::symlink_metadata(path) {
        Ok(_) => true,
        Err(e) => e.kind() != std::io::ErrorKind::NotFound,
    }
}

/// Entradas de `dir`. Un directorio inexistente no tiene nada que barrer y da una lista
/// vacía; cualquier otro fallo de listado, del directorio o de una entrada, queda en
/// `unreadable` con su causa.
fn list_entries(dir: &Path, unreadable: &mut Vec<(PathBuf, String)>) -> Vec<std::fs::DirEntry> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            unreadable.push((dir.to_path_buf(), e.to_string()));
            return Vec::new();
        }
    };
    let mut out = Vec::new();
    for entry in entries {
        match entry {
            Ok(entry) => out.push(entry),
            Err(e) => unreadable.push((dir.to_path_buf(), e.to_string())),
        }
    }
    out
}

/// Borra cada ruta de `paths`: las borradas van a `removed` y las que el SO no
/// deja borrar (en Windows, un archivo abierto por otro proceso) a `kept`, para el
/// borrado diferido. Un enlace se borra a sí mismo, no su destino.
fn sweep(paths: &[PathBuf], removed: &mut Vec<PathBuf>, kept: &mut Vec<PathBuf>) {
    for path in paths {
        let is_dir = std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir());
        let result = if is_dir {
            std::fs::remove_dir_all(path)
        } else {
            std::fs::remove_file(path)
        };
        match result {
            Ok(()) => removed.push(path.clone()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => removed.push(path.clone()),
            Err(_) => kept.push(path.clone()),
        }
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

    /// Token de prueba: el inicio decide el estado que devuelve [`classify_by_start`].
    fn token(pid: u32, start: &str) -> OwnerToken {
        OwnerToken::new(pid, start).expect("token válido")
    }

    /// Clasificación inyectada: el inicio `ours` está vivo, `unknown` no se puede
    /// observar y cualquier otro es un dueño muerto (incluido un PID reutilizado).
    fn classify_by_start(owner: &OwnerToken) -> OwnerState {
        match owner.start() {
            "ours" => OwnerState::Ours,
            "unknown" => OwnerState::Unknown,
            _ => OwnerState::Dead,
        }
    }

    /// Raíces de prueba bajo `dir`: programa, scratch, datos y enlaces.
    struct Sandbox {
        program: PathBuf,
        scratch_root: PathBuf,
        data: PathBuf,
        bin: PathBuf,
    }

    impl Sandbox {
        fn new(dir: &Path) -> Self {
            let sandbox = Self {
                program: dir.join("opt").join("program"),
                scratch_root: dir.join("cache").join("tmp"),
                data: dir.join("data"),
                bin: dir.join("bin"),
            };
            std::fs::create_dir_all(&sandbox.program).unwrap();
            sandbox
        }

        fn roots(&self) -> Roots<'_> {
            Roots {
                program_dir: &self.program,
                scratch_root: &self.scratch_root,
                data_dir: &self.data,
                bin_dir: &self.bin,
            }
        }

        /// Hermano del programa, donde viven los stagings de instalación.
        fn sibling(&self, name: &str) -> PathBuf {
            self.program.parent().unwrap().join(name)
        }
    }

    /// Un PID reutilizado (mismo PID, otro inicio) está muerto y se barre; el
    /// dueño vivo con el mismo PID no se toca.
    #[test]
    fn reused_pid_is_swept_and_live_owner_is_kept() {
        let dir = scratch("recovery-reused-pid");
        let sandbox = Sandbox::new(&dir);
        let reused = sandbox
            .data
            .join(temp_file::name("daemon.pid", &token(100, "old")));
        let live = sandbox
            .data
            .join(temp_file::name("daemon.ready", &token(100, "ours")));
        write_file(&reused, "x");
        write_file(&live, "x");

        let outcome = recover_with(sandbox.roots(), classify_by_start).unwrap();

        assert_eq!(outcome.removed_temporaries, vec![reused.clone()]);
        assert!(!reused.exists(), "el PID reutilizado es un dueño muerto");
        assert!(live.exists(), "el dueño vivo conserva su temporal");
        assert!(outcome.is_clean(), "conservar lo vivo no es un pendiente");
    }

    /// El scratch propio se conserva; el de un dueño muerto se barre; lo que no
    /// es un token no se toca.
    #[test]
    fn own_scratch_is_kept_and_dead_scratch_is_swept() {
        let dir = scratch("recovery-scratch");
        let sandbox = Sandbox::new(&dir);
        let own = sandbox.scratch_root.join(token(7, "ours").format());
        let dead = sandbox.scratch_root.join(token(8, "gone").format());
        let foreign = sandbox.scratch_root.join("not-a-token");
        write_file(&own.join("clip.wav"), "x");
        write_file(&dead.join("clip.wav"), "x");
        write_file(&foreign.join("clip.wav"), "x");

        let outcome = recover_with(sandbox.roots(), classify_by_start).unwrap();

        assert_eq!(outcome.removed_scratch, vec![dead.clone()]);
        assert!(own.exists() && foreign.exists() && !dead.exists());
        assert!(outcome.is_clean());
    }

    /// Un dueño no observable se conserva, se informa y deja la recuperación
    /// sin limpiar.
    #[test]
    fn unknown_owner_is_kept_and_reported() {
        let dir = scratch("recovery-unknown");
        let sandbox = Sandbox::new(&dir);
        let unknown = sandbox.scratch_root.join(token(9, "unknown").format());
        write_file(&unknown.join("clip.wav"), "x");

        let findings = preview_with(sandbox.roots(), classify_by_start);
        assert_eq!(findings.unknown_owner, vec![unknown.clone()]);
        assert!(findings.all().is_empty(), "no se anuncia como borrado");

        let outcome = recover_with(sandbox.roots(), classify_by_start).unwrap();
        assert!(unknown.exists());
        assert_eq!(outcome.kept, vec![unknown]);
        assert!(!outcome.is_clean());
    }

    /// Los stagings de instalación y los aparcados de un dueño muerto se barren;
    /// los de un dueño vivo se conservan, y un nombre sin token (el del
    /// instalador de arranque) no se toca.
    #[test]
    fn dead_owner_stagings_and_parked_are_swept() {
        let dir = scratch("recovery-stagings");
        let sandbox = Sandbox::new(&dir);
        let dead_staging =
            sandbox.sibling(&install_staging::name("update-1.0.0", &token(11, "gone")));
        let live_staging =
            sandbox.sibling(&install_staging::name("update-1.0.0", &token(12, "ours")));
        let bootstrap_staging = sandbox.sibling(".ai-voice-interconnector-staging-aB3dE9");
        let dead_parked = sandbox.program.join(parked::name(&token(13, "gone"), 5));
        let live_parked = sandbox.program.join(parked::name(&token(14, "ours"), 5));
        for path in [
            &dead_staging,
            &live_staging,
            &bootstrap_staging,
            &dead_parked,
            &live_parked,
        ] {
            write_file(&path.join("f"), "x");
        }

        let outcome = recover_with(sandbox.roots(), classify_by_start).unwrap();

        assert_eq!(outcome.removed_stagings, vec![dead_staging.clone()]);
        assert_eq!(outcome.removed_parked, vec![dead_parked.clone()]);
        assert!(!dead_staging.exists() && !dead_parked.exists());
        assert!(live_staging.exists() && live_parked.exists());
        assert!(bootstrap_staging.exists(), "sin token no es de este modelo");
        assert!(outcome.is_clean());
    }

    /// Los temporales con token y los stagings de voz se barren en los
    /// directorios de datos, enlaces, voces y habla.
    #[test]
    fn family_directories_are_swept() {
        let dir = scratch("recovery-families");
        let sandbox = Sandbox::new(&dir);
        let dead = token(21, "gone");
        let in_data = sandbox.data.join(temp_file::name("daemon.pid", &dead));
        let in_bin = sandbox
            .bin
            .join(temp_file::name("ai-voice-interconnector", &dead));
        let in_voice = sandbox
            .data
            .join("voices")
            .join("ana")
            .join(temp_file::name("reference.qvoice", &dead));
        let in_speech = sandbox
            .data
            .join("speech")
            .join("ana")
            .join(temp_file::name("hello.wav", &dead));
        let voice_staging_dir = sandbox.data.join(voice_staging::name("ana", &dead));
        let live = sandbox
            .data
            .join("voices")
            .join("ana")
            .join(temp_file::name("other.qvoice", &token(22, "ours")));
        let plain = sandbox
            .data
            .join("voices")
            .join("ana")
            .join("reference.qvoice");
        for path in [&in_data, &in_bin, &in_voice, &in_speech, &live, &plain] {
            write_file(path, "x");
        }
        write_file(&voice_staging_dir.join("reference.qvoice"), "x");

        let outcome = recover_with(sandbox.roots(), classify_by_start).unwrap();

        let mut expected = vec![
            in_data.clone(),
            in_bin.clone(),
            in_voice.clone(),
            in_speech.clone(),
            voice_staging_dir.clone(),
        ];
        expected.sort();
        let mut removed = outcome.removed_temporaries.clone();
        removed.sort();
        assert_eq!(removed, expected);
        assert!(live.exists() && plain.exists());
        assert!(outcome.is_clean());
    }

    /// El registro de borrado fallido se retira solo cuando su destino ya no
    /// existe; mientras exista, se conserva y se informa como pendiente.
    #[test]
    fn failure_log_is_removed_only_when_destination_is_gone() {
        let dir = scratch("recovery-failure-logs");
        let sandbox = Sandbox::new(&dir);
        let gone_log = sandbox.sibling("gone.removal-failed.log");
        let present = sandbox.sibling("present");
        let present_log = sandbox.sibling("present.removal-failed.log");
        write_file(&gone_log, "no se pudo retirar");
        write_file(&present.join("f"), "x");
        write_file(&present_log, "no se pudo retirar");

        let findings = preview_with(sandbox.roots(), classify_by_start);
        assert_eq!(findings.failure_logs, vec![gone_log.clone()]);
        assert_eq!(findings.pending_failures, vec![present_log.clone()]);

        let outcome = recover_with(sandbox.roots(), classify_by_start).unwrap();
        assert_eq!(outcome.removed_failure_logs, vec![gone_log.clone()]);
        assert!(!gone_log.exists());
        assert!(present.exists() && present_log.exists());
        assert_eq!(outcome.kept, vec![present_log]);
        assert!(!outcome.is_clean());
    }

    /// El registro de borrado fallido de un aparcado o de un staging no se toma
    /// por el aparcado ni por el staging.
    #[test]
    fn failure_log_of_a_family_name_is_not_taken_for_the_family() {
        let dir = scratch("recovery-log-names");
        let sandbox = Sandbox::new(&dir);
        let parked_name = parked::name(&token(31, "ours"), 7);
        let parked_log = sandbox
            .program
            .join(format!("{parked_name}{REMOVAL_FAILED_SUFFIX}"));
        let staging_name = install_staging::name("update-1.0.0", &token(32, "ours"));
        let staging_log = sandbox.sibling(&format!("{staging_name}{REMOVAL_FAILED_SUFFIX}"));
        write_file(&parked_log, "x");
        write_file(&staging_log, "x");

        let findings = preview_with(sandbox.roots(), |_| OwnerState::Dead);

        assert!(findings.parked.is_empty() && findings.stagings.is_empty());
        let mut logs = findings.failure_logs.clone();
        logs.sort();
        let mut expected = vec![parked_log, staging_log];
        expected.sort();
        assert_eq!(logs, expected, "su destino ya no existe");
    }

    /// Los registros de los contenedores propiedad de la aplicación se retiran
    /// si el destino ya no existe y se informan mientras el borrado siga pendiente.
    #[test]
    fn container_failure_logs_follow_destination_state() {
        let dir = scratch("recovery-container-logs");
        let sandbox = Sandbox::new(&dir);
        let cache = sandbox.scratch_root.parent().unwrap();
        let cache_log = cache.with_file_name("cache.removal-failed.log");
        write_file(&cache_log, "no se pudo retirar");

        let findings = preview_with(sandbox.roots(), classify_by_start);
        assert_eq!(findings.failure_logs, vec![cache_log.clone()]);

        std::fs::create_dir_all(cache).unwrap();
        let findings = preview_with(sandbox.roots(), classify_by_start);
        assert_eq!(findings.pending_failures, vec![cache_log.clone()]);

        std::fs::remove_dir(cache).unwrap();
        let outcome = recover_with(sandbox.roots(), classify_by_start).unwrap();
        assert_eq!(outcome.removed_failure_logs, vec![cache_log.clone()]);
        assert!(!cache_log.exists());
    }

    #[cfg(windows)]
    #[test]
    fn app_container_failure_log_is_found_without_touching_foreign_logs() {
        let dir = scratch("recovery-app-container-logs");
        let app = dir.join(crate::APP_NAME);
        let program = app.join("bin").join("program");
        let data = app.join("data");
        let scratch_root = app.join("cache").join("tmp");
        let bin = dir.join("bin");
        let app_log = dir.join(format!("{}{REMOVAL_FAILED_SUFFIX}", crate::APP_NAME));
        let foreign_log = dir.join("foreign.removal-failed.log");
        write_file(&app_log, "falló");
        write_file(&foreign_log, "ajeno");
        let roots = Roots {
            program_dir: &program,
            scratch_root: &scratch_root,
            data_dir: &data,
            bin_dir: &bin,
        };

        let findings = preview_with(roots, classify_by_start);
        assert_eq!(findings.failure_logs, vec![app_log.clone()]);
        assert!(foreign_log.exists());
        recover_with(roots, classify_by_start).unwrap();
        assert!(!app_log.exists());
        assert!(foreign_log.exists());
    }

    /// Un sandbox ajeno al scratch (aunque su nombre sea un token de dueño
    /// muerto, pero fuera de la raíz del scratch) no se toca.
    #[test]
    fn directory_outside_the_scratch_is_not_touched() {
        let dir = scratch("recovery-foreign-sandbox");
        let sandbox = Sandbox::new(&dir);
        let foreign = dir.join("elsewhere").join(token(41, "gone").format());
        let renamed = sandbox
            .scratch_root
            .join(format!("{}-sandbox", token(42, "gone").format()));
        write_file(&foreign.join("f"), "x");
        write_file(&renamed.join("f"), "x");

        let outcome = recover_with(sandbox.roots(), classify_by_start).unwrap();

        assert!(foreign.exists(), "fuera del scratch no se barre");
        assert!(renamed.exists(), "un nombre que no es un token no se barre");
        assert!(outcome.removed().is_empty());
        assert!(outcome.is_clean());
    }

    /// Un directorio a barrer que existe pero no se puede listar (aquí, un fichero)
    /// queda en el informe y en `kept`; que no exista es «nada que barrer».
    #[test]
    fn unlistable_directories_are_reported_and_missing_ones_are_not() {
        let dir = scratch("recovery-unlistable");
        let scratch_file = dir.join("scratch-is-a-file");
        write_file(&scratch_file, "no es un directorio");
        let program = dir.join("missing").join("program");
        let missing = dir.join("missing-data");
        let roots = Roots {
            program_dir: &program,
            scratch_root: &scratch_file,
            data_dir: &missing,
            bin_dir: &missing,
        };

        let findings = preview_with(roots, classify_by_start);
        let unreadable: Vec<&PathBuf> = findings.unreadable.iter().map(|(p, _)| p).collect();
        assert_eq!(
            unreadable,
            vec![&scratch_file],
            "solo el que existe y falla"
        );

        let outcome = recover_with(roots, classify_by_start).unwrap();
        assert_eq!(outcome.kept, vec![scratch_file.clone()]);
        assert_eq!(outcome.unreadable, vec![scratch_file]);
        assert!(!outcome.is_clean());
    }

    /// El informe del barrido y el barrido coinciden, y el informe no toca nada:
    /// es lo que permite que `doctor` informe y que `cleanup --dry-run` anuncie sin
    /// tener dos implementaciones que puedan divergir.
    #[test]
    fn preview_matches_the_sweep_and_touches_nothing() {
        let dir = scratch("recovery-preview");
        let sandbox = Sandbox::new(&dir);
        let parked_dir = sandbox.program.join(parked::name(&token(51, "gone"), 1));
        let staging = sandbox.sibling(&install_staging::name("update-1.0.0", &token(52, "gone")));
        let dead_scratch = sandbox.scratch_root.join(token(53, "gone").format());
        let temp = sandbox
            .data
            .join(temp_file::name("daemon.pid", &token(54, "gone")));
        let log = sandbox.sibling("old.removal-failed.log");
        let alive = sandbox
            .data
            .join(temp_file::name("daemon.ready", &token(55, "ours")));
        write_file(&parked_dir.join("old"), "v1");
        write_file(&staging.join("downloaded"), "bundle");
        write_file(&dead_scratch.join("f"), "x");
        write_file(&temp, "x");
        write_file(&log, "x");
        write_file(&alive, "x");
        let before = crate::test_support::snapshot(&dir);

        let findings = preview_with(sandbox.roots(), classify_by_start);
        assert_eq!(findings.parked, vec![parked_dir.clone()]);
        assert_eq!(findings.stagings, vec![staging.clone()]);
        assert_eq!(findings.scratch, vec![dead_scratch.clone()]);
        assert_eq!(findings.temporaries, vec![temp.clone()]);
        assert_eq!(findings.failure_logs, vec![log.clone()]);
        assert!(findings.unknown_owner.is_empty());
        assert_eq!(
            crate::test_support::snapshot(&dir),
            before,
            "el informe no modifica el disco"
        );

        let outcome = recover_with(sandbox.roots(), classify_by_start).unwrap();
        assert_eq!(outcome.removed(), findings.all());
        assert!(alive.exists(), "lo que el informe conservaba se conserva");
    }

    /// Un diario confirmado se completa como commit y uno sin confirmar se
    /// revierte: el estado del diario es lo que decide, y ambos dejan cero
    /// residuo. Es el estado en el que una operación que murió a mitad deja el
    /// directorio de programa.
    #[test]
    fn journal_state_decides_commit_or_rollback() {
        // Confirmado: se completa el commit.
        let dir = scratch("recovery-commit");
        let sandbox = Sandbox::new(&dir);
        let program = sandbox.program.clone();
        let parked = program.join(parked::name(&token(77, "ours"), 77));
        write_file(&parked.join("previous"), "v1");
        write_file(&program.join("new"), "v2");
        write_journal(
            &program,
            &Journal {
                schema_version: transaction::JOURNAL_SCHEMA_VERSION,
                txid: "77".to_string(),
                state: JournalState::Committed,
                parked_dir: Some(parked.clone()),
                placed: vec!["new".to_string()],
                source_dir: None,
            },
        );

        let outcome = recover_with(sandbox.roots(), classify_by_start).unwrap();
        assert!(outcome.committed, "el diario confirmado se completa");
        assert!(!outcome.rolled_back);
        assert!(!parked.exists(), "el aparcado se borra en el commit");
        assert!(program.join("new").is_file(), "lo colocado se conserva");
        assert!(
            transaction::read_journal(&program).unwrap().is_none(),
            "el diario desaparece"
        );
        assert!(outcome.is_clean());

        // Sin confirmar: se revierte y la versión anterior vuelve a ser la
        // operativa, que es lo que promete la recuperación.
        let dir = scratch("recovery-rollback");
        let sandbox = Sandbox::new(&dir);
        let program = sandbox.program.clone();
        let staging = dir.join("staging");
        let parked = program.join(parked::name(&token(88, "ours"), 88));
        write_file(&parked.join("previous"), "v1");
        write_file(&parked.join("engine"), "motor v1");
        // Mitad colocada: el ejecutable nuevo ya está en el programa, el motor
        // sigue en el origen.
        write_file(&program.join("executable"), "v2");
        write_file(&staging.join("engine"), "motor v2");
        write_journal(
            &program,
            &Journal {
                schema_version: transaction::JOURNAL_SCHEMA_VERSION,
                txid: "88".to_string(),
                state: JournalState::Placed,
                parked_dir: Some(parked.clone()),
                placed: vec!["executable".to_string(), "engine".to_string()],
                source_dir: Some(staging.clone()),
            },
        );

        let outcome = recover_with(sandbox.roots(), classify_by_start).unwrap();
        assert!(outcome.rolled_back, "el diario sin confirmar se revierte");
        assert!(!outcome.committed);
        assert_eq!(
            std::fs::read_to_string(program.join("previous")).unwrap(),
            "v1",
            "la versión anterior vuelve al sitio"
        );
        assert_eq!(
            std::fs::read_to_string(program.join("engine")).unwrap(),
            "motor v1",
            "su motor también, y por fusión de árboles"
        );
        assert!(
            !program.join("executable").exists(),
            "lo colocado se retira del directorio de programa"
        );
        assert_eq!(
            std::fs::read_to_string(staging.join("executable")).unwrap(),
            "v2",
            "y vuelve al origen, que es de donde la colocación lo movió"
        );
        assert!(!parked.exists(), "el aparcado desaparece tras restaurarlo");
        assert!(transaction::read_journal(&program).unwrap().is_none());
        assert!(outcome.is_clean());
    }
}
