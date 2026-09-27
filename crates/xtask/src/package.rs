//! `xtask package`: monta el bundle del release para el target del host (§10.4).
//!
//! Lee `packaging/bundle-manifest.json` como lista canónica (la misma que valida
//! `self install`): empaquetado e instalación no pueden divergir. Resuelve ONNX
//! Runtime reutilizando `ort-bundle/` si está fresco para la versión fijada
//! en `packaging/pins.json` (fuente única, espejo de `ort_version` en
//! `.circleci/config.yml:58-60`), o con descarga verificada si no. Valida `--expect-version` (decisión c: la
//! puerta tag-versión vive aquí, ningún paso YAML la duplica), pasa el humo
//! (`version` y `voice list`) y comprime de forma determinista con el nombre de
//! `release_asset_name` (`target.rs:90-94`).
//!
//! Sin red en los tests: la descarga vive en `ensure_ort_bundle` y todo lo demás
//! (manifiesto, nombres, puerta de versión, montaje, compresión) es puro o solo
//! toca temporales.

use anyhow::{bail, Result};
use avi_shared::manifest::{parse_manifest_text, BundleTarget};
use avi_shared::pins;
use std::path::{Path, PathBuf};

/// Versión de ONNX Runtime empaquetada (pareja del crate `ort` en uso). Se lee
/// de `packaging/pins.json` (fuente única del ciclo 4, espejo del parámetro
/// `ort_version` de `.circleci/config.yml:58-60`): si el pipeline la sube, el
/// pin sube con ella o el bundle reutilizado no coincidirá con el que espera
/// el motor. Sin réplica en el código.
fn load_ort_version(root: &Path) -> Result<String> {
    pins::load_from_root(root)
        .map(|p| p.ort)
        .map_err(|e| anyhow::anyhow!("{e}"))
}

/// Manifiesto canónico, relativo a la raíz del repositorio.
const MANIFEST_REL: &str = "packaging/bundle-manifest.json";

/// Caché local del bundle de ONNX Runtime, relativa a la raíz.
const ORT_BUNDLE_REL: &str = "ort-bundle";

/// Marcador de versión dentro de `ort-bundle/`: lo escribe una descarga de este
/// comando. Un bundle sin marcador pero con las librerías (el que restaura la
/// caché de CI, que no lo escribe) se considera fresco.
const ORT_MARKER: &str = ".ort-version";

/// Marca temporal fija de los archivos comprimidos (2020-01-01 UTC): con
/// entradas ordenadas y metadatos fijos, el mismo árbol produce el mismo
/// archivo y el `SHA256SUMS.txt` del release es reproducible.
const ARCHIVE_MTIME: u64 = 1_577_836_800;

/// DLL del runtime de VC++ que la librería oficial de Windows necesita (la DLL
/// de Microsoft es /MD). Réplica de la lista del job `build-windows-x64`.
const VC_DLLS: &[&str] = &["vcruntime140.dll", "vcruntime140_1.dll", "msvcp140.dll"];

/// Une un fragmento del manifiesto (siempre con `/`) con una raíz, con el
/// separador de la plataforma.
fn relative_path(root: &Path, relative: &str) -> PathBuf {
    let mut path = root.to_path_buf();
    for part in relative.split('/') {
        path.push(part);
    }
    path
}

/// Triple del host según la tabla de §3, derivado de la plataforma de
/// compilación (este binario siempre corre donde se compiló).
pub(crate) fn host_triple() -> Result<&'static str> {
    match (std::env::consts::ARCH, std::env::consts::OS) {
        ("x86_64", "windows") => Ok("x86_64-pc-windows-msvc"),
        ("x86_64", "linux") => Ok("x86_64-unknown-linux-gnu"),
        ("aarch64", "linux") => Ok("aarch64-unknown-linux-gnu"),
        ("aarch64", "macos") => Ok("aarch64-apple-darwin"),
        (arch, os) => bail!(
            "la plataforma de este host ({arch}-{os}) no está en la tabla de §3: compila desde el código fuente (docs/BUILD.md)"
        ),
    }
}

/// Raíz de arquitectura de la convención de §3 (`aarch64-*` → `arm64`).
fn release_arch(triple: &str) -> Result<&'static str> {
    match triple {
        "x86_64-pc-windows-msvc" | "x86_64-unknown-linux-gnu" => Ok("x86_64"),
        "aarch64-unknown-linux-gnu" | "aarch64-apple-darwin" => Ok("arm64"),
        other => bail!("triple no soportado: {other}"),
    }
}

/// Etiqueta de SO y extensión del archivo de release de un triple.
fn release_os(triple: &str) -> Result<(&'static str, &'static str)> {
    match triple {
        "x86_64-pc-windows-msvc" => Ok(("windows", "zip")),
        "x86_64-unknown-linux-gnu" | "aarch64-unknown-linux-gnu" => Ok(("linux", "tar.gz")),
        "aarch64-apple-darwin" => Ok(("macos", "tar.gz")),
        other => bail!("triple no soportado: {other}"),
    }
}

/// Nombre convencional del archivo de release (`target.rs:90-94`):
/// `ai-voice-interconnector-<ver>-<arch>-<os>.<ext>`, sin la `v` del tag.
fn release_asset_name(triple: &str, version: &str) -> Result<String> {
    let arch = release_arch(triple)?;
    let (os, ext) = release_os(triple)?;
    Ok(format!(
        "ai-voice-interconnector-{version}-{arch}-{os}.{ext}"
    ))
}

/// Puerta tag-versión (decisión c): fail-fast si la versión del CLI no es la
/// esperada por quien invoca (en CI, `${CIRCLE_TAG#v}`).
fn check_expect_version(actual: &str, expect: Option<&str>) -> Result<()> {
    if let Some(want) = expect {
        let want = want.strip_prefix('v').unwrap_or(want);
        if actual != want {
            bail!("la versión del CLI ({actual}) no coincide con la esperada ({want}): aborta el empaquetado");
        }
    }
    Ok(())
}

/// Archivo y URL de descarga de ONNX Runtime para el triple (nombres oficiales
/// de los releases de Microsoft, los mismos que usan los cuatro jobs de build).
fn ort_download(triple: &str, ort_version: &str) -> Result<(String, String)> {
    let file = match triple {
        "x86_64-pc-windows-msvc" => format!("onnxruntime-win-x64-{ort_version}.zip"),
        "x86_64-unknown-linux-gnu" => format!("onnxruntime-linux-x64-{ort_version}.tgz"),
        "aarch64-unknown-linux-gnu" => format!("onnxruntime-linux-aarch64-{ort_version}.tgz"),
        "aarch64-apple-darwin" => format!("onnxruntime-osx-arm64-{ort_version}.tgz"),
        other => bail!("triple no soportado: {other}"),
    };
    let url =
        format!("https://github.com/microsoft/onnxruntime/releases/download/v{ort_version}/{file}");
    Ok((file, url))
}

/// Librerías que el bundle debe aportar por triple: la que el crate `ort` busca
/// en `load-dynamic` junto al ejecutable, más el runtime de VC++ en Windows.
fn ort_expected_libs(triple: &str) -> Result<Vec<&'static str>> {
    let mut libs = match triple {
        _ if triple.contains("-windows-") => vec!["onnxruntime.dll"],
        _ if triple.contains("-apple-") => vec!["libonnxruntime.dylib"],
        _ if triple.contains("-linux-") => vec!["libonnxruntime.so"],
        _ => bail!("triple no soportado: {triple}"),
    };
    if triple.contains("-windows-") {
        libs.extend(VC_DLLS);
    }
    Ok(libs)
}

/// `true` si `ort-bundle/` ya sirve para este triple: todas las librerías
/// presentes y marcador ausente (caché de CI) o coincidente con la versión de
/// los pines.
fn ort_bundle_is_fresh(bundle: &Path, triple: &str, ort_version: &str) -> Result<bool> {
    for lib in ort_expected_libs(triple)? {
        if !bundle.join(lib).is_file() {
            return Ok(false);
        }
    }
    let marker = bundle.join(ORT_MARKER);
    if marker.is_file() {
        let pinned = std::fs::read_to_string(&marker).unwrap_or_default();
        if pinned.trim() != ort_version {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Descarga un archivo por HTTPS con `curl` (o `Invoke-WebRequest` en Windows
/// si `curl` falta), como los bootstrap: TLS ≥ 1.2 y fallo ante HTTP erróneo.
fn download_https(url: &str, dest: &Path) -> Result<()> {
    if std::process::Command::new("curl")
        .args([
            "--proto",
            "=https",
            "--tlsv1.2",
            "-fsSL",
            "-o",
            &dest.to_string_lossy(),
            url,
        ])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
    {
        return Ok(());
    }
    if cfg!(windows) {
        let dest_arg = dest.to_string_lossy().to_string();
        let status = std::process::Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                &format!(
                    "[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12; \
                     Invoke-WebRequest -Uri '{url}' -OutFile '{dest_arg}' -UseBasicParsing"
                ),
            ])
            .status()
            .map_err(|e| anyhow::anyhow!("no se pudo descargar {url}: {e}"))?;
        if status.success() {
            return Ok(());
        }
    }
    bail!("no se pudo descargar {url} (¿curl instalado y red disponible?)")
}

/// Extrae la librería de ONNX Runtime del archivo descargado a `bundle/` con su
/// nombre canónico (el que el crate `ort` busca en `load-dynamic`). Verificada:
/// falla si el archivo no trae la librería esperada.
fn extract_ort_library(
    archive: &Path,
    bundle: &Path,
    triple: &str,
    ort_version: &str,
) -> Result<()> {
    let canonical = if triple.contains("-windows-") {
        "onnxruntime.dll"
    } else if triple.contains("-apple-") {
        "libonnxruntime.dylib"
    } else {
        "libonnxruntime.so"
    };
    let pattern = if triple.contains("-linux-") {
        "libonnxruntime.so."
    } else {
        canonical
    };
    let found: Option<Vec<u8>> = if archive.extension().and_then(|e| e.to_str()) == Some("zip") {
        let file = std::fs::File::open(archive)?;
        let mut zip = zip::ZipArchive::new(file)?;
        let mut hit: Option<Vec<u8>> = None;
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i)?;
            if entry.is_dir() {
                continue;
            }
            let name = entry.name().rsplit('/').next().unwrap_or("").to_string();
            if name == canonical && hit.is_none() {
                let mut bytes = Vec::new();
                std::io::Read::read_to_end(&mut entry, &mut bytes)?;
                hit = Some(bytes);
            }
        }
        hit
    } else {
        let file = std::fs::File::open(archive)?;
        let gz = flate2::read::GzDecoder::new(file);
        let mut tar = tar::Archive::new(gz);
        let mut hit: Option<Vec<u8>> = None;
        for entry in tar.entries()? {
            let mut entry = entry?;
            let path = entry.path()?.to_string_lossy().to_string();
            let name = path.rsplit('/').next().unwrap_or("").to_string();
            if (name == canonical || name.starts_with(pattern)) && hit.is_none() {
                let mut bytes = Vec::new();
                std::io::Read::read_to_end(&mut entry, &mut bytes)?;
                hit = Some(bytes);
            }
        }
        hit
    };
    match found {
        Some(bytes) => {
            std::fs::create_dir_all(bundle)?;
            std::fs::write(bundle.join(canonical), bytes)?;
            Ok(())
        }
        None => bail!(
            "el archivo {} no trae la librería {canonical} de ONNX Runtime {ort_version}",
            archive.display()
        ),
    }
}

/// Copia las DLL del runtime de VC++ desde System32 al bundle (app-local, como
/// el job `build-windows-x64`: el usuario no instala el Redistributable).
#[cfg(windows)]
fn copy_vc_runtime(bundle: &Path) -> Result<()> {
    let system32 = Path::new(r"C:\Windows\System32");
    for dll in VC_DLLS {
        let src = system32.join(dll);
        if !src.is_file() {
            bail!("no se encontró {dll} en System32");
        }
        std::fs::copy(&src, bundle.join(dll))?;
    }
    Ok(())
}

/// Asegura `ort-bundle/` para el triple: lo reutiliza si está fresco o lo
/// reconstruye con descarga verificada en caso contrario. Lo reutiliza
/// `bootstrap` (decisión (d)): un solo descargador verificado en el crate.
pub(crate) fn ensure_ort_bundle(root: &Path, triple: &str, ort_version: &str) -> Result<PathBuf> {
    let bundle = root.join(ORT_BUNDLE_REL);
    if bundle.is_dir() && ort_bundle_is_fresh(&bundle, triple, ort_version)? {
        eprintln!("Bundle ONNX Runtime reutilizado de {ORT_BUNDLE_REL}");
        return Ok(bundle);
    }
    if bundle.is_dir() {
        std::fs::remove_dir_all(&bundle)?;
    }
    std::fs::create_dir_all(&bundle)?;
    let (file, url) = ort_download(triple, ort_version)?;
    let dest = std::env::temp_dir().join(format!("avi-ort-{ort_version}-{file}"));
    eprintln!("Descargando ONNX Runtime {ort_version}: {url}");
    download_https(&url, &dest)?;
    extract_ort_library(&dest, &bundle, triple, ort_version)?;
    std::fs::remove_file(&dest).ok();
    #[cfg(windows)]
    copy_vc_runtime(&bundle)?;
    std::fs::write(bundle.join(ORT_MARKER), ort_version)?;
    if !ort_bundle_is_fresh(&bundle, triple, ort_version)? {
        bail!("el bundle de ONNX Runtime quedó incompleto tras la descarga");
    }
    Ok(bundle)
}

/// Orígenes en disco de cada entrada del manifiesto.
struct Sources {
    binary: PathBuf,
    engine: PathBuf,
}

/// Localiza el binario ya compilado, el motor vendido y los cuatro documentos.
/// Va antes de la descarga de ONNX Runtime para fallar rápido sin red si falta
/// lo que el propio repo debe aportar.
fn locate_sources(root: &Path) -> Result<Sources> {
    let exe_suffix = std::env::consts::EXE_SUFFIX;
    let binary = root
        .join("target")
        .join("release")
        .join(format!("ai-voice-interconnector{exe_suffix}"));
    if !binary.is_file() {
        bail!(
            "binario no encontrado: {} (compila antes con `cargo build --release --features full`)",
            binary.display()
        );
    }
    let engine_name = format!("qwen_tts{exe_suffix}");
    let engine = root.join("vendor").join("qwen3-tts").join(&engine_name);
    if !engine.is_file() {
        bail!(
            "motor TTS no encontrado: {} (compila antes con `cargo run -p xtask -- build-engine`)",
            engine.display()
        );
    }
    for doc in [
        "LICENSE",
        "THIRD-PARTY-LICENSES.md",
        "SOURCE-OFFER.md",
        "README.md",
    ] {
        if !root.join(doc).is_file() {
            bail!("documento no encontrado: {}", root.join(doc).display());
        }
    }
    Ok(Sources { binary, engine })
}

/// Humo del binario ya compilado: `version` y `voice list`, como el smoke de CI.
fn smoke_binary(binary: &Path) -> Result<()> {
    for args in [vec!["version"], vec!["voice", "list"]] {
        let status = std::process::Command::new(binary)
            .args(&args)
            .status()
            .map_err(|e| anyhow::anyhow!("no se pudo ejecutar {}: {e}", binary.display()))?;
        if !status.success() {
            bail!(
                "el humo falló: {} {} (exit {:?})",
                binary.display(),
                args.join(" "),
                status.code()
            );
        }
    }
    Ok(())
}

/// Monta el árbol del bundle en `stage/`: cada entrada de `required` con su
/// origen, 0755 para ejecutables y 0644 para el resto en Unix.
fn stage_bundle(
    section: &BundleTarget,
    sources: &Sources,
    root: &Path,
    bundle: &Path,
    stage: &Path,
) -> Result<()> {
    let engine_rel = format!("vendor/qwen3-tts/qwen_tts{}", std::env::consts::EXE_SUFFIX);
    for relative in &section.required {
        let dest = relative_path(stage, relative);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let src = if *relative == section.executable {
            sources.binary.clone()
        } else if *relative == engine_rel {
            sources.engine.clone()
        } else if root.join(relative).is_file() {
            root.join(relative)
        } else {
            bundle.join(relative)
        };
        std::fs::copy(&src, &dest).map_err(|e| {
            anyhow::anyhow!(
                "no se pudo copiar {} a {}: {e}",
                src.display(),
                dest.display()
            )
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let executable = *relative == section.executable || *relative == engine_rel;
            let mode = if executable { 0o755 } else { 0o644 };
            std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(mode))?;
        }
    }
    // El árbol montado debe coincidir con la lista que `self install` valida.
    let mut staged: Vec<String> = Vec::new();
    collect_relative(stage, stage, &mut staged)?;
    staged.sort();
    let mut required = section.required.clone();
    required.sort();
    if staged != required {
        bail!(
            "el árbol montado no coincide con el manifiesto (montado: {staged:?}, manifiesto: {required:?})"
        );
    }
    Ok(())
}

/// Rutas relativas (con `/`) de todo lo que hay bajo `dir`.
fn collect_relative(base: &Path, dir: &Path, out: &mut Vec<String>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_relative(base, &path, out)?;
        } else {
            let rel = path
                .strip_prefix(base)
                .map_err(|e| anyhow::anyhow!("ruta fuera del bundle: {e}"))?
                .to_string_lossy()
                .replace('\\', "/");
            out.push(rel);
        }
    }
    Ok(())
}

/// Entradas ordenadas del árbol (rutas relativas con `/`), para comprimir de
/// forma determinista.
fn sorted_entries(stage: &Path) -> Result<Vec<String>> {
    let mut entries = Vec::new();
    collect_relative(stage, stage, &mut entries)?;
    entries.sort();
    Ok(entries)
}

/// Comprime el árbol a `.zip` (Windows) con entradas ordenadas, modos Unix
/// fijos (0755 ejecutables, 0644 resto) y marca temporal fija.
fn compress_zip(stage: &Path, dest: &Path, section: &BundleTarget) -> Result<()> {
    let engine_rel = format!("vendor/qwen3-tts/qwen_tts{}", std::env::consts::EXE_SUFFIX);
    let file = std::fs::File::create(dest)?;
    let mut zip = zip::ZipWriter::new(file);
    let stamp = zip::DateTime::from_date_and_time(2020, 1, 1, 0, 0, 0)
        .map_err(|e| anyhow::anyhow!("marca temporal inválida: {e:?}"))?;
    let mut dirs: Vec<String> = Vec::new();
    for relative in sorted_entries(stage)? {
        if let Some(parent) = Path::new(&relative).parent() {
            let parent = parent.to_string_lossy().replace('\\', "/");
            if !parent.is_empty() && !dirs.contains(&parent) {
                dirs.push(parent);
            }
        }
        let mode = if relative == section.executable || relative == engine_rel {
            0o755
        } else {
            0o644
        };
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .unix_permissions(mode)
            .last_modified_time(stamp);
        zip.start_file(&relative, options)?;
        let bytes = std::fs::read(relative_path(stage, &relative))?;
        std::io::Write::write_all(&mut zip, &bytes)?;
    }
    for dir in dirs {
        let options = zip::write::SimpleFileOptions::default()
            .unix_permissions(0o755)
            .last_modified_time(stamp);
        zip.add_directory(format!("{dir}/"), options)?;
    }
    zip.finish()?;
    Ok(())
}

/// Comprime el árbol a `.tar.gz` (Unix) con entradas ordenadas y cabeceras
/// fijas (uid/gid 0, mtime fija, modos 0755/0644).
fn compress_tar_gz(stage: &Path, dest: &Path, section: Option<&BundleTarget>) -> Result<()> {
    let engine_rel = format!("vendor/qwen3-tts/qwen_tts{}", std::env::consts::EXE_SUFFIX);
    let file = std::fs::File::create(dest)?;
    let gz = flate2::write::GzEncoder::new(file, flate2::Compression::default());
    let mut tar = tar::Builder::new(gz);
    tar.mode(tar::HeaderMode::Deterministic);
    for relative in sorted_entries(stage)? {
        let src = relative_path(stage, &relative);
        let meta = std::fs::metadata(&src)?;
        let executable =
            section.is_some_and(|s| relative == s.executable || relative == engine_rel);
        let mut header = tar::Header::new_gnu();
        header.set_size(meta.len());
        header.set_mtime(ARCHIVE_MTIME);
        header.set_uid(0);
        header.set_gid(0);
        header.set_mode(if executable { 0o755 } else { 0o644 });
        header.set_cksum();
        let file = std::fs::File::open(&src)?;
        tar.append_data(&mut header, Path::new(&relative), file)?;
    }
    let gz = tar.into_inner()?;
    gz.finish()?;
    Ok(())
}

/// Punto de entrada de `xtask package`.
pub fn run(out: Option<PathBuf>, no_compress: bool, expect_version: Option<String>) -> Result<()> {
    let root = std::env::current_dir()?;
    let manifest_path = root.join(MANIFEST_REL);
    if !manifest_path.is_file() {
        bail!(
            "ejecuta `cargo run -p xtask -- package` desde la raíz del repositorio (falta {MANIFEST_REL})"
        );
    }
    let version = super::get_version()?;
    check_expect_version(&version, expect_version.as_deref())?;
    let ort_version = load_ort_version(&root)?;
    let text = std::fs::read_to_string(&manifest_path)?;
    let sections = parse_manifest_text(&text).map_err(|e| anyhow::anyhow!("{e}"))?;
    let triple = host_triple()?;
    let section = sections.get(triple).ok_or_else(|| {
        anyhow::anyhow!("el manifiesto de bundle no tiene sección para el target {triple}")
    })?;
    let sources = locate_sources(&root)?;
    smoke_binary(&sources.binary)?;
    let bundle = ensure_ort_bundle(&root, triple, &ort_version)?;
    for lib in ort_expected_libs(triple)? {
        if !bundle.join(lib).is_file() {
            bail!("librería no encontrada en {}: {lib}", bundle.display());
        }
    }
    let stage = std::env::temp_dir().join(format!("avi-package-{triple}-{}", std::process::id()));
    if stage.is_dir() {
        std::fs::remove_dir_all(&stage)?;
    }
    std::fs::create_dir_all(&stage)?;
    let result = stage_bundle(section, &sources, &root, &bundle, &stage);
    if let Err(e) = result {
        std::fs::remove_dir_all(&stage).ok();
        return Err(e);
    }
    let out_dir = out.unwrap_or_else(|| root.join("artifacts"));
    std::fs::create_dir_all(&out_dir)?;
    let asset = release_asset_name(triple, &version)?;
    if no_compress {
        let stem = asset
            .trim_end_matches(".tar.gz")
            .trim_end_matches(".zip")
            .to_string();
        let dest = out_dir.join(&stem);
        if dest.is_dir() {
            std::fs::remove_dir_all(&dest)?;
        }
        copy_tree(&stage, &dest)?;
        std::fs::remove_dir_all(&stage).ok();
        println!("Bundle sin comprimir: {}", dest.display());
        return Ok(());
    }
    let dest = out_dir.join(&asset);
    if triple.contains("-windows-") {
        compress_zip(&stage, &dest, section)?;
    } else {
        compress_tar_gz(&stage, &dest, Some(section))?;
    }
    std::fs::remove_dir_all(&stage).ok();
    println!("Artefacto: {}", dest.display());
    Ok(())
}

/// Copia recursiva de un árbol (para `--no-compress`).
fn copy_tree(src: &Path, dest: &Path) -> Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dest.join(entry.file_name());
        if from.is_dir() {
            copy_tree(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Manifiesto mínimo de juguete para los tests (sin red ni disco del repo).
    const TOY_MANIFEST: &str = r#"{
        "targets": {
            "x86_64-pc-windows-msvc": {
                "executable": "ai-voice-interconnector.exe",
                "required": ["ai-voice-interconnector.exe", "LICENSE", "onnxruntime.dll"]
            },
            "x86_64-unknown-linux-gnu": {
                "executable": "ai-voice-interconnector",
                "required": ["ai-voice-interconnector", "LICENSE", "libonnxruntime.so"]
            }
        }
    }"#;

    /// El manifiesto canónico cubre el target del host y cada sección es
    /// coherente: ejecutable entre los obligatorios, sin duplicados.
    #[test]
    fn manifest_covers_host_target() {
        let text = include_str!("../../../packaging/bundle-manifest.json");
        let sections = parse_manifest_text(text).unwrap();
        assert_eq!(sections.len(), 4, "una sección por cada target de §3");
        let triple = host_triple().unwrap();
        let section = sections.get(triple).unwrap();
        assert!(section.required.contains(&section.executable));
        let mut unique = section.required.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), section.required.len(), "sin duplicados");
    }

    /// El árbol montado desde orígenes falsos coincide con `required`.
    #[test]
    fn staged_layout_matches_required() {
        let sections = parse_manifest_text(TOY_MANIFEST).unwrap();
        let triple = if cfg!(windows) {
            "x86_64-pc-windows-msvc"
        } else {
            "x86_64-unknown-linux-gnu"
        };
        let section = sections.get(triple).unwrap();
        let tag = format!("xtask_package_layout_{}", std::process::id());
        let base = std::env::temp_dir().join(&tag);
        let src = base.join("src");
        let stage = base.join("stage");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join(&section.executable), b"binario").unwrap();
        std::fs::write(src.join("LICENSE"), b"licencia").unwrap();
        let lib = if cfg!(windows) {
            "onnxruntime.dll"
        } else {
            "libonnxruntime.so"
        };
        std::fs::write(src.join(lib), b"ort").unwrap();
        let sources = Sources {
            binary: src.join(&section.executable),
            engine: src.join(&section.executable),
        };
        // Orígenes de juguete en `src/`, documentos simulados en la raíz.
        let root = src.clone();
        stage_bundle(section, &sources, &root, &src, &stage).unwrap();
        let mut staged = Vec::new();
        collect_relative(&stage, &stage, &mut staged).unwrap();
        staged.sort();
        let mut required = section.required.clone();
        required.sort();
        assert_eq!(staged, required);
        std::fs::remove_dir_all(&base).ok();
    }

    /// `--expect-version` divergente falla y el coincidente pasa (con y sin `v`).
    #[test]
    fn expect_version_gate() {
        check_expect_version("0.24.0", None).unwrap();
        check_expect_version("0.24.0", Some("0.24.0")).unwrap();
        check_expect_version("0.24.0", Some("v0.24.0")).unwrap();
        let err = check_expect_version("0.24.0", Some("0.25.0")).unwrap_err();
        assert!(err.to_string().contains("0.25.0"));
    }

    /// El nombre del artefacto sigue `release_asset_name` en los cuatro targets.
    #[test]
    fn asset_name_per_target() {
        let cases = [
            (
                "x86_64-pc-windows-msvc",
                "ai-voice-interconnector-0.24.0-x86_64-windows.zip",
            ),
            (
                "x86_64-unknown-linux-gnu",
                "ai-voice-interconnector-0.24.0-x86_64-linux.tar.gz",
            ),
            (
                "aarch64-unknown-linux-gnu",
                "ai-voice-interconnector-0.24.0-arm64-linux.tar.gz",
            ),
            (
                "aarch64-apple-darwin",
                "ai-voice-interconnector-0.24.0-arm64-macos.tar.gz",
            ),
        ];
        for (triple, name) in cases {
            assert_eq!(release_asset_name(triple, "0.24.0").unwrap(), name);
        }
        assert!(release_asset_name("i686-unknown-linux-gnu", "0.24.0").is_err());
    }

    /// Versión de ONNX fijada en `pins.json`: los tests la leen de la fuente
    /// única, como el comando, en vez de replicarla.
    fn test_ort_version() -> String {
        pins::parse_text(include_str!("../../../packaging/pins.json"))
            .expect("pins.json válido")
            .ort
    }

    /// La descarga de ONNX Runtime apunta a los archivos oficiales por triple.
    #[test]
    fn ort_download_per_target() {
        let ort_version = test_ort_version();
        let (file, url) = ort_download("x86_64-pc-windows-msvc", &ort_version).unwrap();
        assert_eq!(file, format!("onnxruntime-win-x64-{ort_version}.zip"));
        assert!(url.ends_with(&file) && url.contains(&ort_version));
        let (file, _) = ort_download("aarch64-apple-darwin", &ort_version).unwrap();
        assert_eq!(file, format!("onnxruntime-osx-arm64-{ort_version}.tgz"));
        assert!(ort_expected_libs("x86_64-pc-windows-msvc")
            .unwrap()
            .contains(&"onnxruntime.dll"));
        assert!(ort_expected_libs("x86_64-unknown-linux-gnu")
            .unwrap()
            .contains(&"libonnxruntime.so"));
    }

    /// Un `ort-bundle/` con las librerías y marcador coincidente se reutiliza;
    /// con marcador divergente o librerías ausentes, no.
    #[test]
    fn ort_bundle_freshness() {
        let ort_version = test_ort_version();
        let triple = if cfg!(windows) {
            "x86_64-pc-windows-msvc"
        } else {
            "x86_64-unknown-linux-gnu"
        };
        let base = std::env::temp_dir().join(format!("xtask_package_ort_{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        for lib in ort_expected_libs(triple).unwrap() {
            std::fs::write(base.join(lib), b"x").unwrap();
        }
        assert!(ort_bundle_is_fresh(&base, triple, &ort_version).unwrap());
        std::fs::write(base.join(ORT_MARKER), "0.0.0").unwrap();
        assert!(!ort_bundle_is_fresh(&base, triple, &ort_version).unwrap());
        std::fs::write(base.join(ORT_MARKER), &ort_version).unwrap();
        assert!(ort_bundle_is_fresh(&base, triple, &ort_version).unwrap());
        std::fs::remove_file(base.join(ort_expected_libs(triple).unwrap()[0])).unwrap();
        assert!(!ort_bundle_is_fresh(&base, triple, &ort_version).unwrap());
        std::fs::remove_dir_all(&base).ok();
    }

    /// Compresión determinista: dos pasadas sobre el mismo árbol dan el mismo
    /// archivo con exactamente las entradas del manifiesto.
    #[test]
    fn compression_is_deterministic() {
        let base = std::env::temp_dir().join(format!("xtask_package_zip_{}", std::process::id()));
        let stage = base.join("stage");
        std::fs::create_dir_all(stage.join("vendor/qwen3-tts")).unwrap();
        std::fs::write(stage.join("ai-voice-interconnector"), b"bin").unwrap();
        std::fs::write(stage.join("LICENSE"), b"lic").unwrap();
        std::fs::write(stage.join("vendor/qwen3-tts/qwen_tts"), b"motor").unwrap();
        let first = base.join("first.tar.gz");
        let second = base.join("second.tar.gz");
        compress_tar_gz(&stage, &first, None).unwrap();
        compress_tar_gz(&stage, &second, None).unwrap();
        assert_eq!(
            std::fs::read(&first).unwrap(),
            std::fs::read(&second).unwrap()
        );
        let file = std::fs::File::open(&first).unwrap();
        let gz = flate2::read::GzDecoder::new(file);
        let mut tar = tar::Archive::new(gz);
        let mut names: Vec<String> = tar
            .entries()
            .unwrap()
            .map(|e| {
                e.unwrap()
                    .path()
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        names.sort();
        assert_eq!(
            names,
            [
                "LICENSE",
                "ai-voice-interconnector",
                "vendor/qwen3-tts/qwen_tts"
            ]
        );
        std::fs::remove_dir_all(&base).ok();
    }
}
