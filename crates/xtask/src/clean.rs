//! `xtask clean`: devuelve la máquina del desarrollador a un estado limpio por
//! capas, para compilar, instalar o usar solo artefactos nuevos:
//!
//! | Capa | Flag | Contenido | Mecanismo |
//! |---|---|---|---|
//! | Repositorio | `--repo` (por defecto) | `target/`, `ort-bundle/`, salidas de `package`, binario y objetos del motor, cobertura y pesos locales heredados bajo `vendor/qwen3-tts` | `xtask` |
//! | Aplicación | `--app` | Instalación y estado del usuario | Delegado en `self uninstall --yes` (o `cleanup --all --yes` si el canal es `homebrew`), ejecutado con el binario del repositorio |
//! | Ambas | `--all` | Aplicación y después repositorio | Ídem |
//!
//! La capa global compartida (`~/.cargo`, caché de sccache, paquetes del
//! sistema, MSYS2) nunca se toca (criterio 23); solo se informa. Detiene
//! primero los daemons lanzados desde `target/`, nunca borra código fuente
//! versionado y aplica las reglas de confirmación (`--dry-run`, `--yes`
//! obligatorio sin terminal). En Windows, el `xtask.exe` en ejecución se
//! borra con el mismo mecanismo de borrado diferido que el producto.
//!
//! Las rutas de la capa app viven en `avi-shared` y las ejecuta el binario
//! del repositorio: este módulo no replica ni una (fuente única del ciclo 4).

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
    // Pesos legados: el motor los prefiere (dir hermano del binario) sobre el
    // snapshot HF de `setup`, enmascarando el modelo pinneado.
    "vendor/qwen3-tts/qwen3-tts-0.6b",
    "vendor/qwen3-tts/qwen3-tts-0.6b-base",
];

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

/// Tamaño en bytes de una ruta (recursivo, sin seguir symlinks).
fn size_of(path: &Path) -> u64 {
    let Ok(meta) = path.symlink_metadata() else {
        return 0;
    };
    if !meta.is_dir() {
        return meta.len();
    }
    std::fs::read_dir(path)
        .map(|rd| rd.flatten().map(|e| size_of(&e.path())).sum())
        .unwrap_or(0)
}

pub(crate) fn human_size(bytes: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    const GB: f64 = MB * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.1} GB", b / GB)
    } else {
        format!("{:.1} MB", b / MB)
    }
}

/// Líneas del listado con su tamaño: directorios uno por línea; ficheros
/// sueltos agrupados por (directorio, extensión) cuando hay 3 o más.
fn summarize(paths: &[PathBuf]) -> Vec<(String, u64)> {
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
            let size = members.iter().map(|p| size_of(p)).sum();
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

fn run_quiet(bin: &Path, args: &[&str]) {
    let _ = std::process::Command::new(bin)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
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

/// Punto de entrada de `xtask clean`.
pub fn run(layer: Layer, dry_run: bool, yes: bool) -> Result<()> {
    let root = std::env::current_dir()?;
    if !root.join("Cargo.toml").is_file() || !root.join("crates").join("xtask").is_dir() {
        bail!("ejecuta `cargo xtask clean` desde la raíz del repositorio");
    }

    let repo_paths = if layer.wants_repo() {
        repo_targets(&root)
    } else {
        Vec::new()
    };
    let homebrew = if layer.wants_app() {
        installed_copy_is_homebrew()
    } else {
        false
    };

    let title = match layer {
        Layer::Repo => "Capa proyecto (repo)",
        Layer::App => "Capa app (perfil de usuario, delegada)",
        Layer::All => "Ambas capas (app y después repo)",
    };
    println!("{title}:");
    if layer.wants_repo() {
        if repo_paths.is_empty() {
            println!("  (nada en el repo)");
        }
        let mut total = 0u64;
        for (label, size) in summarize(&repo_paths) {
            total += size;
            println!("  {:>9}  {}", human_size(size), label);
        }
        println!("Total repo: {}", human_size(total));
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
                Layer::Repo => "repo",
                Layer::App => "app",
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

    let self_exe = std::env::current_exe().ok();
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
                let own = cfg!(windows)
                    && *p == target_dir
                    && self_exe
                        .as_ref()
                        .is_some_and(|x| x.starts_with(&target_dir));
                if own {
                    deferred_target = true;
                } else {
                    failed.push((p.clone(), e.to_string()));
                }
            }
        }
    }

    if deferred_target {
        spawn_deferred_removal(&target_dir)?;
        println!(
            "  … {} se termina de borrar al salir este proceso (helper en segundo plano)",
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

/// Borra `dir` tras la salida de este proceso y de `cargo` (reintentos
/// acotados), con un PowerShell desacoplado: mismo patrón que el helper de
/// `uninstall` del producto.
#[cfg(windows)]
fn spawn_deferred_removal(dir: &Path) -> Result<()> {
    use std::os::windows::process::CommandExt;
    let dir_literal = dir.to_string_lossy().replace('\'', "''");
    let script = format!(
        "Wait-Process -Id {pid} -ErrorAction SilentlyContinue; \
         for ($i = 0; $i -lt 20 -and (Test-Path -LiteralPath '{dir}'); $i++) {{ \
           Start-Sleep -Milliseconds 500; \
           Remove-Item -LiteralPath '{dir}' -Recurse -Force -ErrorAction SilentlyContinue \
         }}",
        pid = std::process::id(),
        dir = dir_literal
    );
    std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP: sobrevive a la salida de cargo.
        .creation_flags(0x00000008 | 0x00000200)
        .spawn()?;
    Ok(())
}

#[cfg(not(windows))]
fn spawn_deferred_removal(_dir: &Path) -> Result<()> {
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
        let root = std::env::temp_dir().join(format!("xtask_clean_repo_{}", std::process::id()));
        let engine = root.join("vendor/qwen3-tts");
        let ingot = engine.join("third_party/ingot/src");
        std::fs::create_dir_all(&ingot).unwrap();
        std::fs::create_dir_all(root.join("target/debug")).unwrap();
        std::fs::create_dir_all(engine.join("qwen3-tts-0.6b")).unwrap();
        std::fs::create_dir_all(engine.join("presets")).unwrap();
        for f in ["main.c", "main.o", "main.d", "Makefile"] {
            std::fs::write(engine.join(f), b"x").unwrap();
        }
        std::fs::write(ingot.join("q.o"), b"x").unwrap();
        std::fs::write(engine.join("third_party/ingot/libingot.a"), b"x").unwrap();

        let got = repo_targets(&root);
        let _ = std::fs::remove_dir_all(&root);

        for keep in ["main.c", "Makefile", "presets"] {
            assert!(!got.contains(&engine.join(keep)), "no debe borrar {}", keep);
        }
        for gone in [
            root.join("target"),
            engine.join("qwen3-tts-0.6b"),
            engine.join("main.o"),
            engine.join("main.d"),
            ingot.join("q.o"),
            engine.join("third_party/ingot/libingot.a"),
        ] {
            assert!(got.contains(&gone), "falta {}", gone.display());
        }
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

    #[test]
    fn human_size_units() {
        assert_eq!(human_size(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(human_size(3 * 1024 * 1024 * 1024 / 2), "1.5 GB");
    }
}
