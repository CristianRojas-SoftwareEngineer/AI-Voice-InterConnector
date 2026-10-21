// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! `xtask clean`: devuelve la máquina del desarrollador a un estado limpio por
//! capas, para compilar, instalar o usar solo artefactos nuevos:
//!
//! | Capa | Flag | Contenido | Mecanismo |
//! |---|---|---|---|
//! | Repositorio | `--repo` (por defecto) | `target/`, `ort-bundle/`, salidas de `package`, binario y objetos del motor y cobertura | `xtask` |
//! | Aplicación | `--app` | Instalación y estado del usuario | Delegado en `self uninstall --yes` (o `cleanup --all --yes` si el canal es `homebrew`), ejecutado con el binario del repositorio |
//! | Ambas | `--all` | Aplicación y después repositorio | Ídem |
//!
//! Con `--prune`, la capa repo se modera: poda solo `incremental/`, cachés
//! efímeras, targets cruzados ajenos al host, `.pdb` viejos de `deps/` y los
//! sandboxes de prueba huérfanos, y conserva la compilación vigente para que
//! el siguiente build siga siendo incremental en vez de frío.
//!
//! La poda reconoce por nombre los directorios temporales de las pruebas.
//! Los que no llevan PID se retiran tras un día; los sandboxes de ciclo de
//! vida se retiran solo si su PID ya no está vivo.
//!
//! La capa global (`--global`) es explícita y nunca entra en `--all`: borra
//! solo lo que Cargo documenta como regenerable bajo `CARGO_HOME`
//! (`registry/{cache,src,index}` y `git/{db,checkouts}`) y, en Windows, el
//! contenido del `CacheDir` de pacman declarado en el `pacman.conf` de MSYS2.
//! Sola, no limpia el repositorio. Nunca se toca `CARGO_HOME/bin`, rustup, la
//! caché de sccache ni la instalación de MSYS2. El borrado diferido de
//! `target/` puede dejar `target.removal-failed.log`: la capa repo lo retira
//! cuando `target/` ya no existe y lo informa cuando sigue existiendo.
//!
//! Detiene
//! primero los daemons lanzados desde `target/`, nunca borra código fuente
//! versionado y aplica las reglas de confirmación (`--dry-run`, `--yes`
//! obligatorio sin terminal). En Windows, el `xtask.exe` en ejecución se
//! borra con la misma implementación de borrado diferido que el producto
//! (`avi-process`).
//!
//! Las rutas de la capa app viven en `avi-shared` y las ejecuta el binario
//! del repositorio: este módulo no replica ni una (fuente única en `avi-shared`).

use anyhow::{bail, Result};
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

const APP_NAME: &str = "ai-voice-interconnector";

/// Capa a limpiar: `--repo` (por defecto), `--app` o `--all` (aplicación y
/// después repositorio). `None` es el caso de `--global` solo: ni repo ni app.
/// La capa global no es una variante: se pide aparte y `All` nunca la incluye.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    Repo,
    App,
    All,
    None,
}

impl Layer {
    /// Capa pedida por los flags. `--global` no cambia la capa salvo cuando
    /// va solo: entonces no se limpia ni repo ni app.
    pub fn select(repo: bool, app: bool, all: bool, global: bool) -> Self {
        if all || (repo && app) {
            Self::All
        } else if app {
            Self::App
        } else if repo || !global {
            Self::Repo
        } else {
            Self::None
        }
    }

    /// ¿Alcanza la capa del repositorio?
    pub fn wants_repo(self) -> bool {
        matches!(self, Self::Repo | Self::All)
    }

    /// ¿Alcanza la capa de la aplicación?
    pub fn wants_app(self) -> bool {
        matches!(self, Self::App | Self::All)
    }
}

/// Entradas del repo regenerables por el build (relativas a la raíz).
const REPO_ENTRIES: &[&str] = &[
    "target",
    "ort-bundle",
    "artifacts",
    ".coverage",
    "coverage.json",
    "coverage.xml",
    "Cargo.lock.cachekey",
    "vendor/qwen3-tts/qwen_tts",
    "vendor/qwen3-tts/qwen_tts.exe",
    "vendor/qwen3-tts/output.wav",
    "vendor/qwen3-tts/.engine-cachekey",
];

/// Antigüedad a partir de la cual un `.pdb` de `deps/` cuenta como obsoleto
/// para la poda fina: depurar esas unidades ya no interesa y el PDB se
/// regenera al recompilar.
pub(crate) const PRUNE_PDB_MAX_AGE_DAYS: u64 = 7;

/// Antigüedad a partir de la cual un sandbox de prueba del temporal del
/// sistema cuenta como residuo: una corrida viva no puede tener esa edad, y
/// al día siguiente el directorio ya no lo sostiene nadie.
///
/// Un día basta porque el daño de un sandbox huérfano es el espacio que ocupa
/// —cientos de MB en las pruebas que instalan modelos— y ese espacio se
/// recupera al día siguiente. Subir el umbral dejaría el disco ocupado
/// semanas; bajarlo rozaría una corrida larga.
pub(crate) const PRUNE_SANDBOX_MAX_AGE_DAYS: u64 = 1;

/// Triple del host deducido de la compilación en curso: la poda fina
/// conserva el perfil nativo y retira los cruces ajenos a este equipo.
pub(crate) fn host_triple() -> &'static str {
    match (std::env::consts::ARCH, std::env::consts::OS) {
        ("x86_64", "windows") => "x86_64-pc-windows-msvc",
        ("x86_64", "linux") => "x86_64-unknown-linux-gnu",
        ("aarch64", "linux") => "aarch64-unknown-linux-gnu",
        ("aarch64", "macos") => "aarch64-apple-darwin",
        _ => "unknown",
    }
}

/// Añade el directorio si existe, sin fallar cuando falta.
fn push_dir(out: &mut Vec<PathBuf>, dir: PathBuf) {
    if dir.is_dir() {
        out.push(dir);
    }
}

/// `.pdb` de `deps/` más antiguos que el umbral: pesan cientos de MB por
/// suite de tests y se regeneran al recompilar la unidad.
fn prune_old_pdbs(deps: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(deps) else {
        return;
    };
    let now = std::time::SystemTime::now();
    for entry in entries.flatten() {
        let path = entry.path();
        let is_pdb = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("pdb"));
        if !is_pdb {
            continue;
        }
        let old = path
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .is_some_and(|d| d.as_secs() > PRUNE_PDB_MAX_AGE_DAYS * 86400);
        if old {
            out.push(path);
        }
    }
}

/// PID de un sandbox de ciclo de vida con nombre completo y válido.
/// La etiqueta admite guiones pero no dígitos como único componente; PID y
/// nanosegundos son decimales no vacíos, y el PID debe caber en `u32`.
fn lifecycle_sandbox_pid(name: &str) -> Option<u32> {
    let suffix = name.strip_prefix(avi_shared::paths::LIFECYCLE_SANDBOX_PREFIX)?;
    let (tag_and_pid, nanos) = suffix.rsplit_once('-')?;
    let (tag, pid) = tag_and_pid.rsplit_once('-')?;
    if tag.is_empty()
        || !tag.split('-').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
                && !part.bytes().all(|b| b.is_ascii_digit())
        })
        || nanos.is_empty()
        || !nanos.bytes().all(|b| b.is_ascii_digit())
        || pid.is_empty()
        || !pid.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    pid.parse().ok()
}

/// Selecciona sandboxes reconocidos en una raíz temporal: la familia con PID
/// se decide solo por proceso vivo; las demás familias, solo por antigüedad.
fn prune_old_test_sandboxes(temp_root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(temp_root) else {
        return;
    };
    let now = std::time::SystemTime::now();
    for entry in entries.flatten() {
        let path = entry.path();
        if !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(avi_shared::paths::LIFECYCLE_SANDBOX_PREFIX) {
            if lifecycle_sandbox_pid(&name).is_some_and(|pid| !avi_process::pid_alive_exact(pid)) {
                out.push(path);
            }
            continue;
        }
        if !avi_shared::paths::TEST_SANDBOX_PREFIXES
            .iter()
            .any(|prefix| name.starts_with(prefix))
        {
            continue;
        }
        let old = path
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .is_some_and(|d| d.as_secs() > PRUNE_SANDBOX_MAX_AGE_DAYS * 86400);
        if old {
            out.push(path);
        }
    }
}

/// Carpetas de un proceso de pruebas muerto dentro de una raíz
/// `golden-avi-daemon`.
///
/// El nombre de cada carpeta es el PID del proceso que la creó. Se borra solo
/// la de un PID sin vida: la de un PID reutilizado por un proceso vivo se
/// respeta, porque ese proceso sigue usándola.
fn prune_dead_process_dirs(parent: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.is_empty() || !name.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let Ok(pid) = name.parse::<u32>() else {
            continue;
        };
        if !avi_process::pid_alive_exact(pid) {
            out.push(entry.path());
        }
    }
}

/// Residuos de las pruebas del motor en el temporal del sistema.
///
/// Son directorios de pruebas que sobreviven cuando la corrida se interrumpe
/// antes de que sus dueños los borren.
///
/// La configuración de Cargo confina el temporal de las pruebas en `target/`,
/// así que la raíz que se recorre es la del **sistema**: los residuos de las
/// corridas que no pasaron por Cargo.
///
/// La familia de ciclo de vida se decide por PID; las demás, por antigüedad.
pub(crate) fn prune_system_temp_sandboxes() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(temp_root) = avi_shared::paths::system_temp_dir() else {
        return out;
    };
    prune_old_test_sandboxes(&temp_root, &mut out);
    // La raíz de las pruebas golden se barre por PID, no por antigüedad: la
    // carpeta de un proceso vivo se respeta aunque sea vieja.
    prune_dead_process_dirs(
        &temp_root.join(avi_shared::paths::GOLDEN_DAEMON_DIR_NAME),
        &mut out,
    );
    out
}

/// Entradas de la poda fina (`--prune`): cachés regenerables y artefactos
/// obsoletos de `target/`. Nunca incluye fuentes ni la compilación vigente
/// de dependencias, que es lo que mantiene rápido el siguiente build.
pub(crate) fn prune_targets(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let target = root.join("target");
    for profile in ["debug", "release"] {
        let dir = target.join(profile);
        push_dir(&mut out, dir.join("incremental"));
        prune_old_pdbs(&dir.join("deps"), &mut out);
    }
    // Cargo confina aquí los sandboxes de prueba; se aplica el mismo criterio
    // por PID o antigüedad que en el temporal del sistema.
    prune_old_test_sandboxes(&target, &mut out);
    let Ok(entries) = std::fs::read_dir(&target) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        // Cachés efímeras de editores y compilaciones a medias, y temporales
        // confinados en `target/` (directorios `.tmp*` de `tempfile`) que dejan
        // las pruebas abortadas.
        if name.starts_with("flycheck") || name.starts_with(".tmp") || name == "tmp" {
            out.push(path);
        // Directorio de target ajeno al host: otro triple no sirve a este
        // equipo. Los marcadores de Cargo distinguen un target real de un
        // directorio cualquiera con guiones en el nombre.
        } else if name != host_triple()
            && (path.join("CACHEDIR.TAG").is_file() || path.join("debug").is_dir())
        {
            out.push(path);
        }
    }
    // La raíz de las pruebas golden se barre por PID y no por antigüedad: la
    // carpeta de un proceso vivo se respeta aunque sea vieja.
    prune_dead_process_dirs(
        &target.join(avi_shared::paths::GOLDEN_DAEMON_DIR_NAME),
        &mut out,
    );
    out
}

/// Rutas de la capa proyecto que existen bajo `root`.
pub(crate) fn repo_targets(root: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = REPO_ENTRIES
        .iter()
        .map(|e| e.split('/').fold(root.to_path_buf(), |acc, c| acc.join(c)))
        .filter(|p| p.symlink_metadata().is_ok())
        .collect();
    // Objetos, dependencias y libs estáticas del motor (sin versionar).
    collect_c_artifacts(&root.join("vendor").join("qwen3-tts"), &mut out);
    out
}

fn collect_c_artifacts(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_dir() {
            if !out.contains(&path) {
                collect_c_artifacts(&path, out);
            }
        } else if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("o" | "d" | "a")
        ) {
            out.push(path);
        }
    }
}

/// Tamaño de una ruta. Si alguna parte no se pudo leer, `bytes` es solo una
/// cota inferior y `incomplete` lo señala.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Size {
    pub bytes: u64,
    pub incomplete: bool,
}

impl Size {
    const ZERO: Self = Self {
        bytes: 0,
        incomplete: false,
    };

    fn plus(self, other: Self) -> Self {
        Self {
            bytes: self.bytes + other.bytes,
            incomplete: self.incomplete || other.incomplete,
        }
    }

    /// Texto legible; una cota inferior se marca como incompleta.
    pub(crate) fn text(self) -> String {
        if self.incomplete {
            format!("≥ {} (incompleto)", avi_shared::human_bytes(self.bytes))
        } else {
            avi_shared::human_bytes(self.bytes)
        }
    }
}

/// Tamaño en bytes de una ruta (recursivo, sin seguir symlinks). Una ruta
/// inexistente pesa cero; cualquier otro fallo de lectura marca el resultado
/// como incompleto en vez de contarlo como cero.
pub(crate) fn size_of(path: &Path) -> Size {
    let meta = match path.symlink_metadata() {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Size::ZERO,
        Err(_) => {
            return Size {
                bytes: 0,
                incomplete: true,
            }
        }
    };
    if !meta.is_dir() {
        return Size {
            bytes: meta.len(),
            incomplete: false,
        };
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return Size {
            bytes: 0,
            incomplete: true,
        };
    };
    let mut total = Size::ZERO;
    for entry in entries {
        total = match entry {
            Ok(e) => total.plus(size_of(&e.path())),
            Err(_) => total.plus(Size {
                bytes: 0,
                incomplete: true,
            }),
        };
    }
    total
}

/// Líneas del listado con su tamaño: directorios uno por línea; ficheros
/// sueltos agrupados por (directorio, extensión) cuando hay 3 o más.
fn summarize(paths: &[PathBuf]) -> Vec<(String, Size)> {
    let mut groups: Vec<((PathBuf, String), Vec<&PathBuf>)> = Vec::new();
    for p in paths {
        let key = if p.is_dir() {
            (p.clone(), String::new())
        } else {
            let ext = p
                .extension()
                .map(|e| e.to_string_lossy().to_string())
                .unwrap_or_default();
            (p.parent().unwrap_or(p).to_path_buf(), ext)
        };
        match groups
            .iter_mut()
            .find(|(k, _)| *k == key && !k.1.is_empty())
        {
            Some((_, v)) => v.push(p),
            None => groups.push((key, vec![p])),
        }
    }
    let mut out = Vec::new();
    for ((dir, ext), members) in groups {
        if members.len() >= 3 {
            let size = members
                .iter()
                .fold(Size::ZERO, |acc, p| acc.plus(size_of(p)));
            out.push((
                format!("{} × *.{} en {}", members.len(), ext, dir.display()),
                size,
            ));
        } else {
            for p in members {
                out.push((p.display().to_string(), size_of(p)));
            }
        }
    }
    out
}

fn remove_path(path: &Path) -> std::io::Result<()> {
    let meta = path.symlink_metadata()?;
    if meta.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

/// Binario del producto en `target/` (para parar un daemon lanzado con `cargo run`).
fn repo_product_bins(root: &Path) -> Vec<PathBuf> {
    let name = format!("{}{}", APP_NAME, std::env::consts::EXE_SUFFIX);
    ["debug", "release"]
        .iter()
        .map(|p| root.join("target").join(p).join(&name))
        .filter(|p| p.is_file())
        .collect()
}

/// Binario del producto compilado en el repo para delegar la capa app:
/// `target/release` preferido, `target/debug` como alternativa. `None` si no
/// hay ninguno compilado.
pub(crate) fn repo_product_bin(root: &Path) -> Option<PathBuf> {
    repo_product_bins(root).into_iter().next()
}

/// Ejecuta `bin args…` sin salida propia. El borrado posterior no depende del
/// resultado, pero un fallo (no se pudo lanzar o salió con error) se avisa
/// porque deja ficheros bloqueados.
fn run_quiet(bin: &Path, args: &[&str]) {
    let result = std::process::Command::new(bin)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    match result {
        Ok(status) if status.success() => {}
        Ok(status) => eprintln!(
            "aviso: `{} {}` terminó con {status}",
            bin.display(),
            args.join(" ")
        ),
        Err(e) => eprintln!(
            "aviso: no se pudo ejecutar `{} {}`: {e}",
            bin.display(),
            args.join(" ")
        ),
    }
}

/// `true` si la ruta contiene el componente `Caskroom`: la copia la
/// gestiona Homebrew (criterio por componente, no por subcadena). Pura.
pub(crate) fn path_has_caskroom(path: &Path) -> bool {
    path.components().any(|c| {
        c.as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case("Caskroom")
    })
}

/// `true` si la copia instalada del producto la gestiona Homebrew: el comando
/// resuelto en el PATH está bajo `Caskroom`. Mejor esfuerzo: sin
/// comando instalado no hay canal `homebrew` que respetar.
fn installed_copy_is_homebrew() -> bool {
    let name = format!("{}{}", APP_NAME, std::env::consts::EXE_SUFFIX);
    super::doctor::find_on_path(&name).is_some_and(|p| path_has_caskroom(&p))
}

/// Argumentos de la delegación de la capa app con el binario del repositorio:
/// `self uninstall --yes`, o `cleanup --all --yes` en canal `homebrew`; con
/// `dry_run`, la variante de solo lectura. Puros, fijados por test.
pub(crate) fn delegate_app_args(homebrew: bool, dry_run: bool) -> Vec<String> {
    let mut args: Vec<String> = if homebrew {
        ["cleanup", "--all"]
            .iter()
            .map(|s| (*s).to_string())
            .collect()
    } else {
        ["self", "uninstall"]
            .iter()
            .map(|s| (*s).to_string())
            .collect()
    };
    if dry_run {
        args.push("--dry-run".to_string());
    }
    args.push("--yes".to_string());
    args
}

/// Delega la capa app en el binario del repositorio (hereda stdio para que la
/// confirmación y el progreso del producto queden visibles) y propaga su
/// código de salida.
fn delegate_app_layer(repo_bin: &Path, homebrew: bool, dry_run: bool) -> Result<()> {
    let args = delegate_app_args(homebrew, dry_run);
    println!(
        "Capa app: delegando en {} {} …",
        repo_bin.display(),
        args.join(" ")
    );
    let status = std::process::Command::new(repo_bin)
        .args(&args)
        .status()
        .map_err(|e| anyhow::anyhow!("no se pudo ejecutar {}: {e}", repo_bin.display()))?;
    if !status.success() {
        bail!(
            "la limpieza de la capa app falló (exit {:?})",
            status.code()
        );
    }
    Ok(())
}

/// Subdirectorios de `CARGO_HOME` que Cargo documenta como regenerables.
/// `bin/` (herramientas instaladas) queda fuera a propósito.
const CARGO_HOME_REGENERABLE: &[&[&str]] = &[
    &["registry", "cache"],
    &["registry", "src"],
    &["registry", "index"],
    &["git", "db"],
    &["git", "checkouts"],
];

/// `CARGO_HOME` (variable) o `~/.cargo`.
fn cargo_home() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("CARGO_HOME").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    std::env::home_dir()
        .map(|h| h.join(".cargo"))
        .ok_or_else(|| anyhow::anyhow!("no se pudo resolver CARGO_HOME ni el directorio personal"))
}

/// Directorios `CacheDir` declarados en el texto de un `pacman.conf`. Las
/// rutas estilo MSYS2 (`/var/cache/pacman/pkg/`) se anclan en `msys2_root`.
/// Sin ninguna línea `CacheDir` vale el defecto documentado de pacman,
/// `/var/cache/pacman/pkg/`. Pura.
fn pacman_cache_dirs(conf: &str, msys2_root: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = conf
        .lines()
        .filter_map(|line| {
            let (key, value) = line.split_once('=')?;
            (key.trim() == "CacheDir").then_some(value.trim())
        })
        .filter(|v| !v.is_empty())
        .map(|v| match v.strip_prefix('/') {
            Some(rel) => rel
                .split('/')
                .filter(|c| !c.is_empty())
                .fold(msys2_root.to_path_buf(), |acc, c| acc.join(c)),
            None => PathBuf::from(v),
        })
        .collect();
    if dirs.is_empty() {
        dirs.push(
            msys2_root
                .join("var")
                .join("cache")
                .join("pacman")
                .join("pkg"),
        );
    }
    dirs
}

/// Rutas de la capa global: los directorios regenerables de `cargo_home` que
/// existen y, si se da `msys2_root`, el contenido (no el directorio) de cada
/// `CacheDir` de pacman. Falla si `msys2_root` no tiene `etc/pacman.conf`.
fn global_targets(cargo_home: &Path, msys2_root: Option<&Path>) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for parts in CARGO_HOME_REGENERABLE {
        let dir = parts
            .iter()
            .fold(cargo_home.to_path_buf(), |a, c| a.join(c));
        push_dir(&mut out, dir);
    }
    if let Some(root) = msys2_root {
        let conf_path = root.join("etc").join("pacman.conf");
        let conf = std::fs::read_to_string(&conf_path).map_err(|e| {
            anyhow::anyhow!(
                "no se pudo leer {} (¿MSYS2 instalado?): {e}",
                conf_path.display()
            )
        })?;
        for dir in pacman_cache_dirs(&conf, root) {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            let mut items: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
            items.sort();
            out.extend(items);
        }
    }
    Ok(out)
}

/// Resultado de conciliar `target.removal-failed.log` con la existencia de
/// `target/`.
#[derive(Debug, PartialEq, Eq)]
enum RemovalLog {
    /// No hay registro de fallo.
    Absent,
    /// `target/` ya no existe: el registro sobra (retirado, o por retirar en dry-run).
    Stale(PathBuf),
    /// `target/` sigue existiendo: el registro se conserva e informa.
    Active(PathBuf),
}

/// Concilia el registro que deja el borrado diferido de `target/` al fallar.
/// Con `apply` retira el registro obsoleto; nunca borra el de un `target/`
/// que sigue existiendo.
fn reconcile_removal_log(root: &Path, apply: bool) -> Result<RemovalLog> {
    let log = root.join("target.removal-failed.log");
    if log.symlink_metadata().is_err() {
        return Ok(RemovalLog::Absent);
    }
    if root.join("target").symlink_metadata().is_ok() {
        return Ok(RemovalLog::Active(log));
    }
    if apply {
        std::fs::remove_file(&log)
            .map_err(|e| anyhow::anyhow!("no se pudo retirar {}: {e}", log.display()))?;
    }
    Ok(RemovalLog::Stale(log))
}

/// Concilia e informa el registro de fallo del borrado diferido; con `apply`
/// retira el obsoleto, sin él solo anuncia lo que se haría.
fn report_removal_log(root: &Path, apply: bool) -> Result<()> {
    match reconcile_removal_log(root, apply)? {
        RemovalLog::Absent => {}
        RemovalLog::Stale(log) if apply => println!("  ✓ {} (obsoleto, retirado)", log.display()),
        RemovalLog::Stale(log) => {
            println!(
                "  {} (obsoleto: target/ ya no existe; se retirará)",
                log.display()
            );
        }
        RemovalLog::Active(log) => println!(
            "  aviso: {} indica que el borrado diferido de target/ falló; revísalo",
            log.display()
        ),
    }
    Ok(())
}

/// Punto de entrada de `xtask clean`. Con `prune`, la capa repo se limita
/// a la poda fina y conserva la compilación vigente; el defecto (borrado
/// total de la capa) no cambia.
pub fn run(layer: Layer, global: bool, dry_run: bool, yes: bool, prune: bool) -> Result<()> {
    let root = std::env::current_dir()?;
    if !root.join("Cargo.toml").is_file() || !root.join("crates").join("xtask").is_dir() {
        bail!("ejecuta `cargo xtask clean` desde la raíz del repositorio");
    }

    let repo_paths = if layer.wants_repo() {
        if prune {
            prune_targets(&root)
        } else {
            repo_targets(&root)
        }
    } else {
        Vec::new()
    };
    // Los sandboxes del temporal del sistema son residuos de construir y probar
    // el repo, así que van en la capa repo. Solo la poda fina los retira: la
    // capa completa borra el repo entero y no necesita de ellos.
    let mut sandbox_paths = if layer.wants_repo() && prune {
        prune_system_temp_sandboxes()
    } else {
        Vec::new()
    };
    // Las dos raíces pueden coincidir —si el temporal del sistema es el del
    // repo—, y una ruta repetida se borraría dos veces.
    sandbox_paths.retain(|p| !repo_paths.contains(p));
    let homebrew = if layer.wants_app() {
        installed_copy_is_homebrew()
    } else {
        false
    };
    let global_paths = if global {
        let msys2 = cfg!(windows).then(super::doctor::msys2_root);
        global_targets(&cargo_home()?, msys2.as_deref())?
    } else {
        Vec::new()
    };

    let title = match layer {
        Layer::None => "Capa global (--global)",
        Layer::Repo if prune => "Capa proyecto (poda fina --prune)",
        Layer::Repo => "Capa proyecto (repo)",
        Layer::App => "Capa app (perfil de usuario, delegada)",
        Layer::All if prune => "App y después poda fina (app + --prune)",
        Layer::All => "Ambas capas (app y después repo)",
    };
    if layer != Layer::None {
        println!("{title}:");
    }
    if layer.wants_repo() {
        if repo_paths.is_empty() {
            println!("  (nada en el repo)");
        }
        let mut total = Size::ZERO;
        for (label, size) in summarize(&repo_paths) {
            total = total.plus(size);
            println!("  {:>10}  {}", size.text(), label);
        }
        println!("Total repo: {}", total.text());
        report_removal_log(&root, false)?;
    }
    if global {
        println!("Capa global (--global):");
        if global_paths.is_empty() {
            println!("  (nada en la capa global)");
        }
        let mut total = Size::ZERO;
        for (label, size) in summarize(&global_paths) {
            total = total.plus(size);
            println!("  {:>10}  {}", size.text(), label);
        }
        println!("Total global: {}", total.text());
    }
    if !sandbox_paths.is_empty() {
        println!("Sandboxes de prueba huérfanos (temporal del sistema):");
        let mut total = Size::ZERO;
        for (label, size) in summarize(&sandbox_paths) {
            total = total.plus(size);
            println!("  {:>10}  {}", size.text(), label);
        }
        println!("Total sandboxes: {}", total.text());
    }
    if layer.wants_app() {
        println!(
            "  (la capa app la ejecuta el binario del repositorio: {} {})",
            if homebrew {
                "cleanup --all"
            } else {
                "self uninstall"
            },
            if dry_run { "--dry-run" } else { "--yes" }
        );
    }
    println!(
        "Nunca se toca: CARGO_HOME/bin, rustup, la caché de sccache ni la instalación de MSYS2. \
         Solo `--global` limpia las cachés regenerables de Cargo (registry, git) y de pacman."
    );
    if !sandbox_paths.is_empty() {
        println!(
            "Del temporal del sistema solo se retiran directorios de prueba reconocidos: \
             los de ciclo de vida con PID muerto y los demás más antiguos que el umbral."
        );
    }
    if dry_run {
        if layer.wants_app() {
            let repo_bin = repo_product_bin(&root).ok_or_else(|| {
                anyhow::anyhow!(
                    "sin binario del producto en target/: compila antes (`cargo build`) para previsualizar la capa app"
                )
            })?;
            delegate_app_layer(&repo_bin, homebrew, true)?;
        }
        println!("Dry-run: no se borró nada.");
        return Ok(());
    }
    if !layer.wants_app()
        && repo_paths.is_empty()
        && global_paths.is_empty()
        && sandbox_paths.is_empty()
    {
        // Un registro obsoleto se retira aunque no quede nada más que borrar.
        if layer.wants_repo() {
            report_removal_log(&root, true)?;
        }
        println!("Nada que limpiar.");
        return Ok(());
    }

    if !yes {
        if !std::io::stdin().is_terminal() {
            bail!("sin TTY: confirma con --yes (o revisa antes con --dry-run)");
        }
        print!(
            "¿Borrar lo listado (capa {}{})? [s/N]: ",
            match layer {
                Layer::None => "global",
                Layer::Repo if prune => "poda fina",
                Layer::Repo => "repo",
                Layer::App => "app",
                Layer::All if prune => "app + poda fina",
                Layer::All => "app + repo",
            },
            if global && layer != Layer::None {
                " + global"
            } else {
                ""
            }
        );
        std::io::stdout().flush()?;
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        let t = input.trim().to_lowercase();
        if !matches!(t.as_str(), "s" | "si" | "sí" | "y" | "yes") {
            println!("Cancelado.");
            return Ok(());
        }
    }

    // Parar daemons lanzados desde `target/`: libera ficheros bloqueados
    // antes de desinstalar o borrar.
    for bin in repo_product_bins(&root) {
        run_quiet(&bin, &["daemon", "stop", "--json"]);
    }

    // Capa app primero (en `--all`, antes que el repo): desinstala con el
    // propio producto y revierte PATH/registro, que un borrado no cubre.
    if layer.wants_app() {
        let repo_bin = repo_product_bin(&root).ok_or_else(|| {
            anyhow::anyhow!(
                "sin binario del producto en target/: compila antes (`cargo build`) para limpiar la capa app"
            )
        })?;
        delegate_app_layer(&repo_bin, homebrew, false)?;
    }

    let self_exe = std::env::current_exe()
        .map_err(|e| anyhow::anyhow!("no se pudo resolver el ejecutable de xtask: {e}"))?;
    let target_dir = root.join("target");
    let mut failed: Vec<(PathBuf, String)> = Vec::new();
    let mut deferred_target = false;
    for p in repo_paths.iter().chain(&global_paths).chain(&sandbox_paths) {
        if p.symlink_metadata().is_err() {
            continue;
        }
        match remove_path(p) {
            Ok(()) => println!("  ✓ {}", p.display()),
            Err(e) => {
                // Windows no borra el ejecutable en uso (`target/.../xtask.exe`).
                let own = cfg!(windows) && *p == target_dir && self_exe.starts_with(&target_dir);
                if own {
                    deferred_target = true;
                } else {
                    failed.push((p.clone(), e.to_string()));
                }
            }
        }
    }

    if deferred_target {
        #[cfg(windows)]
        {
            avi_process::schedule_removal(&target_dir, &avi_process::current_owner_token()?)?;
        }
        println!(
            "  … {} se termina de borrar al salir este proceso (limpiador en segundo plano)",
            target_dir.display()
        );
    }
    if !failed.is_empty() {
        eprintln!("No se pudieron borrar {} ruta(s):", failed.len());
        for (p, e) in &failed {
            eprintln!("  ✗ {}: {}", p.display(), e);
        }
        bail!("limpieza incompleta");
    }
    // Con `target/` ya borrado, el registro del fallo anterior sobra.
    if layer.wants_repo() {
        report_removal_log(&root, true)?;
    }
    println!("Limpieza completa.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Las capas piden lo que declaran: `--repo` por defecto no alcanza la
    /// app, `--all` alcanza ambas.
    #[test]
    fn layer_scope() {
        assert!(Layer::Repo.wants_repo() && !Layer::Repo.wants_app());
        assert!(!Layer::App.wants_repo() && Layer::App.wants_app());
        assert!(Layer::All.wants_repo() && Layer::All.wants_app());
        assert!(!Layer::None.wants_repo() && !Layer::None.wants_app());
    }

    /// Selección de capa desde los flags: `--all` no implica `--global` (que
    /// viaja aparte), y `--global` solo no limpia el repositorio.
    #[test]
    fn layer_selection_from_flags() {
        assert_eq!(Layer::select(false, false, false, false), Layer::Repo);
        assert_eq!(Layer::select(false, false, true, false), Layer::All);
        assert_eq!(Layer::select(true, true, false, false), Layer::All);
        assert_eq!(Layer::select(false, true, false, true), Layer::App);
        assert_eq!(Layer::select(false, false, false, true), Layer::None);
        assert_eq!(Layer::select(true, false, false, true), Layer::Repo);
        assert_eq!(Layer::select(false, false, true, true), Layer::All);
    }

    /// Registro de fallo del borrado diferido: se retira si `target/` ya no
    /// existe, se informa sin borrar si existe y no hay nada si falta.
    #[test]
    fn removal_log_reconciliation() {
        let guard = tempfile::Builder::new()
            .prefix("xtask_clean_log_")
            .tempdir()
            .unwrap();
        let root = guard.path();
        let log = root.join("target.removal-failed.log");
        assert_eq!(
            reconcile_removal_log(root, true).unwrap(),
            RemovalLog::Absent
        );

        std::fs::write(&log, b"fallo").unwrap();
        std::fs::create_dir(root.join("target")).unwrap();
        assert_eq!(
            reconcile_removal_log(root, true).unwrap(),
            RemovalLog::Active(log.clone())
        );
        assert!(log.is_file());

        std::fs::remove_dir(root.join("target")).unwrap();
        assert_eq!(
            reconcile_removal_log(root, false).unwrap(),
            RemovalLog::Stale(log.clone())
        );
        assert!(log.is_file(), "sin apply no se borra");
        assert_eq!(
            reconcile_removal_log(root, true).unwrap(),
            RemovalLog::Stale(log.clone())
        );
        assert!(!log.exists());
    }

    /// Lista exacta de la capa global sobre raíces falsas: los cinco
    /// directorios regenerables de Cargo y el contenido del `CacheDir` de
    /// pacman, sin `bin/` ni nada más.
    #[test]
    fn global_targets_exact_list() {
        let guard = tempfile::Builder::new()
            .prefix("xtask_clean_global_")
            .tempdir()
            .unwrap();
        let cargo = guard.path().join("cargo");
        for d in [
            "registry/cache",
            "registry/src",
            "registry/index",
            "git/db",
            "git/checkouts",
            "bin",
        ] {
            std::fs::create_dir_all(cargo.join(d)).unwrap();
        }
        let msys = guard.path().join("msys64");
        std::fs::create_dir_all(msys.join("etc")).unwrap();
        std::fs::create_dir_all(msys.join("custom/pkgs")).unwrap();
        std::fs::write(msys.join("custom/pkgs/b.pkg"), b"x").unwrap();
        std::fs::write(msys.join("custom/pkgs/a.pkg"), b"x").unwrap();
        std::fs::write(
            msys.join("etc/pacman.conf"),
            "[options]\n#CacheDir = /var/cache/pacman/pkg/\nCacheDir = /custom/pkgs/\n",
        )
        .unwrap();

        let got = global_targets(&cargo, Some(&msys)).unwrap();
        let want = vec![
            cargo.join("registry/cache"),
            cargo.join("registry/src"),
            cargo.join("registry/index"),
            cargo.join("git/db"),
            cargo.join("git/checkouts"),
            msys.join("custom/pkgs/a.pkg"),
            msys.join("custom/pkgs/b.pkg"),
        ];
        assert_eq!(got, want);

        // Sin raíz MSYS2 (plataformas no Windows) solo entra Cargo.
        assert_eq!(global_targets(&cargo, None).unwrap(), want[..5]);
    }

    /// En Windows, un MSYS2 sin `pacman.conf` es un error explícito, y sin
    /// línea `CacheDir` vale el defecto documentado de pacman.
    #[test]
    fn global_targets_pacman_conf_rules() {
        let guard = tempfile::Builder::new()
            .prefix("xtask_clean_pacman_")
            .tempdir()
            .unwrap();
        let err = global_targets(
            &guard.path().join("cargo"),
            Some(&guard.path().join("msys64")),
        )
        .unwrap_err();
        assert!(err.to_string().contains("pacman.conf"));
        assert_eq!(
            pacman_cache_dirs("[options]\n#CacheDir = /x/\n", Path::new("/m")),
            [Path::new("/m").join("var/cache/pacman/pkg")]
        );
    }

    /// La delegación usa `self uninstall --yes`, o `cleanup --all --yes` en
    /// canal `homebrew`; el dry-run es de solo lectura.
    #[test]
    fn delegation_args_per_channel() {
        assert_eq!(
            delegate_app_args(false, false),
            ["self", "uninstall", "--yes"]
        );
        assert_eq!(
            delegate_app_args(false, true),
            ["self", "uninstall", "--dry-run", "--yes"]
        );
        assert_eq!(
            delegate_app_args(true, false),
            ["cleanup", "--all", "--yes"]
        );
        assert_eq!(
            delegate_app_args(true, true),
            ["cleanup", "--all", "--dry-run", "--yes"]
        );
    }

    /// El canal `homebrew` se reconoce por componente (`Caskroom`), no por
    /// subcadena.
    #[test]
    fn caskroom_detection_by_component() {
        assert!(path_has_caskroom(Path::new(
            "/opt/homebrew/Caskroom/ai-voice-interconnector/0.24.0/ai-voice-interconnector"
        )));
        assert!(!path_has_caskroom(Path::new(
            "/opt/homebrew/bin/ai-voice-interconnector"
        )));
        assert!(!path_has_caskroom(Path::new(
            r"C:\Users\ana\.local\bin\ai-voice-interconnector.exe"
        )));
    }

    #[test]
    fn repo_targets_curated_and_keep_sources() {
        let root_guard = tempfile::Builder::new()
            .prefix("xtask_clean_repo_")
            .tempdir()
            .unwrap();
        let root = root_guard.path().to_path_buf();
        let engine = root.join("vendor/qwen3-tts");
        let ingot = engine.join("third_party/ingot/src");
        std::fs::create_dir_all(&ingot).unwrap();
        std::fs::create_dir_all(root.join("target/debug")).unwrap();
        std::fs::create_dir_all(engine.join("presets")).unwrap();
        for f in ["main.c", "main.o", "main.d", "Makefile"] {
            std::fs::write(engine.join(f), b"x").unwrap();
        }
        std::fs::write(ingot.join("q.o"), b"x").unwrap();
        std::fs::write(engine.join("third_party/ingot/libingot.a"), b"x").unwrap();

        let got = repo_targets(&root);

        for keep in ["main.c", "Makefile", "presets"] {
            assert!(!got.contains(&engine.join(keep)), "no debe borrar {}", keep);
        }
        for gone in [
            root.join("target"),
            engine.join("main.o"),
            engine.join("main.d"),
            ingot.join("q.o"),
            engine.join("third_party/ingot/libingot.a"),
        ] {
            assert!(got.contains(&gone), "falta {}", gone.display());
        }
    }

    /// La poda fina retira cachés, cruces ajenos y PDBs viejos, y conserva
    /// fuentes, la compilación vigente y el triple del host.
    #[test]
    fn prune_targets_selective() {
        let root_guard = tempfile::Builder::new()
            .prefix("xtask_clean_prune_")
            .tempdir()
            .unwrap();
        let root = root_guard.path().to_path_buf();
        let target = root.join("target");
        let debug_deps = target.join("debug/deps");
        std::fs::create_dir_all(target.join("debug/incremental")).unwrap();
        std::fs::create_dir_all(target.join("release/incremental")).unwrap();
        std::fs::create_dir_all(&debug_deps).unwrap();
        std::fs::create_dir_all(target.join("flycheck0")).unwrap();
        std::fs::create_dir_all(target.join("tmp")).unwrap();
        std::fs::create_dir_all(target.join("cxxbridge")).unwrap();
        // Cruce ajeno al host (con marcador de Cargo) y triple propio.
        let foreign = if host_triple() == "aarch64-apple-darwin" {
            "x86_64-unknown-linux-gnu"
        } else {
            "aarch64-apple-darwin"
        };
        let foreign_dir = target.join(foreign);
        std::fs::create_dir_all(&foreign_dir).unwrap();
        std::fs::write(foreign_dir.join("CACHEDIR.TAG"), b"x").unwrap();
        let native_dir = target.join(host_triple());
        std::fs::create_dir_all(native_dir.join("debug")).unwrap();
        // PDB reciente (vigente) y PDB viejo (obsoleto).
        let fresh = debug_deps.join("current.pdb");
        let stale = debug_deps.join("obsolete.pdb");
        std::fs::write(&fresh, b"x").unwrap();
        std::fs::write(&stale, b"x").unwrap();
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(8 * 86400);
        std::fs::File::options()
            .write(true)
            .open(&stale)
            .unwrap()
            .set_modified(old)
            .unwrap();
        std::fs::write(debug_deps.join("libfoo.rlib"), b"x").unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.rs"), b"x").unwrap();

        let got = prune_targets(&root);

        for gone in [
            target.join("debug/incremental"),
            target.join("release/incremental"),
            target.join("flycheck0"),
            target.join("tmp"),
            foreign_dir,
            stale,
        ] {
            assert!(got.contains(&gone), "falta {}", gone.display());
        }
        for keep in [
            debug_deps.join("libfoo.rlib"),
            fresh,
            native_dir,
            target.join("cxxbridge"),
            root.join("src/main.rs"),
        ] {
            assert!(!got.contains(&keep), "no debe borrar {}", keep.display());
        }
    }

    /// La poda de sandboxes retira solo los viejos: un sandbox de una corrida
    /// viva se conserva, y uno de otro nombre tampoco se toca.
    #[test]
    fn prune_old_test_sandboxes_selects_by_age() {
        let root_guard = tempfile::Builder::new()
            .prefix("xtask_clean_sandbox_")
            .tempdir()
            .unwrap();
        let root = root_guard.path();

        // Un sandbox de cada prefijo reconocido: uno viejo y uno joven.
        let mut old_sandboxes = Vec::new();
        let mut fresh_sandboxes = Vec::new();
        for prefix in avi_shared::paths::TEST_SANDBOX_PREFIXES
            .iter()
            .filter(|prefix| **prefix != avi_shared::paths::LIFECYCLE_SANDBOX_PREFIX)
        {
            let old = root.join(format!("{prefix}caso_AbCd12"));
            let fresh = root.join(format!("{prefix}caso_ZzYy34"));
            std::fs::create_dir_all(&old).unwrap();
            std::fs::create_dir_all(&fresh).unwrap();
            let old_time = std::time::SystemTime::now() - std::time::Duration::from_secs(2 * 86400);
            filetime::set_file_mtime(&old, filetime::FileTime::from_system_time(old_time)).unwrap();
            old_sandboxes.push(old);
            fresh_sandboxes.push(fresh);
        }
        // Un directorio con otro nombre: no es del motor y no se toca.
        let foreign = root.join("otro-proyecto");
        std::fs::create_dir_all(&foreign).unwrap();
        let foreign_time =
            std::time::SystemTime::now() - std::time::Duration::from_secs(30 * 86400);
        filetime::set_file_mtime(&foreign, filetime::FileTime::from_system_time(foreign_time))
            .unwrap();

        let mut got = Vec::new();
        prune_old_test_sandboxes(root, &mut got);

        for gone in &old_sandboxes {
            assert!(got.contains(gone), "falta {}", gone.display());
        }
        for keep in fresh_sandboxes.iter().chain([&foreign]) {
            assert!(!got.contains(keep), "no debe borrar {}", keep.display());
        }
    }

    /// Los sandboxes de ciclo de vida se deciden por PID y forma completa,
    /// nunca por antigüedad; los nombres incompletos se conservan.
    #[test]
    fn prune_lifecycle_sandboxes_by_pid_and_complete_name() {
        let guard = tempfile::Builder::new()
            .prefix("xtask_clean_pid_")
            .tempdir()
            .unwrap();
        let root = guard.path();
        let prefix = avi_shared::paths::LIFECYCLE_SANDBOX_PREFIX;
        let dead = root.join(format!("{prefix}daemon-stop-4294967000-1"));
        let alive = root.join(format!("{prefix}daemon-stop-{}-2", std::process::id()));
        let invalid = root.join(format!("{prefix}daemon-stop-4294967000-extra"));
        let missing_tag = root.join(format!("{prefix}4294967000-1"));
        let foreign = root.join("other-project-sandbox-daemon-stop-4294967000-1");
        for path in [&dead, &alive, &invalid, &missing_tag, &foreign] {
            std::fs::create_dir(path).unwrap();
        }
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(2 * 86400);
        for path in [&alive, &invalid, &missing_tag, &foreign] {
            filetime::set_file_mtime(path, filetime::FileTime::from_system_time(old)).unwrap();
        }
        let mut got = Vec::new();
        prune_old_test_sandboxes(root, &mut got);
        assert!(
            got.contains(&dead),
            "un PID muerto se retira aunque sea reciente"
        );
        for path in [&alive, &invalid, &missing_tag, &foreign] {
            assert!(!got.contains(path), "no debe borrar {}", path.display());
        }
    }

    /// La poda por PID de la raíz de las pruebas golden borra la carpeta de un
    /// proceso muerto y respeta la de un proceso vivo.
    #[test]
    fn prune_dead_process_dirs_respects_live_pids() {
        let root_guard = tempfile::Builder::new()
            .prefix("xtask_clean_pid_")
            .tempdir()
            .unwrap();
        let root = root_guard.path();
        let alive = root.join(std::process::id().to_string());
        let dead = root.join("4294967000");
        let other = root.join("no-es-un-pid");
        for dir in [&alive, &dead, &other] {
            std::fs::create_dir_all(dir).unwrap();
        }

        let mut got = Vec::new();
        prune_dead_process_dirs(root, &mut got);

        assert!(got.contains(&dead), "falta la carpeta del PID muerto");
        assert!(!got.contains(&alive), "no debe borrar la de un PID vivo");
        assert!(
            !got.contains(&other),
            "no debe borrar un nombre que no es un PID"
        );
    }

    /// Una ruta inexistente pesa cero y una ilegible marca el tamaño como
    /// incompleto en vez de contarse como cero.
    #[test]
    fn size_of_marks_unreadable_paths() {
        let guard = tempfile::Builder::new()
            .prefix("xtask_clean_size_")
            .tempdir()
            .unwrap();
        let file = guard.path().join("f");
        std::fs::write(&file, b"abcd").unwrap();
        assert_eq!(
            size_of(&file),
            Size {
                bytes: 4,
                incomplete: false
            }
        );
        assert_eq!(size_of(&guard.path().join("missing")), Size::ZERO);
        assert!(Size {
            bytes: 1,
            incomplete: true
        }
        .text()
        .contains("incompleto"));
        assert_eq!(size_of(guard.path()).bytes, 4);
    }

    /// Sin réplicas: la capa app la ejecuta el binario del repositorio, así
    /// que `xtask` no necesita ni las rutas del producto (`avi-store`, con
    /// su árbol TLS) ni `directories`. Este test lo fija sobre el manifiesto.
    #[test]
    fn no_product_path_dependencies() {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let text = std::fs::read_to_string(&manifest).unwrap();
        for dep in ["avi-store", "directories"] {
            assert!(
                !text.contains(dep),
                "xtask/Cargo.toml no debe depender de `{dep}` (fin de la réplica)"
            );
        }
    }
}
