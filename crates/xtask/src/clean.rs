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
//! efímeras, targets cruzados ajenos al host y `.pdb` viejos de `deps/`, y
//! conserva la compilación vigente para que el siguiente build siga siendo
//! incremental en vez de frío.
//!
//! La capa global compartida (`~/.cargo`, caché de sccache, paquetes del
//! sistema, MSYS2) nunca se toca; solo se informa. Detiene
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
/// después repositorio).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    Repo,
    App,
    All,
}

impl Layer {
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
    "build",
    "dist",
    "dist_test",
    "models",
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
    let Ok(entries) = std::fs::read_dir(&target) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        // Cachés efímeras de editores y compilaciones a medias.
        if name.starts_with("flycheck") || name == "tmp" {
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

/// Punto de entrada de `xtask clean`. Con `prune`, la capa repo se limita
/// a la poda fina y conserva la compilación vigente; el defecto (borrado
/// total de la capa) no cambia.
pub fn run(layer: Layer, dry_run: bool, yes: bool, prune: bool) -> Result<()> {
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
    let homebrew = if layer.wants_app() {
        installed_copy_is_homebrew()
    } else {
        false
    };

    let title = match layer {
        Layer::Repo if prune => "Capa proyecto (poda fina --prune)",
        Layer::Repo => "Capa proyecto (repo)",
        Layer::App => "Capa app (perfil de usuario, delegada)",
        Layer::All if prune => "App y después poda fina (app + --prune)",
        Layer::All => "Ambas capas (app y después repo)",
    };
    println!("{title}:");
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
    println!("Nunca se toca: ~/.cargo, caché de sccache, paquetes del sistema ni MSYS2.");
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
    if !layer.wants_app() && repo_paths.is_empty() {
        println!("Nada que limpiar.");
        return Ok(());
    }

    if !yes {
        if !std::io::stdin().is_terminal() {
            bail!("sin TTY: confirma con --yes (o revisa antes con --dry-run)");
        }
        print!(
            "¿Borrar lo listado (capa {})? [s/N]: ",
            match layer {
                Layer::Repo if prune => "poda fina",
                Layer::Repo => "repo",
                Layer::App => "app",
                Layer::All if prune => "app + poda fina",
                Layer::All => "app + repo",
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
    for p in &repo_paths {
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
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|e| anyhow::anyhow!("el reloj del sistema es anterior a 1970: {e}"))?
                .as_millis();
            let log = std::env::temp_dir().join(format!(
                "avi-xtask-clean-{}-{}.log",
                std::process::id(),
                stamp
            ));
            avi_process::schedule_clean_removal(&target_dir, std::process::id(), &log)?;
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
