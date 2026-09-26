//! `xtask clean`: devuelve la máquina del desarrollador a un estado limpio en
//! dos capas, para compilar, instalar o usar solo artefactos nuevos:
//!
//! - **Proyecto**: artefactos de build del repo (`target/`, motor TTS compilado,
//!   pesos legados bajo `vendor/qwen3-tts`, coverage, cachekeys de CI).
//! - **App**: estado del producto en el perfil del usuario (binario instalado vía
//!   su propio `uninstall`, `data_dir`, raíz de modelos, derivados CT2 y
//!   temporales del producto).
//!
//! No toca caches globales compartidas con otros proyectos (`~/.cargo/registry`,
//! `~/.cargo/git`, sccache) ni paquetes Python. Las rutas de la capa app replican
//! la resolución de `avi-store` sin depender de él (arrastraría el árbol TLS de
//! `hf-hub`); un test con `avi-store` como dev-dependency fija la paridad.
//!
//! **La réplica y sus tests desaparecen en el Ciclo 4**, cuando `xtask clean`
//! delegue en el binario y la fuente única pase a ser `avi-store` sin duplicado.
//! Hasta entonces no es una decisión permanente: es el coste de no compilar el
//! árbol TLS aquí.

use anyhow::{bail, Result};
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

const APP_NAME: &str = "ai-voice-interconnector";

/// Repos HF pinneados por el producto (espejo de `avi_store::MODEL_REVISIONS`).
pub(crate) const PINNED_REPOS: &[&str] = &[
    "Qwen/Qwen3-TTS-12Hz-0.6B-CustomVoice",
    "Qwen/Qwen3-TTS-12Hz-0.6B-Base",
    "istupakov/parakeet-tdt-0.6b-v3-onnx",
    "Helsinki-NLP/opus-mt-es-en",
    "Helsinki-NLP/opus-mt-en-es",
];

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

/// Prefijos de temporales del producto (mismo barrido que `cleanup`, más el
/// helper de `uninstall` en Windows). `avi-` y `avi_` son la fuente
/// (`avi_store::TEMP_PREFIXES`) y el único que cubre el borrado diferido
/// `avi-uninstall-<pid>-<ms>.ps1`; el tercero es el staging de los scripts de
/// instalación de la raíz, que desaparece con ellos en el Ciclo 3.
const TEMP_PREFIXES: &[&str] = &["avi_", "avi-", "ai-voice-interconnector-install-"];

/// Valor de una variable de entorno de reubicación, ignorando el vacío.
fn relocated_dir(var: &str) -> Option<PathBuf> {
    std::env::var(var)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .map(PathBuf::from)
}

fn home_dir() -> PathBuf {
    directories::UserDirs::new()
        .map(|d| d.home_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Réplica de la resolución de `%LOCALAPPDATA%` de `avi-store`.
fn local_app_data() -> PathBuf {
    relocated_dir("LOCALAPPDATA").unwrap_or_else(|| home_dir().join("AppData").join("Local"))
}

/// Réplica de `avi_store::cache_home`.
fn cache_home() -> PathBuf {
    relocated_dir("XDG_CACHE_HOME").unwrap_or_else(|| home_dir().join(".cache"))
}

/// Réplica de `avi_store::data_dir`.
pub(crate) fn data_dir() -> PathBuf {
    if let Some(dir) = relocated_dir("AVI_DATA_DIR") {
        return dir;
    }
    if cfg!(windows) {
        local_app_data().join(APP_NAME).join("data")
    } else {
        directories::ProjectDirs::from("", "", APP_NAME)
            .map(|d| d.data_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from(".").join(APP_NAME))
    }
}

/// Réplica de `avi_store::install_dir`.
fn install_dir() -> PathBuf {
    if let Some(dir) = relocated_dir("AVI_INSTALL_DIR") {
        return dir;
    }
    if cfg!(windows) {
        local_app_data().join("Programs").join(APP_NAME)
    } else {
        home_dir().join(".local").join("opt").join(APP_NAME)
    }
}

/// Réplica de `avi_store::bin_dir`. En Windows no hay enlace: la entrada que se
/// escribe en `HKCU\Environment\Path` es el propio directorio de programa.
fn bin_dir() -> PathBuf {
    if let Some(dir) = relocated_dir("AVI_BIN_DIR") {
        return dir;
    }
    if cfg!(windows) {
        install_dir()
    } else {
        home_dir().join(".local").join("bin")
    }
}

/// Réplica de `avi_store::shared_hf_root`: la caché HF que el usuario eligió
/// compartir, o `None` si no eligió ninguna.
pub(crate) fn shared_hf_root() -> Option<PathBuf> {
    if let Some(dir) = relocated_dir("HF_HUB_CACHE") {
        return Some(dir);
    }
    relocated_dir("HF_HOME").map(|home| home.join("hub"))
}

/// Réplica de `avi_store::models_root_is_shared`.
pub(crate) fn models_root_is_shared() -> bool {
    shared_hf_root().is_some()
}

/// Réplica de `avi_store::models_cache_dir`: raíz de modelos **exclusiva** de la
/// aplicación por defecto, y la caché HF compartida del usuario si definió
/// `HF_HUB_CACHE` o `HF_HOME`.
pub(crate) fn models_cache_dir() -> PathBuf {
    if let Some(dir) = relocated_dir("AVI_CACHE_DIR") {
        return dir;
    }
    if let Some(shared) = shared_hf_root() {
        return shared;
    }
    if cfg!(windows) {
        local_app_data().join(APP_NAME).join("cache").join("models")
    } else if cfg!(target_os = "macos") {
        home_dir()
            .join("Library")
            .join("Caches")
            .join(APP_NAME)
            .join("models")
    } else {
        cache_home().join(APP_NAME).join("models")
    }
}

/// Réplica de `avi_store::xet_cache_dir`: `xet` cuelga de la raíz de modelos
/// cuando es exclusiva, y sigue la cadena real de `xet-runtime` cuando el
/// usuario eligió una caché HF compartida.
///
/// Ya no la consume la capa app: `xet` cae con la raíz de modelos entera cuando
/// es exclusiva, y R3 lo prohíbe cuando es compartida. Sobrevive solo como
/// fixture del test de paridad, que es lo que fija que ambas copias no divergen
/// antes de que C4 elimine la réplica.
#[cfg(test)]
pub(crate) fn xet_cache_dir() -> PathBuf {
    if models_root_is_shared() {
        if let Some(dir) = relocated_dir("HF_XET_CACHE") {
            return dir;
        }
        if let Some(home) = relocated_dir("HF_HOME") {
            return home.join("xet");
        }
        return cache_home().join("huggingface").join("xet");
    }
    models_cache_dir().join("xet")
}

/// Directorio de instalación y comando instalado por plataforma: los directorios
/// que `clean` barre de la capa app y la ruta del comando, que es el binario en
/// Windows y el enlace simbólico en Unix.
fn install_layout() -> (Vec<PathBuf>, Option<PathBuf>) {
    let dir = install_dir();
    if cfg!(windows) {
        (
            vec![dir.clone()],
            Some(dir.join(format!("{}.exe", APP_NAME))),
        )
    } else {
        let link = bin_dir().join(APP_NAME);
        (vec![dir, link.clone()], Some(link))
    }
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

/// Rutas de la capa app que existen.
pub(crate) fn app_targets() -> Vec<PathBuf> {
    let models = models_cache_dir();
    let mut out: Vec<PathBuf> = install_layout().0;
    out.push(data_dir());
    if models_root_is_shared() {
        // R3: en una raíz compartida solo alcanzan los repos fijados, sus
        // derivados y sus locks. `xet` y el `.locks` completo son globales de la
        // caché HF y quedan vivos aunque alojen shards de la aplicación.
        for repo in PINNED_REPOS {
            let repo_dir = format!("models--{}", repo.replace('/', "--"));
            out.push(models.join(&repo_dir));
            out.push(models.join(".locks").join(&repo_dir));
        }
        out.push(models.join("ct2"));
    } else {
        // Raíz exclusiva: el directorio entero es de la aplicación, `xet` y el
        // derivado CT2 incluidos, así que basta con borrarlo de una vez.
        out.push(models);
    }
    out.retain(|p| p.symlink_metadata().is_ok());
    out.extend(temp_targets(&std::env::temp_dir()));
    out
}

/// Temporales del producto en `tmp`: prefijos propios y `*.qvoice` sueltos
/// (el `voice clone` local de versiones previas los dejaba sin borrar).
pub(crate) fn temp_targets(tmp: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(tmp) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            TEMP_PREFIXES.iter().any(|p| name.starts_with(p)) || name.ends_with(".qvoice")
        })
        .map(|e| e.path())
        .collect()
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

fn run_quiet(bin: &Path, args: &[&str]) {
    let _ = std::process::Command::new(bin)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

/// Punto de entrada de `xtask clean`.
pub fn run(dry_run: bool, yes: bool) -> Result<()> {
    let root = std::env::current_dir()?;
    if !root.join("Cargo.toml").is_file() || !root.join("crates").join("xtask").is_dir() {
        bail!("ejecuta `cargo run -p xtask -- clean` desde la raíz del repositorio");
    }
    let layers = [
        ("Capa proyecto (repo)", repo_targets(&root)),
        ("Capa app (perfil de usuario)", app_targets()),
    ];

    let mut total = 0u64;
    for (title, paths) in &layers {
        println!("{}:", title);
        if paths.is_empty() {
            println!("  (nada)");
        }
        for (label, size) in summarize(paths) {
            total += size;
            println!("  {:>9}  {}", human_size(size), label);
        }
    }
    println!("Total: {}", human_size(total));
    if dry_run {
        println!("Dry-run: no se borró nada.");
        return Ok(());
    }
    if layers.iter().all(|(_, p)| p.is_empty()) {
        println!("Nada que limpiar.");
        return Ok(());
    }

    if !yes {
        if !std::io::stdin().is_terminal() {
            bail!("sin TTY: confirma con --yes (o revisa antes con --dry-run)");
        }
        print!("¿Borrar todo lo listado? [s/N]: ");
        std::io::stdout().flush()?;
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        let t = input.trim().to_lowercase();
        if !matches!(t.as_str(), "s" | "si" | "sí" | "y" | "yes") {
            println!("Cancelado.");
            return Ok(());
        }
    }

    // Parar daemons y desinstalar con el propio producto: libera ficheros
    // bloqueados y revierte PATH/registro, que un borrado de ficheros no cubre.
    let (install_dirs, installed_bin) = install_layout();
    if let Some(bin) = installed_bin.filter(|b| b.is_file()) {
        println!("Desinstalando {} ...", bin.display());
        // `self uninstall --yes --json`: el comando de nivel superior `uninstall` y su
        // `--force` desaparecen en el ciclo de vida, sin alias. La capa de aplicación de
        // esta tarea sigue funcionando porque invoca el comando nuevo, y **su
        // sustitución por delegación real es del Ciclo 4**, donde `xtask clean` dejará de
        // lanzar el binario del producto y reutilizará el motor como biblioteca.
        run_quiet(&bin, &["self", "uninstall", "--yes", "--json"]);
        // En Windows el install_dir lo borra un helper desacoplado tras la salida.
        for _ in 0..20 {
            if install_dirs.iter().all(|d| d.symlink_metadata().is_err()) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
    }
    for bin in repo_product_bins(&root) {
        run_quiet(&bin, &["daemon", "stop", "--json"]);
    }

    let self_exe = std::env::current_exe().ok();
    let target_dir = root.join("target");
    let mut failed: Vec<(PathBuf, String)> = Vec::new();
    let mut deferred_target = false;
    for (_, paths) in &layers {
        for p in paths {
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

    /// La réplica de pines y este test desaparecen con la réplica de rutas en el
    /// Ciclo 4. Hasta entonces son la única garantía de que ambas copias de la
    /// lista no divergen.
    #[test]
    fn pinned_repos_match_avi_store() {
        let store: std::collections::BTreeSet<&str> = avi_store::MODEL_REVISIONS
            .iter()
            .map(|(_, r, _)| *r)
            .collect();
        let ours: std::collections::BTreeSet<&str> = PINNED_REPOS.iter().copied().collect();
        assert_eq!(ours, store);
    }

    #[test]
    fn app_paths_match_avi_store() {
        assert_eq!(data_dir(), avi_store::data_dir());
        assert_eq!(models_cache_dir(), avi_store::models_cache_dir());
        assert_eq!(shared_hf_root(), avi_store::shared_hf_root());
        assert_eq!(xet_cache_dir(), avi_store::xet_cache_dir());
        assert_eq!(models_cache_dir().join("ct2"), avi_store::ct2_cache_dir());
        assert_eq!(install_dir(), avi_store::install_dir());
        assert_eq!(bin_dir(), avi_store::bin_dir());
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

    #[test]
    fn temp_targets_match_product_prefixes_only() {
        let tmp = std::env::temp_dir().join(format!("xtask_clean_tmp_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        for f in [
            "avi_daemon_clone_x_1.wav",
            "avi-uninstall-1-2.ps1",
            "ai-voice-interconnector-install-abc",
            "maria.qvoice",
            "otro_programa.tmp",
        ] {
            std::fs::write(tmp.join(f), b"x").unwrap();
        }
        let mut got: Vec<String> = temp_targets(&tmp)
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        got.sort();
        let _ = std::fs::remove_dir_all(&tmp);
        assert_eq!(
            got,
            [
                "ai-voice-interconnector-install-abc",
                "avi-uninstall-1-2.ps1",
                "avi_daemon_clone_x_1.wav",
                "maria.qvoice",
            ]
        );
    }

    #[test]
    fn human_size_units() {
        assert_eq!(human_size(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(human_size(3 * 1024 * 1024 * 1024 / 2), "1.5 GB");
    }
}
