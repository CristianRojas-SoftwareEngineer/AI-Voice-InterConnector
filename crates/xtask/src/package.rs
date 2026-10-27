// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! `xtask package`: monta el bundle del release para el target del host.
//!
//! Lee `packaging/bundle-manifest.json` como lista canónica (la misma que valida
//! `self install`): empaquetado e instalación no pueden divergir. Resuelve ONNX
//! Runtime reutilizando `ort-bundle/` si está fresco para la versión fijada
//! en `packaging/pins.json` (fuente única, espejo del parámetro `ort_pin` de
//! la CI), o descargándolo si no: el archivo descargado debe tener el SHA-256
//! que fija `pins.json` para el target y traer la librería esperada. Resuelve
//! FFmpeg del mismo modo con `ffmpeg-bundle/`, verifica su SHA-256, exige los
//! códecs de la matriz de egressión y ejecuta `-version` antes de montarlo.
//! Valida `--expect-version` (la puerta tag-versión vive aquí, ningún paso YAML
//! la duplica), pasa el humo (`version` y `voice list`) y comprime de forma
//! determinista con el nombre de `release_asset_name`.
//!
//! Sin red en los tests: las descargas viven en `ensure_ort_bundle` y
//! `ensure_ffmpeg_bundle`; todo lo demás (manifiesto, nombres, puerta de
//! versión, hash, montaje, compresión) es puro o solo toca temporales.

use anyhow::{bail, Result};
use avi_shared::manifest::{parse_manifest_text, BundleTarget};
use avi_shared::pins::{self, Pins};
use std::io::Seek;
use std::path::{Path, PathBuf};

/// Manifiesto canónico, relativo a la raíz del repositorio.
pub(crate) const MANIFEST_REL: &str = "packaging/bundle-manifest.json";

/// Caché local del bundle de ONNX Runtime, relativa a la raíz.
const ORT_BUNDLE_REL: &str = "ort-bundle";

/// Caché local del binario de FFmpeg, relativa a la raíz.
const FFMPEG_BUNDLE_REL: &str = "ffmpeg-bundle";

/// Marcador del SHA-256 del artefacto dentro de `ffmpeg-bundle/`: un cambio de
/// pin invalida el bundle aunque la versión nominal no cambie.
const FFMPEG_MARKER: &str = ".ffmpeg-sha256";

/// Release inmutable de BtbN del que salen los binarios Linux: el tag
/// `latest` se mueve, así que la URL y el hash fijados apuntan aquí.
const FFMPEG_BTBN_TAG: &str = "autobuild-2026-10-24-13-02";

/// Build concreto de FFmpeg dentro de [`FFMPEG_BTBN_TAG`].
const FFMPEG_BTBN_BUILD: &str = "n9.0.2-35-g9a9b70bb8c";

/// Marcador de versión dentro de `ort-bundle/`: lo escribe una descarga de este
/// comando y el job de CI que arma el bundle. Un bundle sin marcador tiene una
/// versión desconocida y no se reutiliza.
const ORT_MARKER: &str = ".ort-version";

/// Marca temporal fija de los archivos comprimidos (2020-01-01 UTC): con
/// entradas ordenadas y metadatos fijos, el mismo árbol produce el mismo
/// archivo y el `SHA256SUMS.txt` del release es reproducible.
const ARCHIVE_MTIME: u64 = 1_577_836_800;

/// DLL del runtime de VC++ que viajan junto al binario en Windows (despliegue
/// app-local, que la licencia del Redistributable permite): las cuatro que
/// necesita la librería oficial de ONNX Runtime (es /MD) y `vcomp140.dll`, el
/// runtime OpenMP de MSVC que el binario importa porque CTranslate2 se compila
/// con `/openmp`. Ninguna forma parte de Windows: sin ellas el binario o ONNX
/// Runtime no cargan en un sistema sin el Redistributable.
const VC_DLLS: &[&str] = &[
    "vcruntime140.dll",
    "vcruntime140_1.dll",
    "msvcp140.dll",
    "msvcp140_1.dll",
    "vcomp140.dll",
];

/// Une un fragmento del manifiesto (siempre con `/`) con una raíz, con el
/// separador de la plataforma.
fn relative_path(root: &Path, relative: &str) -> PathBuf {
    let mut path = root.to_path_buf();
    for part in relative.split('/') {
        path.push(part);
    }
    path
}

/// Triple del host según la tabla de targets soportados, derivado de la plataforma de
/// compilación (este binario siempre corre donde se compiló).
pub(crate) fn host_triple() -> Result<&'static str> {
    match (std::env::consts::ARCH, std::env::consts::OS) {
        ("x86_64", "windows") => Ok("x86_64-pc-windows-msvc"),
        ("x86_64", "linux") => Ok("x86_64-unknown-linux-gnu"),
        ("aarch64", "linux") => Ok("aarch64-unknown-linux-gnu"),
        ("aarch64", "macos") => Ok("aarch64-apple-darwin"),
        (arch, os) => bail!(
            "la plataforma de este host ({arch}-{os}) no está soportada: compila desde el código fuente (docs/BUILD.md)"
        ),
    }
}

/// Raíz de arquitectura de la convención de nombres (`aarch64-*` → `arm64`).
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

/// Nombre convencional del archivo de release, el que fija la tabla de targets:
/// `ai-voice-interconnector-<ver>-<arch>-<os>.<ext>`, sin la `v` del tag.
fn release_asset_name(triple: &str, version: &str) -> Result<String> {
    let arch = release_arch(triple)?;
    let (os, ext) = release_os(triple)?;
    Ok(format!(
        "ai-voice-interconnector-{version}-{arch}-{os}.{ext}"
    ))
}

/// Puerta tag-versión: falla de inmediato si la versión del CLI no es la
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

/// SHA-256 fijado en `pins.json` para el asset de ONNX Runtime del triple: 64
/// dígitos hexadecimales en minúsculas. Un triple sin entrada o un valor mal
/// formado es un error, nunca una descarga sin verificar.
fn pinned_ort_sha256<'a>(pins: &'a Pins, triple: &str) -> Result<&'a str> {
    pinned_sha256(&pins.ort_sha256, "ONNX Runtime", triple)
}

/// SHA-256 fijado en `pins.json` para el artefacto de FFmpeg del triple.
fn pinned_ffmpeg_sha256<'a>(pins: &'a Pins, triple: &str) -> Result<&'a str> {
    pinned_sha256(&pins.ffmpeg_sha256, "FFmpeg", triple)
}

/// Valida que `pins.json` fije un SHA-256 bien formado para el target: 64
/// dígitos hexadecimales en minúsculas. Un triple sin entrada o un valor mal
/// formado es un error, nunca una descarga sin verificar.
fn pinned_sha256<'a>(
    hashes: &'a std::collections::BTreeMap<String, String>,
    artifact: &str,
    triple: &str,
) -> Result<&'a str> {
    let Some(hash) = hashes.get(triple) else {
        bail!("packaging/pins.json no fija el SHA-256 de {artifact} para {triple}");
    };
    if hash.len() != 64
        || !hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        bail!("el SHA-256 de {artifact} para {triple} en packaging/pins.json no son 64 dígitos hexadecimales en minúsculas: {hash:?}");
    }
    Ok(hash)
}

/// Archivo y URL de descarga de FFmpeg para el triple. Linux usa un release
/// inmutable de los builds GPL de BtbN y macOS el build de Martin Riedl;
/// Windows, el essentials de Gyan. El pin de versión manda en todos los casos.
///
/// Windows se descarga del release que el propio autor del build publica en
/// GitHub, no de su sitio de paquetes: ese sitio degrada las transferencias
/// sostenidas hasta volver inútil bajar 110 MB, mientras que el release los
/// sirve a velocidad de CDN. Los dos orígenes publican el mismo fichero, y
/// quien lo decide no es la URL sino el SHA-256 fijado, que se verifica sobre
/// el archivo descargado antes de extraer nada: un origen que sirviera otro
/// binario no llegaría al bundle.
fn ffmpeg_download(triple: &str, version: &str) -> Result<(String, String)> {
    let major: String = version.split('.').take(2).collect::<Vec<_>>().join(".");
    let (file, url) = match triple {
        "x86_64-pc-windows-msvc" => (
            format!("ffmpeg-{version}-essentials_build.zip"),
            format!(
                "https://github.com/GyanD/codexffmpeg/releases/download/{version}/ffmpeg-{version}-essentials_build.zip"
            ),
        ),
        "x86_64-unknown-linux-gnu" => {
            let file = format!("ffmpeg-{FFMPEG_BTBN_BUILD}-linux64-gpl-{major}.tar.xz");
            let url = format!(
                "https://github.com/BtbN/FFmpeg-Builds/releases/download/{FFMPEG_BTBN_TAG}/{file}"
            );
            (file, url)
        }
        "aarch64-unknown-linux-gnu" => {
            let file = format!("ffmpeg-{FFMPEG_BTBN_BUILD}-linuxarm64-gpl-{major}.tar.xz");
            let url = format!(
                "https://github.com/BtbN/FFmpeg-Builds/releases/download/{FFMPEG_BTBN_TAG}/{file}"
            );
            (file, url)
        }
        "aarch64-apple-darwin" => (
            "ffmpeg.zip".to_string(),
            format!("https://ffmpeg.martin-riedl.de/download/macos/arm64/1789931890_{version}/ffmpeg.zip"),
        ),
        other => bail!("triple no soportado: {other}"),
    };
    Ok((file, url))
}

/// Nombre del ejecutable de FFmpeg dentro del bundle para el triple.
fn ffmpeg_binary_name(triple: &str) -> Result<&'static str> {
    if triple.contains("-windows-") {
        Ok("ffmpeg.exe")
    } else if triple.contains("-linux-") || triple.contains("-apple-") {
        Ok("ffmpeg")
    } else {
        bail!("triple no soportado: {triple}")
    }
}

/// SHA-256 en hexadecimal minúsculo de unos bytes.
fn hex_sha256(bytes: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Falla si el fichero descargado no tiene el SHA-256 fijado: un asset
/// alterado, truncado o de otra versión no llega al bundle.
fn verify_sha256(path: &Path, expected: &str) -> Result<()> {
    let bytes = std::fs::read(path)
        .map_err(|e| anyhow::anyhow!("no se pudo leer {}: {e}", path.display()))?;
    let actual = hex_sha256(&bytes);
    if actual != expected {
        bail!(
            "el SHA-256 de {} no coincide con el fijado en packaging/pins.json (esperado {expected}, obtenido {actual})",
            path.display()
        );
    }
    Ok(())
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

/// Ruta de la librería de ONNX Runtime dentro de `bundle` para el triple, la
/// que `ort` carga en `load-dynamic` cuando se la indica `ORT_DYLIB_PATH`.
pub(crate) fn ort_dylib_path(bundle: &Path, triple: &str) -> Result<PathBuf> {
    let libs = ort_expected_libs(triple)?;
    Ok(bundle.join(libs[0]))
}

/// `true` si `ort-bundle/` ya sirve para este triple: todas las librerías
/// presentes y marcador existente cuya versión coincide con la de los pines.
/// Un marcador o una librería ausentes dejan el bundle sin servir; un fichero
/// que existe pero no se puede leer es un error.
fn ort_bundle_is_fresh(bundle: &Path, triple: &str, ort_version: &str) -> Result<bool> {
    for lib in ort_expected_libs(triple)? {
        // Existencia y tamaño: un archivo ausente o vacío (descarga truncada)
        // no es fresco y fuerza la reconstrucción; cualquier otro error de
        // lectura se propaga.
        let path = bundle.join(lib);
        match path.metadata() {
            Ok(meta) if meta.len() > 0 => {}
            Ok(_) => return Ok(false),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(anyhow::anyhow!("no se pudo leer {}: {e}", path.display())),
        }
    }
    let marker = bundle.join(ORT_MARKER);
    match std::fs::read_to_string(&marker) {
        Ok(pinned) => Ok(pinned.trim() == ort_version),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(anyhow::anyhow!(
            "no se pudo leer el marcador {}: {e}",
            marker.display()
        )),
    }
}

/// Escapa una cadena para un literal PowerShell entre comillas simples.
fn powershell_single_quoted(text: &str) -> String {
    text.replace('\'', "''")
}

/// Descarga un archivo por HTTPS con `curl`, como los bootstrap: TLS ≥ 1.2 y
/// fallo ante HTTP erróneo. Solo en Windows y solo si `curl` no existe se usa
/// `Invoke-WebRequest`; cualquier otro fallo de `curl` se propaga.
///
/// Estos artefactos pesan más de 100 MB, así que la transferencia no puede
/// quedarse esperando a un origen que deja de enviar sin cerrar la conexión:
/// eso dejaría el paso colgado hasta el límite del job, sin mensaje que lo
/// explique. De ahí el detector de estancamiento —aborta si la velocidad cae
/// por debajo de 1 KB/s durante un minuto— y los reintentos ante error de
/// conexión o de transferencia. Un reintento vuelve a bajar el archivo desde el
/// principio; el SHA-256 fijado se verifica después sobre el fichero completo,
/// así que reescribirlo no compromete nada: o coincide con el pin, o el
/// empaquetado falla.
fn download_https(url: &str, dest: &Path) -> Result<()> {
    let curl = std::process::Command::new("curl")
        .args([
            "--proto",
            "=https",
            "--tlsv1.2",
            "-fsSL",
            "--connect-timeout",
            "15",
            "--retry",
            "3",
            "--retry-delay",
            "2",
            "--retry-connrefused",
            "--speed-limit",
            "1024",
            "--speed-time",
            "60",
            "--max-time",
            "600",
            "-o",
            &dest.to_string_lossy(),
            url,
        ])
        .status();
    match curl {
        Ok(status) if status.success() => return Ok(()),
        Ok(status) => bail!("curl falló al descargar {url}: {status}"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && cfg!(windows) => {}
        Err(e) => bail!("no se pudo ejecutar curl para descargar {url}: {e}"),
    }
    let dest_arg = dest.to_string_lossy().to_string();
    let status = std::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &format!(
                "[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12; \
                 Invoke-WebRequest -Uri '{}' -OutFile '{}' -UseBasicParsing",
                powershell_single_quoted(url),
                powershell_single_quoted(&dest_arg)
            ),
        ])
        .status()
        .map_err(|e| anyhow::anyhow!("no se pudo descargar {url}: {e}"))?;
    if status.success() {
        return Ok(());
    }
    bail!("Invoke-WebRequest falló al descargar {url}: {status}")
}

/// Prioridad de una entrada del archivo como candidata a librería: el nombre
/// exacto gana a la versión, y a igualdad la más específica (más larga).
/// Tamaño cero nunca vale: los assets de Microsoft listan primero los enlaces
/// simbólicos (`libonnxruntime.so.1 -> .so.1.28.0`) y quedarse con la primera
/// coincidencia escribe una librería vacía que todo lo demás acepta.
fn rank_lib_candidate(
    canonical: &str,
    pattern: &str,
    name: &str,
    size: u64,
) -> Option<(u8, usize)> {
    if size == 0 {
        return None;
    }
    if name == canonical {
        Some((2, name.len()))
    } else if name.starts_with(pattern) {
        Some((1, name.len()))
    } else {
        None
    }
}

/// Extrae la librería de ONNX Runtime del archivo descargado a `bundle/` con su
/// nombre canónico (el que el crate `ort` busca en `load-dynamic`). Falla si el
/// archivo no trae la librería esperada con tamaño mayor que cero.
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
        let mut best: Option<((u8, usize), usize)> = None;
        for i in 0..zip.len() {
            let entry = zip.by_index(i)?;
            if entry.is_dir() {
                continue;
            }
            let name = entry.name().rsplit('/').next().unwrap_or("").to_string();
            if let Some(rank) = rank_lib_candidate(canonical, pattern, &name, entry.size()) {
                if best.is_none_or(|(r, _)| rank > r) {
                    best = Some((rank, i));
                }
            }
        }
        match best {
            Some((_, i)) => {
                let mut entry = zip.by_index(i)?;
                let mut bytes = Vec::new();
                std::io::Read::read_to_end(&mut entry, &mut bytes)?;
                Some(bytes)
            }
            None => None,
        }
    } else {
        let file = std::fs::File::open(archive)?;
        let gz = flate2::read::GzDecoder::new(file);
        let mut tar = tar::Archive::new(gz);
        let mut best_rank: Option<(u8, usize)> = None;
        let mut hit: Option<Vec<u8>> = None;
        for entry in tar.entries()? {
            let mut entry = entry?;
            let path = entry.path()?.to_string_lossy().to_string();
            let name = path.rsplit('/').next().unwrap_or("").to_string();
            let rank = rank_lib_candidate(canonical, pattern, &name, entry.size());
            if rank.is_some_and(|r| best_rank.is_none_or(|b| r > b)) {
                let mut bytes = Vec::new();
                std::io::Read::read_to_end(&mut entry, &mut bytes)?;
                best_rank = rank;
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
/// reconstruye descargándolo en caso contrario. La descarga va por HTTPS, su
/// SHA-256 debe coincidir con el fijado en `pins.json` y el archivo debe traer
/// la librería esperada con tamaño mayor que cero; el marcador de versión solo
/// se escribe tras esas comprobaciones, así que un bundle fresco procede de
/// una descarga verificada. Lo reutilizan `bootstrap`, la gate de pruebas con
/// recursos y la CI: un solo descargador en el repositorio.
pub(crate) fn ensure_ort_bundle(root: &Path, triple: &str, pins: &Pins) -> Result<PathBuf> {
    let ort_version = pins.ort.as_str();
    let expected_sha256 = pinned_ort_sha256(pins, triple)?;
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
    // Archivo descargado con dueño: se borra al salir, también si falla la
    // descarga o la extracción.
    let dest = tempfile::Builder::new()
        .prefix(&format!("xtask-ort-{ort_version}-"))
        .suffix(&format!("-{file}"))
        .tempfile()?
        .into_temp_path();
    eprintln!("Descargando ONNX Runtime {ort_version}: {url}");
    download_https(&url, &dest)?;
    verify_sha256(&dest, expected_sha256)?;
    extract_ort_library(&dest, &bundle, triple, ort_version)?;
    #[cfg(windows)]
    copy_vc_runtime(&bundle)?;
    std::fs::write(bundle.join(ORT_MARKER), ort_version)?;
    if !ort_bundle_is_fresh(&bundle, triple, ort_version)? {
        bail!("el bundle de ONNX Runtime quedó incompleto tras la descarga");
    }
    Ok(bundle)
}

/// `true` si `ffmpeg-bundle/` ya sirve para el triple: el ejecutable presente
/// con tamaño mayor que cero y el marcador con el SHA-256 del artefacto fijado.
fn ffmpeg_bundle_is_fresh(bundle: &Path, triple: &str, expected_sha256: &str) -> Result<bool> {
    let binary = bundle.join(ffmpeg_binary_name(triple)?);
    let marker = bundle.join(FFMPEG_MARKER);
    if !binary.is_file() || !marker.is_file() {
        return Ok(false);
    }
    let size = std::fs::metadata(&binary)?.len();
    if size == 0 {
        return Ok(false);
    }
    Ok(std::fs::read_to_string(marker)?.trim() == expected_sha256)
}

/// Extrae el ejecutable de FFmpeg del archivo descargado. En zip se busca por
/// nombre; en `.tar.xz` se descomprime a un temporal y se recorre el tar. Falla
/// si no aparece el ejecutable esperado.
fn extract_ffmpeg_binary(archive: &Path, triple: &str) -> Result<Vec<u8>> {
    let wanted = ffmpeg_binary_name(triple)?;
    let name = archive.to_string_lossy();
    if name.ends_with(".zip") {
        let file = std::fs::File::open(archive)?;
        let mut zip = zip::ZipArchive::new(file)?;
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i)?;
            if entry.is_dir() {
                continue;
            }
            let entry_name = entry.name().rsplit('/').next().unwrap_or("");
            if entry_name == wanted {
                let mut bytes = Vec::new();
                std::io::Read::read_to_end(&mut entry, &mut bytes)?;
                return Ok(bytes);
            }
        }
    } else if name.ends_with(".tar.xz") {
        let file = std::fs::File::open(archive)?;
        let mut reader = std::io::BufReader::new(file);
        let mut tar_file = tempfile::Builder::new()
            .prefix("xtask-ffmpeg-")
            .tempfile()?;
        lzma_rs::xz_decompress(&mut reader, tar_file.as_file_mut())?;
        tar_file.as_file_mut().seek(std::io::SeekFrom::Start(0))?;
        let mut tar = tar::Archive::new(tar_file.as_file_mut());
        for entry in tar.entries()? {
            let mut entry = entry?;
            let path = entry.path()?.to_string_lossy().to_string();
            let entry_name = path.rsplit('/').next().unwrap_or("");
            if entry_name == wanted {
                let mut bytes = Vec::new();
                std::io::Read::read_to_end(&mut entry, &mut bytes)?;
                return Ok(bytes);
            }
        }
    } else {
        bail!(
            "formato de archivo FFmpeg no soportado: {}",
            archive.display()
        );
    }
    bail!(
        "el archivo de FFmpeg no trae el ejecutable {wanted}: {}",
        archive.display()
    )
}

/// Directorio canónico del bundle de FFmpeg dentro del repositorio.
fn ffmpeg_bundle_dir(root: &Path) -> PathBuf {
    root.join(FFMPEG_BUNDLE_REL)
}

/// Punto de entrada de `xtask ffmpeg`: aprovisiona en `dest` el FFmpeg fijado
/// en `packaging/pins.json` para el target del host —la misma operación que
/// `xtask package` hace antes de montar el bundle, con la misma verificación de
/// SHA-256 y el mismo humo— y publica en la salida estándar la ruta del
/// ejecutable resultante, para que quien invoque el subcomando no tenga que
/// adivinar dónde quedó.
pub fn run_ffmpeg(dest: Option<&Path>) -> Result<()> {
    let root = std::env::current_dir()?;
    let pins = pins::load_from_root(&root).map_err(|e| anyhow::anyhow!("{e}"))?;
    let triple = host_triple()?;
    let bundle = match dest {
        Some(dir) => dir.to_path_buf(),
        None => ffmpeg_bundle_dir(&root),
    };
    let bundle = ensure_ffmpeg_bundle(&bundle, triple, &pins)?;
    println!("{}", bundle.join(ffmpeg_binary_name(triple)?).display());
    Ok(())
}

/// Asegura el bundle de FFmpeg en `bundle` para el triple: reutiliza el binario
/// si está fresco o lo reconstruye desde la URL fijada, verificando el SHA-256
/// antes de extraerlo. El humo `-version` corre sobre el binario extraído y el
/// marcador solo se escribe después, así que un bundle fresco procede de una
/// descarga verificada y ejecutable.
fn ensure_ffmpeg_bundle(bundle: &Path, triple: &str, pins: &Pins) -> Result<PathBuf> {
    let version = pins.ffmpeg.as_str();
    let expected_sha256 = pinned_ffmpeg_sha256(pins, triple)?;
    if bundle.is_dir() && ffmpeg_bundle_is_fresh(bundle, triple, expected_sha256)? {
        let binary = bundle.join(ffmpeg_binary_name(triple)?);
        if smoke_ffmpeg(&binary).is_ok() {
            eprintln!("Bundle FFmpeg reutilizado de {}", bundle.display());
            return Ok(bundle.to_path_buf());
        }
        eprintln!("El bundle FFmpeg en caché no supera el humo; se reconstruye");
    }
    if bundle.is_dir() {
        std::fs::remove_dir_all(bundle)?;
    }
    std::fs::create_dir_all(bundle)?;
    let (file, url) = ffmpeg_download(triple, version)?;
    let dest = tempfile::Builder::new()
        .prefix(&format!("xtask-ffmpeg-{version}-"))
        .suffix(&format!("-{file}"))
        .tempfile()?
        .into_temp_path();
    eprintln!("Descargando FFmpeg {version}: {url}");
    download_https(&url, &dest)?;
    verify_sha256(&dest, expected_sha256)?;
    let bytes = extract_ffmpeg_binary(&dest, triple)?;
    if bytes.is_empty() {
        bail!("el ejecutable de FFmpeg extraído está vacío");
    }
    let binary = bundle.join(ffmpeg_binary_name(triple)?);
    std::fs::write(&binary, &bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755))?;
    }
    smoke_ffmpeg(&binary)?;
    std::fs::write(bundle.join(FFMPEG_MARKER), expected_sha256)?;
    if !ffmpeg_bundle_is_fresh(bundle, triple, expected_sha256)? {
        bail!("el bundle de FFmpeg quedó incompleto tras la descarga");
    }
    Ok(bundle.to_path_buf())
}

/// Humo mínimo del FFmpeg extraído: debe ejecutarse, reportar su versión y
/// anunciar los codificadores que exige la matriz de egresión.
fn smoke_ffmpeg(binary: &Path) -> Result<()> {
    let version = std::process::Command::new(binary)
        .arg("-version")
        .output()
        .map_err(|e| anyhow::anyhow!("no se pudo ejecutar {}: {e}", binary.display()))?;
    let stdout = String::from_utf8_lossy(&version.stdout);
    if !version.status.success() || !stdout.contains("ffmpeg version") {
        bail!(
            "el humo de FFmpeg falló: {} -version (exit {:?})",
            binary.display(),
            version.status.code()
        );
    }
    let encoders = std::process::Command::new(binary)
        .args(["-hide_banner", "-encoders"])
        .output()
        .map_err(|e| anyhow::anyhow!("no se pudo ejecutar {} -encoders: {e}", binary.display()))?;
    let listed = String::from_utf8_lossy(&encoders.stdout);
    if !encoders.status.success() {
        bail!(
            "el humo de FFmpeg falló: {} -encoders (exit {:?})",
            binary.display(),
            encoders.status.code()
        );
    }
    for codec in [
        "libmp3lame",
        "libvorbis",
        "libopus",
        "aac",
        "flac",
        "pcm_s16le",
    ] {
        if !listed.contains(codec) {
            bail!(
                "el FFmpeg de {} no incluye el codificador {codec}",
                binary.display()
            );
        }
    }
    Ok(())
}

/// Orígenes en disco de cada entrada del manifiesto.
struct Sources {
    binary: PathBuf,
    engine: PathBuf,
}

/// Localiza el binario ya compilado, el motor vendido y los cinco documentos.
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
            "motor TTS no encontrado: {} (compila antes con `cargo xtask build-engine`)",
            engine.display()
        );
    }
    for doc in [
        "LICENSE",
        "NOTICE",
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

/// Humo del binario ya compilado: versión, voces y contrato de `speech denoise`.
fn smoke_binary(binary: &Path) -> Result<()> {
    for args in [
        vec!["version"],
        vec!["voice", "list"],
        vec!["speech", "denoise", "--help"],
    ] {
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
    let status = std::process::Command::new(binary)
        .arg("denoise")
        .arg("--help")
        .status()
        .map_err(|e| anyhow::anyhow!("no se pudo ejecutar {}: {e}", binary.display()))?;
    if status.success() {
        bail!(
            "el humo falló: {} aceptó el comando raíz `denoise`",
            binary.display()
        );
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
    ffmpeg: &Path,
    stage: &Path,
) -> Result<()> {
    let engine_rel = format!("vendor/qwen3-tts/qwen_tts{}", std::env::consts::EXE_SUFFIX);
    let ffmpeg_rel = format!("ffmpeg{}", std::env::consts::EXE_SUFFIX);
    for relative in &section.required {
        let dest = relative_path(stage, relative);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let src = if *relative == section.executable {
            sources.binary.clone()
        } else if *relative == engine_rel {
            sources.engine.clone()
        } else if *relative == ffmpeg_rel {
            ffmpeg.to_path_buf()
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
            let executable = *relative == section.executable
                || *relative == engine_rel
                || *relative == ffmpeg_rel;
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
    let ffmpeg_rel = format!("ffmpeg{}", std::env::consts::EXE_SUFFIX);
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
        let mode =
            if relative == section.executable || relative == engine_rel || relative == ffmpeg_rel {
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
    let ffmpeg_rel = format!("ffmpeg{}", std::env::consts::EXE_SUFFIX);
    let file = std::fs::File::create(dest)?;
    let gz = flate2::write::GzEncoder::new(file, flate2::Compression::default());
    let mut tar = tar::Builder::new(gz);
    tar.mode(tar::HeaderMode::Deterministic);
    for relative in sorted_entries(stage)? {
        let src = relative_path(stage, &relative);
        let meta = std::fs::metadata(&src)?;
        let executable = section
            .is_some_and(|s| relative == s.executable || relative == engine_rel)
            || relative == ffmpeg_rel;
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

/// Destino del árbol sin comprimir: la raíz de `--out` con `--flat` (layout de
/// staging) o un subdirectorio con el nombre del artefacto.
fn uncompressed_dest(out_dir: &Path, asset: &str, flat: bool) -> PathBuf {
    if flat {
        return out_dir.to_path_buf();
    }
    let stem = asset
        .trim_end_matches(".tar.gz")
        .trim_end_matches(".zip")
        .to_string();
    out_dir.join(&stem)
}

/// Punto de entrada de `xtask package`.
pub fn run(
    out: Option<PathBuf>,
    no_compress: bool,
    flat: bool,
    expect_version: Option<String>,
) -> Result<()> {
    if flat && !no_compress {
        bail!("`--flat` solo tiene sentido con `--no-compress`");
    }
    let root = std::env::current_dir()?;
    let manifest_path = root.join(MANIFEST_REL);
    if !manifest_path.is_file() {
        bail!("ejecuta `cargo xtask package` desde la raíz del repositorio (falta {MANIFEST_REL})");
    }
    let version = super::get_version()?;
    check_expect_version(&version, expect_version.as_deref())?;
    let pins = pins::load_from_root(&root).map_err(|e| anyhow::anyhow!("{e}"))?;
    let text = std::fs::read_to_string(&manifest_path)?;
    let sections = parse_manifest_text(&text).map_err(|e| anyhow::anyhow!("{e}"))?;
    let triple = host_triple()?;
    let section = sections.get(triple).ok_or_else(|| {
        anyhow::anyhow!("el manifiesto de bundle no tiene sección para el target {triple}")
    })?;
    let sources = locate_sources(&root)?;
    smoke_binary(&sources.binary)?;
    let bundle = ensure_ort_bundle(&root, triple, &pins)?;
    for lib in ort_expected_libs(triple)? {
        if !bundle.join(lib).is_file() {
            bail!("librería no encontrada en {}: {lib}", bundle.display());
        }
    }
    let ffmpeg_bundle = ensure_ffmpeg_bundle(&ffmpeg_bundle_dir(&root), triple, &pins)?;
    let ffmpeg_binary = ffmpeg_bundle.join(ffmpeg_binary_name(triple)?);
    if !ffmpeg_binary.is_file() {
        bail!(
            "ejecutable de FFmpeg no encontrado en {}",
            ffmpeg_binary.display()
        );
    }
    // Directorio de montaje con dueño: se borra al salir por cualquier camino.
    let stage_guard = tempfile::Builder::new()
        .prefix(&format!("xtask-package-{triple}-"))
        .tempdir()?;
    let stage = stage_guard.path().to_path_buf();
    stage_bundle(section, &sources, &root, &bundle, &ffmpeg_binary, &stage)?;
    // Ningún binario del bundle puede depender de algo que el sistema del
    // usuario no trae: se audita antes de publicar nada.
    super::deps::check_tree(&stage, section)?;
    // Si el binario enlaza `alsa-sys`, la lista de funciones de ALSA que
    // comprueba el audio debe ser exactamente la que ese binario alcanza: así no
    // se publica un ejecutable capaz de llamar a una función sin verificar.
    if super::alsa_symbols::linked_in(&root, triple)? {
        super::alsa_symbols::check_binary(&root, &stage.join(&section.executable))?;
    }
    let out_dir = out.unwrap_or_else(|| root.join("artifacts"));
    std::fs::create_dir_all(&out_dir)?;
    let asset = release_asset_name(triple, &version)?;
    if no_compress {
        let dest = uncompressed_dest(&out_dir, &asset, flat);
        if dest.is_dir() {
            std::fs::remove_dir_all(&dest)?;
        }
        copy_tree(&stage, &dest)?;
        println!("Bundle sin comprimir: {}", dest.display());
        return Ok(());
    }
    let dest = out_dir.join(&asset);
    if triple.contains("-windows-") {
        compress_zip(&stage, &dest, section)?;
    } else {
        compress_tar_gz(&stage, &dest, Some(section))?;
    }
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

    /// Manifiesto mínimo de fixture para los tests (sin red ni disco del repo).
    const FIXTURE_MANIFEST: &str = r#"{
        "targets": {
            "x86_64-pc-windows-msvc": {
                "executable": "ai-voice-interconnector.exe",
                "required": ["ai-voice-interconnector.exe", "LICENSE", "onnxruntime.dll", "ffmpeg.exe"]
            },
            "x86_64-unknown-linux-gnu": {
                "executable": "ai-voice-interconnector",
                "required": ["ai-voice-interconnector", "LICENSE", "libonnxruntime.so", "ffmpeg"]
            }
        }
    }"#;

    /// El manifiesto canónico cubre el target del host y cada sección es
    /// coherente: ejecutable entre los obligatorios, sin duplicados.
    #[test]
    fn manifest_covers_host_target() {
        let text = include_str!("../../../packaging/bundle-manifest.json");
        let sections = parse_manifest_text(text).unwrap();
        assert_eq!(sections.len(), 4, "una sección por cada target soportado");
        let triple = host_triple().unwrap();
        let section = sections.get(triple).unwrap();
        assert!(section.required.contains(&section.executable));
        let mut unique = section.required.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), section.required.len(), "sin duplicados");
    }

    /// La librería que la gate de release impone con `ORT_DYLIB_PATH` es la de
    /// ONNX Runtime del triple dentro del bundle, no el runtime de VC++.
    #[test]
    fn ort_dylib_path_points_to_the_onnx_runtime_library() {
        let bundle = Path::new("ort-bundle");
        for (triple, lib) in [
            ("x86_64-pc-windows-msvc", "onnxruntime.dll"),
            ("x86_64-unknown-linux-gnu", "libonnxruntime.so"),
            ("aarch64-apple-darwin", "libonnxruntime.dylib"),
        ] {
            assert_eq!(ort_dylib_path(bundle, triple).unwrap(), bundle.join(lib));
        }
        assert!(ort_dylib_path(bundle, "riscv64gc-unknown-none-elf").is_err());
    }

    /// El árbol montado desde orígenes falsos coincide con `required`.
    #[test]
    fn staged_layout_matches_required() {
        let sections = parse_manifest_text(FIXTURE_MANIFEST).unwrap();
        let triple = if cfg!(windows) {
            "x86_64-pc-windows-msvc"
        } else {
            "x86_64-unknown-linux-gnu"
        };
        let section = sections.get(triple).unwrap();
        let base_guard = tempfile::Builder::new()
            .prefix("xtask_package_layout_")
            .tempdir()
            .unwrap();
        let base = base_guard.path().to_path_buf();
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
        let ffmpeg = format!("ffmpeg{}", std::env::consts::EXE_SUFFIX);
        std::fs::write(src.join(&ffmpeg), b"ffmpeg").unwrap();
        let sources = Sources {
            binary: src.join(&section.executable),
            engine: src.join(&section.executable),
        };
        // Orígenes de fixture en `src/`, documentos simulados en la raíz.
        let root = src.clone();
        stage_bundle(section, &sources, &root, &src, &src.join(&ffmpeg), &stage).unwrap();
        let mut staged = Vec::new();
        collect_relative(&stage, &stage, &mut staged).unwrap();
        staged.sort();
        let mut required = section.required.clone();
        required.sort();
        assert_eq!(staged, required);
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

    /// Las comillas simples se duplican para no cerrar el literal de PowerShell.
    #[test]
    fn powershell_literal_escapes_single_quotes() {
        assert_eq!(powershell_single_quoted("a'b"), "a''b");
        assert_eq!(powershell_single_quoted("sin comillas"), "sin comillas");
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

    /// La descarga de FFmpeg apunta a los archivos oficiales por triple.
    #[test]
    fn ffmpeg_download_per_target() {
        let version = test_ffmpeg_version();
        let (file, url) = ffmpeg_download("x86_64-pc-windows-msvc", &version).unwrap();
        assert_eq!(file, format!("ffmpeg-{version}-essentials_build.zip"));
        assert!(url.ends_with(&file) && url.contains(&version));
        // Windows baja del release del build en GitHub y no del sitio de
        // paquetes del mismo autor, que degrada las transferencias sostenidas.
        assert!(url.starts_with("https://github.com/GyanD/codexffmpeg/releases/download/"));
        let (file, url) = ffmpeg_download("x86_64-unknown-linux-gnu", &version).unwrap();
        assert_eq!(file, "ffmpeg-n9.0.2-35-g9a9b70bb8c-linux64-gpl-9.0.tar.xz");
        assert!(url.ends_with(&file) && url.contains(FFMPEG_BTBN_TAG));
        let (file, _) = ffmpeg_download("aarch64-unknown-linux-gnu", &version).unwrap();
        assert_eq!(
            file,
            "ffmpeg-n9.0.2-35-g9a9b70bb8c-linuxarm64-gpl-9.0.tar.xz"
        );
        let (file, url) = ffmpeg_download("aarch64-apple-darwin", &version).unwrap();
        assert_eq!(file, "ffmpeg.zip");
        assert!(url.contains(&version));
    }

    /// El bundle se aprovisiona en el directorio que recibe —no en una ruta
    /// fija bajo la raíz— y el marcador con el SHA-256 fijado es lo que decide
    /// entre reutilizarlo y reconstruirlo: un marcador coincidente lo declara
    /// fresco y uno divergente lo invalida.
    #[test]
    fn ffmpeg_bundle_freshness_is_decided_in_the_given_directory() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = dir.path().join("bundle-explicito");
        std::fs::create_dir_all(&bundle).unwrap();
        let triple = "x86_64-unknown-linux-gnu";
        std::fs::write(bundle.join(ffmpeg_binary_name(triple).unwrap()), b"ffmpeg").unwrap();
        let pinned = "a".repeat(64);
        std::fs::write(bundle.join(FFMPEG_MARKER), &pinned).unwrap();
        assert!(ffmpeg_bundle_is_fresh(&bundle, triple, &pinned).unwrap());
        assert!(!ffmpeg_bundle_is_fresh(&bundle, triple, &"b".repeat(64)).unwrap());
    }

    /// Cada target del manifiesto exige el ejecutable de FFmpeg y fija su
    /// SHA-256, también en `pins.json`.
    #[test]
    fn manifest_targets_require_ffmpeg() {
        let pins = pins::parse_text(include_str!("../../../packaging/pins.json"))
            .expect("pins.json válido");
        let sections =
            parse_manifest_text(include_str!("../../../packaging/bundle-manifest.json")).unwrap();
        for (triple, section) in &sections {
            pinned_ffmpeg_sha256(&pins, triple).unwrap();
            let name = ffmpeg_binary_name(triple).unwrap();
            assert!(
                section.required.iter().any(|r| r == name),
                "el manifiesto de {triple} debe exigir {name}"
            );
        }
        assert_eq!(
            pins.ffmpeg_sha256.len(),
            sections.len(),
            "un hash por target, sin sobrantes"
        );
    }

    /// Versión de FFmpeg fijada en `pins.json`.
    fn test_ffmpeg_version() -> String {
        pins::parse_text(include_str!("../../../packaging/pins.json"))
            .expect("pins.json válido")
            .ffmpeg
    }

    /// Cada target del manifiesto canónico tiene su SHA-256 de ONNX Runtime
    /// fijado y bien formado, y su sección exige en el bundle todas las
    /// librerías que aporta `ort-bundle/` (en Windows, también las cuatro DLL
    /// de VC++): empaquetado, instalación y pines no pueden divergir.
    #[test]
    fn manifest_targets_have_pinned_hash_and_bundle_libs() {
        let pins = pins::parse_text(include_str!("../../../packaging/pins.json"))
            .expect("pins.json válido");
        let sections =
            parse_manifest_text(include_str!("../../../packaging/bundle-manifest.json")).unwrap();
        for (triple, section) in &sections {
            pinned_ort_sha256(&pins, triple).unwrap();
            for lib in ort_expected_libs(triple).unwrap() {
                assert!(
                    section.required.iter().any(|r| r == lib),
                    "el manifiesto de {triple} debe exigir {lib}"
                );
            }
        }
        assert_eq!(
            pins.ort_sha256.len(),
            sections.len(),
            "un hash por target, sin sobrantes"
        );
        let windows_libs = ort_expected_libs("x86_64-pc-windows-msvc").unwrap();
        assert!(windows_libs.contains(&"vcomp140.dll"));
        // La ONNX Runtime oficial de Windows importa `MSVCP140_1.dll`, que no
        // forma parte de Windows: sin ella no carga en un sistema sin el
        // Redistributable.
        assert!(windows_libs.contains(&"msvcp140_1.dll"));
    }

    /// Un triple sin hash o con un hash mal formado es un error.
    #[test]
    fn pinned_hash_must_exist_and_be_lowercase_hex() {
        let mut pins = pins::parse_text(include_str!("../../../packaging/pins.json"))
            .expect("pins.json válido");
        assert!(pinned_ort_sha256(&pins, "riscv64gc-unknown-linux-gnu").is_err());
        pins.ort_sha256
            .insert("x86_64-unknown-linux-gnu".to_string(), "AB".repeat(32));
        assert!(pinned_ort_sha256(&pins, "x86_64-unknown-linux-gnu").is_err());
        pins.ort_sha256
            .insert("x86_64-unknown-linux-gnu".to_string(), "ab".repeat(31));
        assert!(pinned_ort_sha256(&pins, "x86_64-unknown-linux-gnu").is_err());
    }

    /// La verificación acepta el SHA-256 exacto del fichero y rechaza otro.
    #[test]
    fn sha256_verification_accepts_match_and_rejects_mismatch() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), b"abc").unwrap();
        let abc = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        verify_sha256(file.path(), abc).unwrap();
        let err = verify_sha256(file.path(), &"0".repeat(64)).unwrap_err();
        assert!(
            err.to_string().contains(abc),
            "el error nombra el hash obtenido: {err}"
        );
    }

    /// Un `ort-bundle/` con las librerías y marcador coincidente se reutiliza;
    /// sin marcador, con marcador divergente o con librerías ausentes, no.
    #[test]
    fn ort_bundle_freshness() {
        let ort_version = test_ort_version();
        let triple = if cfg!(windows) {
            "x86_64-pc-windows-msvc"
        } else {
            "x86_64-unknown-linux-gnu"
        };
        let base_guard = tempfile::Builder::new()
            .prefix("xtask_package_ort_")
            .tempdir()
            .unwrap();
        let base = base_guard.path().to_path_buf();
        for lib in ort_expected_libs(triple).unwrap() {
            std::fs::write(base.join(lib), b"x").unwrap();
        }
        // Sin marcador la versión es desconocida: no es fresco.
        assert!(!ort_bundle_is_fresh(&base, triple, &ort_version).unwrap());
        std::fs::write(base.join(ORT_MARKER), "0.0.0").unwrap();
        assert!(!ort_bundle_is_fresh(&base, triple, &ort_version).unwrap());
        std::fs::write(base.join(ORT_MARKER), &ort_version).unwrap();
        assert!(ort_bundle_is_fresh(&base, triple, &ort_version).unwrap());
        std::fs::remove_file(base.join(ort_expected_libs(triple).unwrap()[0])).unwrap();
        assert!(!ort_bundle_is_fresh(&base, triple, &ort_version).unwrap());
        // Vacía (descarga truncada) tampoco es fresca: fuerza reconstruir.
        std::fs::write(base.join(ort_expected_libs(triple).unwrap()[0]), b"").unwrap();
        assert!(!ort_bundle_is_fresh(&base, triple, &ort_version).unwrap());
    }

    /// El asset de Linux lista `libonnxruntime.so.1` (enlace, 0 bytes) antes
    /// que la librería real: la extracción debe traer los bytes reales, no
    /// los del enlace.
    #[test]
    fn extract_ignores_symlink_before_real_library() {
        let base_guard = tempfile::Builder::new()
            .prefix("xtask_package_symlink_")
            .tempdir()
            .unwrap();
        let base = base_guard.path().to_path_buf();
        let archive_path = base.join("ort.tgz");
        {
            let file = std::fs::File::create(&archive_path).unwrap();
            let gz = flate2::write::GzEncoder::new(file, flate2::Compression::default());
            let mut tar = tar::Builder::new(gz);
            let mut link = tar::Header::new_gnu();
            link.set_entry_type(tar::EntryType::Symlink);
            link.set_size(0);
            link.set_cksum();
            tar.append_link(
                &mut link,
                "lib/libonnxruntime.so.1",
                "libonnxruntime.so.1.28.0",
            )
            .unwrap();
            let mut header = tar::Header::new_gnu();
            let data = b"REAL";
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, "lib/libonnxruntime.so.1.28.0", &data[..])
                .unwrap();
            let gz = tar.into_inner().unwrap();
            gz.finish().unwrap();
        }
        let bundle = base.join("bundle");
        extract_ort_library(&archive_path, &bundle, "x86_64-unknown-linux-gnu", "1.28.0").unwrap();
        assert_eq!(
            std::fs::read(bundle.join("libonnxruntime.so")).unwrap(),
            b"REAL"
        );
    }

    /// Compresión determinista: dos pasadas sobre el mismo árbol dan el mismo
    /// archivo con exactamente las entradas del manifiesto.
    #[test]
    fn compression_is_deterministic() {
        let base_guard = tempfile::Builder::new()
            .prefix("xtask_package_zip_")
            .tempdir()
            .unwrap();
        let base = base_guard.path().to_path_buf();
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
    }

    /// El destino del árbol sin comprimir: con `--flat` cuelga de la raíz de
    /// `--out` (layout de staging, que el motor espera plano), y sin él va en
    /// un subdirectorio con el nombre del artefacto. Puro, sin disco del repo.
    #[test]
    fn uncompressed_destination_follows_flat() {
        let out = Path::new("/tmp/salida");
        let asset = "ai-voice-interconnector-0.24.0-x86_64-linux.tar.gz";
        let stem = asset.trim_end_matches(".tar.gz").to_string();
        assert_eq!(uncompressed_dest(out, asset, false), out.join(&stem));
        assert_eq!(uncompressed_dest(out, asset, true), out.to_path_buf());
    }

    /// `--flat` sin `--no-compress` es un error de uso, no una bandera muda.
    #[test]
    fn flat_requires_no_compress() {
        let err = run(
            None,
            /* no_compress */ false,
            /* flat */ true,
            Some("9.9.9".to_string()),
        )
        .expect_err("`--flat` sin `--no-compress` debe fallar");
        assert!(
            err.to_string().contains("--flat"),
            "el mensaje nombra la bandera: {err}"
        );
    }
}
