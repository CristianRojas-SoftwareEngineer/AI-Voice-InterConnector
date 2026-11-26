// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! `xtask package`: monta el bundle del release para el target del host.
//!
//! Lee `packaging/bundle-manifest.json` como lista canónica (la misma que valida
//! `self install`): empaquetado e instalación no pueden divergir. Resuelve ONNX
//! Runtime reutilizando `ort-bundle/` si está fresco para el paquete fijado
//! en `packaging/pins.json` (fuente única, espejo del parámetro `ort_pin` de
//! la CI), o descargándolo si no: el paquete NuGet descargado debe tener el
//! SHA-256 del paquete NuGet que fija `pins.json` y traer la librería del
//! target. Resuelve
//! FFmpeg del mismo modo con `ffmpeg-bundle/`, verifica su SHA-256, exige los
//! códecs de la matriz de egressión y ejecuta `-version` antes de montarlo.
//! Toma el motor TTS de `engine-bundle/`, también verificado por SHA-256, y
//! lo monta en la raíz del bundle. Valida `--expect-version` (la puerta tag-versión vive aquí, ningún paso YAML
//! la duplica), compila el binario desde el árbol actual (el bundle nunca lleva
//! un `target/release` de otra compilación), pasa el humo, que es el único del
//! binario (contrato de `version --json`, `voice list`, `speech denoise`, en un
//! estado temporal propio) y comprime de forma determinista con el nombre de
//! `release_asset_name`.
//!
//! Sin red en los tests: las descargas viven en `ensure_ort_bundle` y
//! `ensure_ffmpeg_bundle`; todo lo demás (manifiesto, nombres, puerta de
//! versión, hash, montaje, compresión) es puro o solo toca temporales.

use anyhow::{anyhow, bail, Result};
use avi_shared::manifest::{parse_manifest_text, BundleTarget};
use avi_shared::pins::{self, Pins};
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::io::Seek;
use std::path::{Path, PathBuf};

/// Manifiesto canónico, relativo a la raíz del repositorio.
pub(crate) const MANIFEST_REL: &str = "packaging/bundle-manifest.json";

/// Caché local del bundle de ONNX Runtime, relativa a la raíz. Es la única
/// fuente de esta ruta en `xtask`: ningún otro módulo la escribe a mano.
pub(crate) const ORT_BUNDLE_REL: &str = "ort-bundle";

/// Caché local del binario de FFmpeg, relativa a la raíz. Es la única fuente
/// de esta ruta en `xtask`: ningún otro módulo la escribe a mano.
pub(crate) const FFMPEG_BUNDLE_REL: &str = "ffmpeg-bundle";

/// Marcador del SHA-256 del artefacto dentro de `ffmpeg-bundle/`: un cambio de
/// pin invalida el bundle aunque la versión nominal no cambie.
pub(crate) const FFMPEG_MARKER: &str = ".ffmpeg-sha256";

/// Release de BtbN del que salen los binarios Linux. Solo se fija el último
/// build de un mes, porque BtbN conserva el último build de cada mes durante dos
/// años y de los diarios solo los 14 últimos; el tag `latest` se mueve, así que
/// se fija uno fechado. Una prueba exige que el día sea el último de su mes.
pub(crate) const FFMPEG_BTBN_TAG: &str = "autobuild-2026-10-31-12-59";

/// Build concreto de FFmpeg dentro de [`FFMPEG_BTBN_TAG`].
pub(crate) const FFMPEG_BTBN_BUILD: &str = "n9.0.2-39-g7adfbe40dc";

/// Commit al que apunta [`FFMPEG_BTBN_TAG`] en el repositorio de scripts de BtbN. Se
/// descarga el archivo del commit y no el del tag, que un autor podría mover.
const FFMPEG_BTBN_COMMIT: &str = "5031c6f1082414e53688d39e521849905071070d";

/// Marcador del SHA-256 del paquete NuGet dentro de `ort-bundle/`: un cambio de
/// pin invalida el bundle aunque la versión nominal no cambie. Un bundle sin
/// marcador no se reutiliza. Es la única fuente de este nombre en `xtask`.
pub(crate) const ORT_MARKER: &str = ".ort-sha256";

/// Caché local del paquete del motor TTS, relativa a la raíz. Es la única
/// fuente de esta ruta en `xtask`: ningún otro módulo la escribe a mano.
pub(crate) const ENGINE_BUNDLE_REL: &str = "engine-bundle";

/// Marcador del SHA-256 del paquete del motor dentro de `engine-bundle/`: un
/// cambio de pin invalida el bundle aunque la versión nominal no cambie.
pub(crate) const ENGINE_MARKER: &str = ".qwen-tts-engine-sha256";

/// Repositorio del motor TTS del que se descarga el paquete fijado.
pub(crate) const ENGINE_REPO: &str = "CristianRojas-SoftwareEngineer/qwen-tts-engine";

/// Avisos que el paquete del motor distribuye y que quedan en la raíz de
/// `engine-bundle/` junto al ejecutable.
const ENGINE_NOTICES: [&str; 3] = ["LICENSE", "NOTICE", "THIRD-PARTY-LICENSES.md"];

/// Marca temporal fija de los archivos comprimidos (2020-01-01 UTC): con
/// entradas ordenadas y metadatos fijos, el mismo árbol produce el mismo
/// archivo y el `SHA256SUMS.txt` del release es reproducible.
const ARCHIVE_MTIME: u64 = 1_577_836_800;

/// DLL del runtime de VC++ que viajan junto al binario en Windows (despliegue
/// app-local, que la licencia del Redistributable permite): las cuatro que
/// necesita la librería oficial de ONNX Runtime (es /MD) y `vcomp140.dll`, el
/// runtime OpenMP de MSVC que el binario importa porque CTranslate2 se compila
/// con `/openmp`. Ninguna forma parte de Windows: sin ellas el binario o ONNX
/// Runtime no cargan en un sistema sin el Redistributable. Salen del redist de
/// la instalación de Visual Studio que compila: las cuatro primeras de la
/// carpeta `Microsoft.VC*.CRT` y `vcomp140.dll` de `Microsoft.VC*.OpenMP`.
pub(crate) const VC_DLLS: &[&str] = &[
    "vcruntime140.dll",
    "vcruntime140_1.dll",
    "msvcp140.dll",
    "msvcp140_1.dll",
    "vcomp140.dll",
];

/// DLL de [`VC_DLLS`] que sale de la carpeta `Microsoft.VC*.OpenMP` del redist;
/// las demás salen de `Microsoft.VC*.CRT`.
#[cfg(any(windows, test))]
const VC_OPENMP_DLL: &str = "vcomp140.dll";

/// Ruta fija de `vswhere.exe`, el localizador de instalaciones de Visual Studio
/// que el instalador deja siempre en el mismo sitio.
pub(crate) const VSWHERE_PATH: &str =
    r"C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe";

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

/// Nombre de archivo y URL del paquete NuGet oficial `Microsoft.ML.OnnxRuntime`
/// de la versión indicada, con el id y la versión en minúsculas como exige el
/// contenedor plano de nuget.org. nuget.org no permite borrar paquetes, así que
/// la versión fijada sigue descargable.
fn ort_nupkg_download(ort_version: &str) -> (String, String) {
    let version = ort_version.to_ascii_lowercase();
    let file = format!("microsoft.ml.onnxruntime.{version}.nupkg");
    let url =
        format!("https://api.nuget.org/v3-flatcontainer/microsoft.ml.onnxruntime/{version}/{file}");
    (file, url)
}

/// Entrada del paquete NuGet que contiene la librería de ONNX Runtime del
/// triple. Un triple sin entrada es un error.
fn ort_package_entry(triple: &str) -> Result<&'static str> {
    Ok(match triple {
        "x86_64-pc-windows-msvc" => "runtimes/win-x64/native/onnxruntime.dll",
        "x86_64-unknown-linux-gnu" => "runtimes/linux-x64/native/libonnxruntime.so",
        "aarch64-unknown-linux-gnu" => "runtimes/linux-arm64/native/libonnxruntime.so",
        "aarch64-apple-darwin" => "runtimes/osx-arm64/native/libonnxruntime.dylib",
        other => bail!("triple no soportado: {other}"),
    })
}

/// SHA-256 fijado en `pins.json` para el paquete NuGet de ONNX Runtime: 64
/// dígitos hexadecimales en minúsculas. Un valor mal formado es un error,
/// nunca una descarga sin verificar.
pub(crate) fn pinned_ort_nupkg_sha256(pins: &Pins) -> Result<&str> {
    let hash = pins.ort_nupkg_sha256.as_str();
    if !is_sha256(hash) {
        bail!("el SHA-256 de ort_nupkg_sha256 en packaging/pins.json no son 64 dígitos hexadecimales en minúsculas: {hash:?}");
    }
    Ok(hash)
}

/// SHA-256 fijado en `pins.json` para el artefacto de FFmpeg del triple.
pub(crate) fn pinned_ffmpeg_sha256<'a>(pins: &'a Pins, triple: &str) -> Result<&'a str> {
    pinned_sha256(&pins.ffmpeg_sha256, "FFmpeg", triple)
}

/// SHA-256 fijado en `pins.json` para el paquete del motor TTS del triple.
pub(crate) fn pinned_engine_sha256<'a>(pins: &'a Pins, triple: &str) -> Result<&'a str> {
    pinned_sha256(&pins.qwen_tts_engine_sha256, "qwen-tts-engine", triple)
}

/// `true` si `hash` son 64 dígitos hexadecimales en minúsculas.
pub(crate) fn is_sha256(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
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
    if !is_sha256(hash) {
        bail!("el SHA-256 de {artifact} para {triple} en packaging/pins.json no son 64 dígitos hexadecimales en minúsculas: {hash:?}");
    }
    Ok(hash)
}

/// Archivo y URL de descarga de FFmpeg para el triple. Linux usa un release
/// de los builds GPL de BtbN y macOS el build de Martin Riedl; Windows, el
/// essentials de Gyan. El pin de versión manda en todos los casos.
///
/// Conservación de cada origen. BtbN conserva dos años el último build de cada
/// mes, y por eso solo se fija uno de fin de mes (ver [`FFMPEG_BTBN_TAG`]).
/// GyanD y Martin Riedl no publican política de conservación, pero conservan
/// todos sus builds de release (GyanD desde la 4.4, de abril de 2021; Martin
/// Riedl desde la 5.0, de enero de 2022). Si alguno desapareciera, la descarga
/// de la CI fallaría con un 404 y los paquetes ya instalados no se verían
/// afectados.
///
/// Windows se descarga del release que el propio autor del build publica en
/// GitHub, no de su sitio de paquetes: ese sitio degrada las transferencias
/// sostenidas hasta volver inútil bajar 110 MB, mientras que el release los
/// sirve a velocidad de CDN. Los dos orígenes publican el mismo fichero, y
/// quien lo decide no es la URL sino el SHA-256 fijado, que se verifica sobre
/// el archivo descargado antes de extraer nada: un origen que sirviera otro
/// binario no llegaría al bundle.
pub(crate) fn ffmpeg_download(triple: &str, version: &str) -> Result<(String, String)> {
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

/// Assets de fuentes de FFmpeg que se publican con cada release, como pares `(nombre,
/// URL)`: el tarball upstream de la versión fijada y los scripts de build de BtbN, el
/// único compilador cuyo repositorio los publica con una URL estable. Gyan y Martin Riedl
/// no publican scripts descargables; su configuración de build queda en el archivo
/// `ffmpeg-<versión>-buildconf-<triple>.txt` que escribe `xtask package`.
pub(crate) fn ffmpeg_sources(version: &str) -> Vec<(String, String)> {
    vec![
        (
            format!("ffmpeg-{version}.tar.xz"),
            format!("https://ffmpeg.org/releases/ffmpeg-{version}.tar.xz"),
        ),
        (
            format!("ffmpeg-builds-{FFMPEG_BTBN_TAG}.tar.gz"),
            format!("https://github.com/BtbN/FFmpeg-Builds/archive/{FFMPEG_BTBN_COMMIT}.tar.gz"),
        ),
    ]
}

/// Descarga cada asset de [`ffmpeg_sources`] a `out` verificando su SHA-256 fijado en
/// `pins.json`. Falla si a un asset le falta el hash, si un hash fijado no corresponde a
/// ningún asset o si algún hash no coincide; un asset no se deja en `out` hasta verificarlo.
pub(crate) fn fetch_ffmpeg_sources(out: &Path, pins: &Pins) -> Result<Vec<PathBuf>> {
    let sources = ffmpeg_sources(&pins.ffmpeg);
    for name in pins.ffmpeg_source_sha256.keys() {
        if !sources.iter().any(|(asset, _)| asset == name) {
            bail!("pins.json fija el hash de `{name}`, que no es un asset de fuentes de FFmpeg");
        }
    }
    let mut hashes = Vec::new();
    for (name, _) in &sources {
        let hash = pins.ffmpeg_source_sha256.get(name).ok_or_else(|| {
            anyhow!("pins.json no fija el hash de `{name}` en `ffmpeg_source_sha256`")
        })?;
        hashes.push(hash.as_str());
    }
    std::fs::create_dir_all(out)?;
    let mut written = Vec::new();
    for ((name, url), hash) in sources.iter().zip(hashes) {
        let temporary = tempfile::Builder::new()
            .prefix("xtask-ffmpeg-source-")
            .suffix(&format!("-{name}"))
            .tempfile()?
            .into_temp_path();
        eprintln!("Descargando {name}: {url}");
        download_https(url, &temporary)?;
        verify_sha256(&temporary, hash)?;
        let dest = out.join(name);
        std::fs::copy(&temporary, &dest)?;
        written.push(dest);
    }
    Ok(written)
}

/// Configuración de build del ejecutable de FFmpeg (`-buildconf`). Falla si el
/// ejecutable no la imprime: sin ella no queda constancia de cómo se compiló.
fn ffmpeg_buildconf(binary: &Path) -> Result<String> {
    let output = std::process::Command::new(binary)
        .args(["-hide_banner", "-buildconf"])
        .output()
        .map_err(|e| anyhow!("no se pudo ejecutar {}: {e}", binary.display()))?;
    if !output.status.success() {
        bail!(
            "{} -buildconf terminó con {}",
            binary.display(),
            output.status
        );
    }
    // FFmpeg imprime la configuración por su registro, que va a la salida de error.
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if text.trim().is_empty() {
        bail!("{} -buildconf no imprimió nada", binary.display());
    }
    Ok(text)
}

/// Triple con el que el motor publica su paquete para el target de AVI: el
/// motor solo compila el binario de Windows con host GNU, así que el target
/// MSVC de AVI consume el artefacto `x86_64-pc-windows-gnu`; el resto coincide.
fn engine_triple(triple: &str) -> Result<&'static str> {
    match triple {
        "x86_64-pc-windows-msvc" => Ok("x86_64-pc-windows-gnu"),
        "x86_64-unknown-linux-gnu" => Ok("x86_64-unknown-linux-gnu"),
        "aarch64-unknown-linux-gnu" => Ok("aarch64-unknown-linux-gnu"),
        "aarch64-apple-darwin" => Ok("aarch64-apple-darwin"),
        other => bail!("triple no soportado: {other}"),
    }
}

/// Archivo y URL de descarga del motor TTS para el target de AVI. El release
/// publica un `.zip` por target de Windows y un `.tar.gz` por los de Linux y
/// macOS, con la versión y el triple del motor en el nombre.
pub(crate) fn engine_download(triple: &str, version: &str) -> Result<(String, String)> {
    let engine = engine_triple(triple)?;
    let extension = if engine.contains("-windows-") {
        "zip"
    } else {
        "tar.gz"
    };
    let file = format!("qwen-tts-engine-v{version}-{engine}.{extension}");
    let url = format!("https://github.com/{ENGINE_REPO}/releases/download/v{version}/{file}");
    Ok((file, url))
}

/// Nombre del archivo con la configuración de build de FFmpeg del triple.
pub(crate) fn ffmpeg_buildconf_name(version: &str, triple: &str) -> String {
    format!("ffmpeg-{version}-buildconf-{triple}.txt")
}

/// Nombre del ejecutable de FFmpeg dentro del bundle para el triple.
pub(crate) fn ffmpeg_binary_name(triple: &str) -> Result<&'static str> {
    if triple.contains("-windows-") {
        Ok("ffmpeg.exe")
    } else if triple.contains("-linux-") || triple.contains("-apple-") {
        Ok("ffmpeg")
    } else {
        bail!("triple no soportado: {triple}")
    }
}

/// Nombre del ejecutable del motor TTS dentro del bundle para el triple.
pub(crate) fn engine_binary_name(triple: &str) -> Result<&'static str> {
    if triple.contains("-windows-") {
        Ok("qwen_tts.exe")
    } else if triple.contains("-linux-") || triple.contains("-apple-") {
        Ok("qwen_tts")
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
/// en `load-dynamic` junto al ejecutable.
fn ort_expected_libs(triple: &str) -> Result<Vec<&'static str>> {
    let libs = match triple {
        _ if triple.contains("-windows-") => vec!["onnxruntime.dll"],
        _ if triple.contains("-apple-") => vec!["libonnxruntime.dylib"],
        _ if triple.contains("-linux-") => vec!["libonnxruntime.so"],
        _ => bail!("triple no soportado: {triple}"),
    };
    Ok(libs)
}

/// Ruta de la librería de ONNX Runtime dentro de `bundle` para el triple, la
/// que `ort` carga en `load-dynamic` cuando se la indica `ORT_DYLIB_PATH`.
pub(crate) fn ort_dylib_path(bundle: &Path, triple: &str) -> Result<PathBuf> {
    let libs = ort_expected_libs(triple)?;
    Ok(bundle.join(libs[0]))
}

/// `true` si `ort-bundle/` ya sirve para este triple: todas las librerías
/// presentes con tamaño mayor que cero y el marcador con el SHA-256 del paquete
/// fijado. Un marcador o una librería ausentes dejan el bundle sin servir; un
/// fichero que existe pero no se puede leer es un error.
pub(crate) fn ort_bundle_is_fresh(
    bundle: &Path,
    triple: &str,
    expected_sha256: &str,
) -> Result<bool> {
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
        Ok(pinned) => Ok(pinned.trim() == expected_sha256),
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

/// Archivos de aviso de ONNX Runtime que el paquete NuGet trae en su raíz y que el
/// paquete de AVI reproduce desde `packaging/notices/onnxruntime-<versión>/`.
const ORT_NOTICE_FILES: [&str; 2] = ["LICENSE", "ThirdPartyNotices.txt"];

/// Directorio con las copias versionadas de los avisos de la versión fijada de ONNX Runtime.
fn ort_notices_dir(root: &Path, ort_version: &str) -> PathBuf {
    root.join("packaging")
        .join("notices")
        .join(format!("onnxruntime-{ort_version}"))
}

/// Compara los avisos leídos del paquete NuGet con las copias versionadas, ignorando
/// la diferencia de fin de línea (el paquete NuGet los trae con CRLF y las copias
/// versionadas están en LF). Falla nombrando el archivo si falta en el paquete o
/// difiere de la copia.
fn verify_ort_notices(
    notices_dir: &Path,
    ort_version: &str,
    archived: &BTreeMap<&'static str, Vec<u8>>,
) -> Result<()> {
    for name in ORT_NOTICE_FILES {
        let bytes = archived
            .get(name)
            .ok_or_else(|| anyhow!("el paquete de ONNX Runtime {ort_version} no trae {name}"))?;
        let copy_path = notices_dir.join(name);
        let copy = std::fs::read(&copy_path).map_err(|e| {
            anyhow!(
                "no se pudo leer la copia versionada {}: {}",
                copy_path.display(),
                e
            )
        })?;
        let normalize = |data: &[u8]| String::from_utf8_lossy(data).replace("\r\n", "\n");
        if normalize(bytes) != normalize(&copy) {
            bail!(
                "{name} de ONNX Runtime {ort_version} difiere de {}: actualiza la copia versionada \
                 con el paquete oficial",
                copy_path.display()
            );
        }
    }
    Ok(())
}

/// Extrae del paquete NuGet descargado (un zip) la librería de ONNX Runtime del
/// triple a `bundle/` con su nombre canónico (el que el crate `ort` busca en
/// `load-dynamic`) y verifica que sus avisos, tomados solo de la raíz del zip,
/// coincidan con las copias de `notices_dir`. Falla nombrando la entrada si
/// falta la librería o mide cero bytes, y nombrando el aviso si falta o difiere.
fn extract_ort_library(
    archive: &Path,
    bundle: &Path,
    triple: &str,
    ort_version: &str,
    notices_dir: &Path,
) -> Result<()> {
    let entry_name = ort_package_entry(triple)?;
    let canonical = entry_name.rsplit('/').next().unwrap_or(entry_name);
    let file = std::fs::File::open(archive)?;
    let mut zip = zip::ZipArchive::new(file)?;
    let mut notices: BTreeMap<&'static str, Vec<u8>> = BTreeMap::new();
    for name in ORT_NOTICE_FILES {
        // Solo la raíz del zip: un homónimo en un subdirectorio no es el aviso.
        if let Ok(mut entry) = zip.by_name(name) {
            let mut bytes = Vec::new();
            std::io::Read::read_to_end(&mut entry, &mut bytes)?;
            notices.insert(name, bytes);
        }
    }
    let library = {
        let mut entry = zip.by_name(entry_name).map_err(|_| {
            anyhow!(
                "el paquete {} no trae la entrada {entry_name} de ONNX Runtime {ort_version}",
                archive.display()
            )
        })?;
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut entry, &mut bytes)?;
        bytes
    };
    if library.is_empty() {
        bail!(
            "la entrada {entry_name} del paquete {} mide 0 bytes",
            archive.display()
        );
    }
    verify_ort_notices(notices_dir, ort_version, &notices)?;
    std::fs::create_dir_all(bundle)?;
    std::fs::write(bundle.join(canonical), library)?;
    Ok(())
}

/// Resuelve las DLL de [`VC_DLLS`] en el redist de la instalación de Visual
/// Studio `installation`. La versión del redist sale de
/// `VC\Auxiliary\Build\Microsoft.VCRedistVersion.default.txt`, y las DLL, de
/// las carpetas `Microsoft.VC*.CRT` y `Microsoft.VC*.OpenMP` de
/// `VC\Redist\MSVC\<versión>\x64`. Falla si falta el archivo de versión o una
/// DLL, o si una de las dos carpetas no resuelve a exactamente una.
#[cfg(any(windows, test))]
fn vc_redist_dlls(installation: &Path) -> Result<Vec<PathBuf>> {
    let version_file = relative_path(
        installation,
        "VC/Auxiliary/Build/Microsoft.VCRedistVersion.default.txt",
    );
    let version = std::fs::read_to_string(&version_file).map_err(|e| {
        anyhow!(
            "no se pudo leer la versión del redist de VC++ en {}: {e}",
            version_file.display()
        )
    })?;
    let arch_dir = relative_path(installation, "VC/Redist/MSVC")
        .join(version.trim())
        .join("x64");
    let crt = unique_redist_dir(&arch_dir, ".CRT")?;
    let openmp = unique_redist_dir(&arch_dir, ".OpenMP")?;
    VC_DLLS
        .iter()
        .map(|dll| {
            let dir = if *dll == VC_OPENMP_DLL { &openmp } else { &crt };
            let path = dir.join(dll);
            if !path.is_file() {
                bail!("no se encontró {}", path.display());
            }
            Ok(path)
        })
        .collect()
}

/// Única carpeta `Microsoft.VC*<suffix>` de `arch_dir`; ninguna o varias son
/// un error, porque el origen de las DLL sería ambiguo.
#[cfg(any(windows, test))]
fn unique_redist_dir(arch_dir: &Path, suffix: &str) -> Result<PathBuf> {
    let entries = std::fs::read_dir(arch_dir)
        .map_err(|e| anyhow!("no se pudo leer {}: {e}", arch_dir.display()))?;
    let mut found = Vec::new();
    for entry in entries {
        let path = entry?.path();
        let matches = path
            .file_name()
            .and_then(OsStr::to_str)
            .is_some_and(|name| name.starts_with("Microsoft.VC") && name.ends_with(suffix));
        if matches && path.is_dir() {
            found.push(path);
        }
    }
    match found.as_slice() {
        [dir] => Ok(dir.clone()),
        [] => bail!(
            "no hay carpeta Microsoft.VC*{suffix} en {}",
            arch_dir.display()
        ),
        _ => bail!(
            "hay {} carpetas Microsoft.VC*{suffix} en {}; se espera una sola",
            found.len(),
            arch_dir.display()
        ),
    }
}

/// Copia `src` en `dest` solo si su contenido difiere y devuelve si escribió:
/// una DLL idéntica no se reescribe, porque un proceso en curso puede tenerla
/// cargada y bloqueada.
#[cfg(any(windows, test))]
fn copy_if_differs(src: &Path, dest: &Path) -> Result<bool> {
    let wanted =
        std::fs::read(src).map_err(|e| anyhow!("no se pudo leer {}: {e}", src.display()))?;
    match std::fs::read(dest) {
        Ok(current) if current == wanted => return Ok(false),
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => bail!("no se pudo leer {}: {e}", dest.display()),
    }
    std::fs::write(dest, &wanted)
        .map_err(|e| anyhow!("no se pudo escribir {}: {e}", dest.display()))?;
    Ok(true)
}

/// Instalación de Visual Studio con el compilador MSVC x64, con la misma
/// consulta de `vswhere` con la que la CI importa `vcvars64`: el runtime
/// empaquetado sale de la toolchain que compila.
#[cfg(windows)]
fn vs_installation() -> Result<PathBuf> {
    let output = std::process::Command::new(VSWHERE_PATH)
        .args([
            "-latest",
            "-products",
            "*",
            "-requires",
            "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
            "-property",
            "installationPath",
        ])
        .output()
        .map_err(|e| anyhow!("no se pudo ejecutar {VSWHERE_PATH}: {e}"))?;
    if !output.status.success() {
        bail!("{VSWHERE_PATH} terminó con {}", output.status);
    }
    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if path.is_empty() {
        bail!("vswhere no encontró una instalación de Visual Studio con MSVC x64");
    }
    Ok(PathBuf::from(path))
}

/// Deja en el bundle las DLL del runtime de VC++ del redist de la toolchain
/// que compila (app-local: el usuario no instala el Redistributable). Corre en
/// cada ejecución y solo escribe las DLL cuyo contenido difiere.
#[cfg(windows)]
fn copy_vc_runtime(bundle: &Path) -> Result<()> {
    for src in vc_redist_dlls(&vs_installation()?)? {
        let name = src
            .file_name()
            .ok_or_else(|| anyhow!("ruta sin nombre de archivo: {}", src.display()))?;
        copy_if_differs(&src, &bundle.join(name))?;
    }
    Ok(())
}

/// Asegura `ort-bundle/` para el triple: lo reutiliza si está fresco o lo
/// reconstruye descargándolo en caso contrario. La descarga del paquete NuGet va
/// por HTTPS, su SHA-256 debe coincidir con el fijado en `pins.json` y el
/// paquete debe traer la librería esperada con tamaño mayor que cero; el
/// marcador con el hash del paquete solo se escribe tras esas comprobaciones, así que un bundle fresco procede de
/// una descarga verificada. En Windows, tanto al reutilizar como al descargar,
/// deja al día las DLL del runtime de VC++ con las del redist de la toolchain
/// que compila. Lo llama `ensure_pinned_bundles`, la entrada que usan
/// `bootstrap`, la puerta de pruebas con recursos y `package`: un solo
/// descargador en el repositorio.
pub(crate) fn ensure_ort_bundle(root: &Path, triple: &str, pins: &Pins) -> Result<PathBuf> {
    let ort_version = pins.ort.as_str();
    let expected_sha256 = pinned_ort_nupkg_sha256(pins)?;
    let bundle = ort_bundle_dir(root);
    if bundle.is_dir() && ort_bundle_is_fresh(&bundle, triple, expected_sha256)? {
        eprintln!("Bundle ONNX Runtime reutilizado de {ORT_BUNDLE_REL}");
        #[cfg(windows)]
        copy_vc_runtime(&bundle)?;
        return Ok(bundle);
    }
    if bundle.is_dir() {
        std::fs::remove_dir_all(&bundle)?;
    }
    std::fs::create_dir_all(&bundle)?;
    let (file, url) = ort_nupkg_download(ort_version);
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
    extract_ort_library(
        &dest,
        &bundle,
        triple,
        ort_version,
        &ort_notices_dir(root, ort_version),
    )?;
    #[cfg(windows)]
    copy_vc_runtime(&bundle)?;
    std::fs::write(bundle.join(ORT_MARKER), expected_sha256)?;
    if !ort_bundle_is_fresh(&bundle, triple, expected_sha256)? {
        bail!("el bundle de ONNX Runtime quedó incompleto tras la descarga");
    }
    Ok(bundle)
}

/// `true` si `ffmpeg-bundle/` ya sirve para el triple: el ejecutable presente
/// con tamaño mayor que cero y el marcador con el SHA-256 del artefacto fijado.
pub(crate) fn ffmpeg_bundle_is_fresh(
    bundle: &Path,
    triple: &str,
    expected_sha256: &str,
) -> Result<bool> {
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

/// `true` si `engine-bundle/` ya sirve para el triple: el ejecutable presente
/// con tamaño mayor que cero y el marcador con el SHA-256 del paquete fijado.
pub(crate) fn engine_bundle_is_fresh(
    bundle: &Path,
    triple: &str,
    expected_sha256: &str,
) -> Result<bool> {
    let binary = bundle.join(engine_binary_name(triple)?);
    let marker = bundle.join(ENGINE_MARKER);
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

/// Extrae del paquete del motor el ejecutable y los tres avisos, que quedan en
/// la raíz del bundle. El paquete los trae dentro de una carpeta con la versión
/// y el triple; solo se acepta el nombre de archivo, así que una ruta anidada o
/// absoluta no puede escribir fuera de `bundle`. Falla si falta alguno de los
/// cuatro o si alguno está vacío.
fn extract_engine_bundle(archive: &Path, bundle: &Path, triple: &str) -> Result<()> {
    let mut wanted = vec![engine_binary_name(triple)?];
    wanted.extend(ENGINE_NOTICES);
    let mut written: Vec<String> = Vec::new();
    let name = archive.to_string_lossy();
    if name.ends_with(".zip") {
        let file = std::fs::File::open(archive)?;
        let mut zip = zip::ZipArchive::new(file)?;
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i)?;
            if entry.is_dir() {
                continue;
            }
            let entry_name = entry.name().rsplit('/').next().unwrap_or("").to_string();
            if wanted.contains(&entry_name.as_str()) {
                let mut bytes = Vec::new();
                std::io::Read::read_to_end(&mut entry, &mut bytes)?;
                write_engine_member(bundle, &entry_name, &bytes, archive)?;
                written.push(entry_name);
            }
        }
    } else if name.ends_with(".tar.gz") {
        let file = std::fs::File::open(archive)?;
        let decoder = flate2::read::GzDecoder::new(std::io::BufReader::new(file));
        let mut tar = tar::Archive::new(decoder);
        for entry in tar.entries()? {
            let mut entry = entry?;
            if !entry.header().entry_type().is_file() {
                continue;
            }
            let path = entry.path()?.to_string_lossy().to_string();
            let entry_name = path.rsplit('/').next().unwrap_or("").to_string();
            if wanted.contains(&entry_name.as_str()) {
                let mut bytes = Vec::new();
                std::io::Read::read_to_end(&mut entry, &mut bytes)?;
                write_engine_member(bundle, &entry_name, &bytes, archive)?;
                written.push(entry_name);
            }
        }
    } else {
        bail!(
            "formato de paquete del motor no soportado: {}",
            archive.display()
        );
    }
    for member in wanted {
        if !written.iter().any(|w| w == member) {
            bail!(
                "el paquete del motor no trae {member}: {}",
                archive.display()
            );
        }
    }
    Ok(())
}

/// Escribe un miembro del paquete del motor en la raíz del bundle. Un miembro
/// vacío es un paquete truncado y detiene el aprovisionamiento.
fn write_engine_member(bundle: &Path, name: &str, bytes: &[u8], archive: &Path) -> Result<()> {
    if bytes.is_empty() {
        bail!(
            "el miembro {name} del paquete del motor está vacío: {}",
            archive.display()
        );
    }
    std::fs::write(bundle.join(name), bytes)?;
    Ok(())
}

/// Directorio canónico del bundle de FFmpeg dentro del repositorio.
pub(crate) fn ffmpeg_bundle_dir(root: &Path) -> PathBuf {
    root.join(FFMPEG_BUNDLE_REL)
}

/// Directorio canónico del bundle de ONNX Runtime dentro del repositorio.
pub(crate) fn ort_bundle_dir(root: &Path) -> PathBuf {
    root.join(ORT_BUNDLE_REL)
}

/// Directorio canónico del bundle del motor TTS dentro del repositorio.
pub(crate) fn engine_bundle_dir(root: &Path) -> PathBuf {
    root.join(ENGINE_BUNDLE_REL)
}

/// Punto de entrada de `xtask ffmpeg`: aprovisiona en `dest` el FFmpeg fijado
/// en `packaging/pins.json` para el target del host —la misma operación que
/// `xtask package` hace antes de montar el bundle, con la misma verificación de
/// SHA-256 y el mismo humo— y publica en la salida estándar la ruta del
/// ejecutable resultante, para que quien invoque el subcomando no tenga que
/// adivinar dónde quedó.
///
/// Con `sources`, en cambio, descarga a ese directorio las fuentes de FFmpeg que se
/// publican con el release, verificando el hash fijado de cada una, y lista las rutas.
pub fn run_ffmpeg(dest: Option<&Path>, sources: Option<&Path>) -> Result<()> {
    let root = std::env::current_dir()?;
    let pins = pins::load_from_root(&root).map_err(|e| anyhow::anyhow!("{e}"))?;
    if let Some(sources) = sources {
        for path in fetch_ffmpeg_sources(sources, &pins)? {
            println!("{}", path.display());
        }
        return Ok(());
    }
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
pub(crate) fn ensure_ffmpeg_bundle(bundle: &Path, triple: &str, pins: &Pins) -> Result<PathBuf> {
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

/// Asegura `engine-bundle/` para el triple: reutiliza el bundle si está fresco
/// o lo reconstruye descargando el paquete de la release del motor. La descarga
/// va por HTTPS y su SHA-256 debe coincidir con el fijado en `pins.json`; el
/// paquete deja en la raíz del bundle el ejecutable y los tres avisos, y el
/// marcador solo se escribe tras esas comprobaciones, así que un bundle fresco
/// procede de una descarga verificada.
pub(crate) fn ensure_engine_bundle(root: &Path, triple: &str, pins: &Pins) -> Result<PathBuf> {
    let version = pins.qwen_tts_engine.as_str();
    let expected_sha256 = pinned_engine_sha256(pins, triple)?;
    let bundle = engine_bundle_dir(root);
    if bundle.is_dir() && engine_bundle_is_fresh(&bundle, triple, expected_sha256)? {
        eprintln!("Bundle del motor TTS reutilizado de {ENGINE_BUNDLE_REL}");
        return Ok(bundle);
    }
    if bundle.is_dir() {
        std::fs::remove_dir_all(&bundle)?;
    }
    std::fs::create_dir_all(&bundle)?;
    let (file, url) = engine_download(triple, version)?;
    let dest = tempfile::Builder::new()
        .prefix(&format!("xtask-qwen-tts-engine-{version}-"))
        .suffix(&format!("-{file}"))
        .tempfile()?
        .into_temp_path();
    eprintln!("Descargando el motor qwen-tts-engine {version}: {url}");
    download_https(&url, &dest)?;
    verify_sha256(&dest, expected_sha256)?;
    extract_engine_bundle(&dest, &bundle, triple)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let binary = bundle.join(engine_binary_name(triple)?);
        std::fs::set_permissions(binary, std::fs::Permissions::from_mode(0o755))?;
    }
    std::fs::write(bundle.join(ENGINE_MARKER), expected_sha256)?;
    if !engine_bundle_is_fresh(&bundle, triple, expected_sha256)? {
        bail!("el bundle del motor TTS quedó incompleto tras la descarga");
    }
    Ok(bundle)
}

/// Conjunto completo de recursos fijados del repositorio, uno por campo. Quien
/// asegura (`ensure_pinned_bundles`) y quien comprueba (`pinned_bundles_status`)
/// construyen este mismo literal, así que añadir un campo obliga a cubrirlo en
/// los dos lados: el compilador impide que uno se quede atrás.
pub(crate) struct PinnedBundles<T> {
    pub ort: T,
    pub ffmpeg: T,
    pub engine: T,
}

/// Asegura todos los recursos fijados en sus directorios canónicos y devuelve
/// la ruta de cada bundle. Es la entrada que comparten `bootstrap`, la puerta
/// de pruebas con recursos y `package`; reutiliza un bundle fresco sin
/// descargar nada.
pub(crate) fn ensure_pinned_bundles(
    root: &Path,
    triple: &str,
    pins: &Pins,
) -> Result<PinnedBundles<PathBuf>> {
    Ok(PinnedBundles {
        ort: ensure_ort_bundle(root, triple, pins)?,
        ffmpeg: ensure_ffmpeg_bundle(&ffmpeg_bundle_dir(root), triple, pins)?,
        engine: ensure_engine_bundle(root, triple, pins)?,
    })
}

/// Comprueba sin escribir nada si cada recurso fijado está fresco en su
/// directorio canónico. Un pin ausente o un fichero ilegible es un error del
/// recurso afectado y no contamina el estado de los demás.
pub(crate) fn pinned_bundles_status(
    root: &Path,
    triple: &str,
    pins: &Pins,
) -> PinnedBundles<Result<bool>> {
    PinnedBundles {
        ort: pinned_ort_nupkg_sha256(pins)
            .and_then(|sha| ort_bundle_is_fresh(&ort_bundle_dir(root), triple, sha)),
        ffmpeg: pinned_ffmpeg_sha256(pins, triple)
            .and_then(|sha| ffmpeg_bundle_is_fresh(&ffmpeg_bundle_dir(root), triple, sha)),
        engine: pinned_engine_sha256(pins, triple)
            .and_then(|sha| engine_bundle_is_fresh(&engine_bundle_dir(root), triple, sha)),
    }
}

/// Copia el ejecutable de FFmpeg del `bundle` a `dest_dir` y devuelve la ruta
/// copiada. La CLI solo busca FFmpeg junto a su propio ejecutable, por eso la
/// puerta de pruebas con recursos lo coloca en la carpeta de binarios de
/// prueba. La copia se sobrescribe siempre para que un cambio de pin no deje
/// un FFmpeg viejo en esa carpeta.
pub(crate) fn stage_ffmpeg_beside(bundle: &Path, dest_dir: &Path, triple: &str) -> Result<PathBuf> {
    let name = ffmpeg_binary_name(triple)?;
    let source = bundle.join(name);
    let dest = dest_dir.join(name);
    std::fs::create_dir_all(dest_dir)
        .and_then(|()| std::fs::copy(&source, &dest))
        .map_err(|e| {
            anyhow::anyhow!(
                "no se pudo copiar FFmpeg de {} a {}: {e}",
                source.display(),
                dest.display()
            )
        })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(dest)
}

/// Devuelve un `PATH` con `dir` en primer lugar seguido de las entradas de
/// `original` en su orden. Las pruebas del crate `denoise` resuelven FFmpeg
/// por `PATH`, y la puerta antepone el FFmpeg fijado, igual que la CI, para no
/// depender del FFmpeg que tenga instalado el sistema.
pub(crate) fn path_with_prepended(dir: &Path, original: Option<&OsStr>) -> Result<OsString> {
    let mut entries = vec![dir.to_path_buf()];
    if let Some(original) = original {
        entries.extend(std::env::split_paths(original));
    }
    std::env::join_paths(entries)
        .map_err(|e| anyhow::anyhow!("no se pudo construir el PATH con {}: {e}", dir.display()))
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
}

/// Argumentos de la compilación del binario que se empaqueta: el perfil
/// release con todos los features, acotada al paquete del producto (sin
/// `--package`, Cargo intentaría reenlazar `xtask.exe` mientras está en uso, y
/// en Windows eso falla), más los argumentos que el llamador reenvía tras `--`,
/// como `--timings`. Puros, fijados por test.
fn build_args(cargo_args: &[String]) -> Vec<String> {
    let mut args: Vec<String> = [
        "build",
        "--release",
        "--package",
        "ai-voice-interconnector",
        "--features",
        "full",
    ]
    .map(String::from)
    .to_vec();
    args.extend_from_slice(cargo_args);
    args
}

/// Compila el binario que se empaqueta desde el árbol actual. Es la única
/// fuente del binario del bundle: un `target/release` de otra compilación
/// nunca llega al bundle, y con el árbol ya compilado Cargo no rehace nada.
fn build_binary(root: &Path, cargo_args: &[String]) -> Result<()> {
    let args = build_args(cargo_args);
    let status = std::process::Command::new("cargo")
        .args(&args)
        .current_dir(root)
        .status()
        .map_err(|e| anyhow::anyhow!("no se pudo ejecutar cargo: {e}"))?;
    if !status.success() {
        bail!(
            "la compilación del binario falló: cargo {} (exit {:?})",
            args.join(" "),
            status.code()
        );
    }
    Ok(())
}

/// Localiza el binario recién compilado y los cinco documentos.
/// Va antes de la descarga de ONNX Runtime para fallar rápido sin red si falta
/// lo que el propio repo debe aportar.
fn locate_sources(root: &Path) -> Result<Sources> {
    let exe_suffix = std::env::consts::EXE_SUFFIX;
    let binary = root
        .join("target")
        .join("release")
        .join(format!("ai-voice-interconnector{exe_suffix}"));
    if !binary.is_file() {
        bail!("binario no encontrado tras compilar: {}", binary.display());
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
    Ok(Sources { binary })
}

/// Comprueba la salida de `version --json`: el nombre del producto, la versión
/// del Cargo.toml y la `schema_version` del envelope vigente de la CLI. Pura,
/// fijada por test.
fn check_version_json(raw: &str, version: &str, schema_version: &str) -> Result<()> {
    let value: serde_json::Value = serde_json::from_str(raw)
        .map_err(|e| anyhow::anyhow!("`version --json` no es JSON válido ({e}): {raw}"))?;
    for (field, expected) in [
        ("name", "ai-voice-interconnector"),
        ("version", version),
        ("schema_version", schema_version),
    ] {
        let actual = value.get(field).and_then(|v| v.as_str());
        if actual != Some(expected) {
            bail!("`version --json`: `{field}` es {actual:?} y debe ser {expected:?}: {raw}");
        }
    }
    Ok(())
}

/// Comprueba que la salida de `--json voice list` registre la voz de fábrica
/// `default`. Pura, fijada por test.
fn check_voice_list_json(raw: &str) -> Result<()> {
    let value: serde_json::Value = serde_json::from_str(raw)
        .map_err(|e| anyhow::anyhow!("`voice list --json` no es JSON válido ({e}): {raw}"))?;
    let has_default = value
        .get("voices")
        .and_then(|v| v.as_array())
        .is_some_and(|voices| voices.iter().any(|v| v.as_str() == Some("default")));
    if !has_default {
        bail!("`voice list --json` no registra la voz de fábrica `default`: {raw}");
    }
    Ok(())
}

/// Humo del binario recién compilado: contrato de `version --json` con la
/// versión del Cargo.toml, voz de fábrica en `voice list`, `speech denoise` y el
/// rechazo del comando raíz `denoise`. Corre con la raíz de datos y el
/// contenedor de caché en un temporal propio, así que empaquetar nunca toca el
/// estado del usuario.
fn smoke_binary(binary: &Path, version: &str) -> Result<()> {
    let state = tempfile::Builder::new()
        .prefix("xtask-package-smoke-")
        .tempdir()?;
    let run = |args: &[&str]| -> Result<std::process::Output> {
        std::process::Command::new(binary)
            .args(args)
            .env("AVI_DATA_DIR", state.path().join("data"))
            .env("AVI_CACHE_DIR", state.path().join("cache"))
            .output()
            .map_err(|e| anyhow::anyhow!("no se pudo ejecutar {}: {e}", binary.display()))
    };
    let succeed = |args: &[&str]| -> Result<String> {
        let out = run(args)?;
        if !out.status.success() {
            bail!(
                "el humo falló: {} {} (exit {:?}): {}",
                binary.display(),
                args.join(" "),
                out.status.code(),
                String::from_utf8_lossy(&out.stderr)
            );
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    };
    check_version_json(
        &succeed(&["version", "--json"])?,
        version,
        avi_core::json_emitter::CLI_SCHEMA_VERSION,
    )?;
    check_voice_list_json(&succeed(&["--json", "voice", "list"])?)?;
    succeed(&["speech", "denoise", "--help"])?;
    if run(&["denoise", "--help"])?.status.success() {
        bail!(
            "el humo falló: {} aceptó el comando raíz `denoise`",
            binary.display()
        );
    }
    println!("humo del binario {version}: version --json, voice list, speech denoise y rechazo de `denoise`");
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
    engine: &Path,
    stage: &Path,
) -> Result<()> {
    let engine_rel = format!("qwen_tts{}", std::env::consts::EXE_SUFFIX);
    let ffmpeg_rel = format!("ffmpeg{}", std::env::consts::EXE_SUFFIX);
    for relative in &section.required {
        let dest = relative_path(stage, relative);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let src = if *relative == section.executable {
            sources.binary.clone()
        } else if *relative == engine_rel {
            engine.to_path_buf()
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
    let engine_rel = format!("qwen_tts{}", std::env::consts::EXE_SUFFIX);
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
    let engine_rel = format!("qwen_tts{}", std::env::consts::EXE_SUFFIX);
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
    cargo_args: Vec<String>,
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
    build_binary(&root, &cargo_args)?;
    let sources = locate_sources(&root)?;
    smoke_binary(&sources.binary, &version)?;
    let PinnedBundles {
        ort: bundle,
        ffmpeg: ffmpeg_bundle,
        engine: engine_bundle,
    } = ensure_pinned_bundles(&root, triple, &pins)?;
    for lib in ort_expected_libs(triple)? {
        if !bundle.join(lib).is_file() {
            bail!("librería no encontrada en {}: {lib}", bundle.display());
        }
    }
    let ffmpeg_binary = ffmpeg_bundle.join(ffmpeg_binary_name(triple)?);
    if !ffmpeg_binary.is_file() {
        bail!(
            "ejecutable de FFmpeg no encontrado en {}",
            ffmpeg_binary.display()
        );
    }
    let engine_binary = engine_bundle.join(engine_binary_name(triple)?);
    // Directorio de montaje con dueño: se borra al salir por cualquier camino.
    let stage_guard = tempfile::Builder::new()
        .prefix(&format!("xtask-package-{triple}-"))
        .tempdir()?;
    let stage = stage_guard.path().to_path_buf();
    stage_bundle(
        section,
        &sources,
        &root,
        &bundle,
        &ffmpeg_binary,
        &engine_binary,
        &stage,
    )?;
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
    // Constancia de cómo se configuró el FFmpeg de este paquete: algunos compiladores no
    // publican un manifiesto reproducible. Con `--flat` la salida es el árbol de instalación
    // y no admite archivos ajenos al manifiesto.
    if !flat {
        let buildconf = out_dir.join(ffmpeg_buildconf_name(&pins.ffmpeg, triple));
        std::fs::write(&buildconf, ffmpeg_buildconf(&ffmpeg_binary)?)?;
        println!("Configuración de build de FFmpeg: {}", buildconf.display());
    }
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
                "required": ["ai-voice-interconnector.exe", "LICENSE", "onnxruntime.dll", "ffmpeg.exe", "qwen_tts.exe"]
            },
            "x86_64-unknown-linux-gnu": {
                "executable": "ai-voice-interconnector",
                "required": ["ai-voice-interconnector", "LICENSE", "libonnxruntime.so", "ffmpeg", "qwen_tts"]
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
        // El motor llega de su propio bundle, fuera de la raíz del repositorio.
        let engine_bundle = base.join("engine-bundle");
        std::fs::create_dir_all(&engine_bundle).unwrap();
        let engine = format!("qwen_tts{}", std::env::consts::EXE_SUFFIX);
        std::fs::write(engine_bundle.join(&engine), b"motor").unwrap();
        let sources = Sources {
            binary: src.join(&section.executable),
        };
        // Orígenes de fixture en `src/`, documentos simulados en la raíz.
        let root = src.clone();
        stage_bundle(
            section,
            &sources,
            &root,
            &src,
            &src.join(&ffmpeg),
            &engine_bundle.join(&engine),
            &stage,
        )
        .unwrap();
        let mut staged = Vec::new();
        collect_relative(&stage, &stage, &mut staged).unwrap();
        staged.sort();
        let mut required = section.required.clone();
        required.sort();
        assert_eq!(staged, required);
        assert_eq!(
            std::fs::read(stage.join(&engine)).unwrap(),
            b"motor",
            "el motor del bundle sale de engine-bundle/"
        );
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

    /// La compilación es release con todos los features, acotada al paquete del
    /// producto, y reenvía al final los argumentos tras `--`.
    #[test]
    fn build_args_target_product_and_forward_extras() {
        let base = [
            "build",
            "--release",
            "--package",
            "ai-voice-interconnector",
            "--features",
            "full",
        ];
        assert_eq!(build_args(&[]), base);
        let extras = ["--verbose".to_string(), "--timings".to_string()];
        let mut expected: Vec<&str> = base.to_vec();
        expected.extend(["--verbose", "--timings"]);
        assert_eq!(build_args(&extras), expected);
    }

    /// `version --json` pasa con nombre, versión y `schema_version` esperados, y
    /// falla si la versión es otra (un binario viejo), si falta un campo o si no
    /// es JSON.
    #[test]
    fn version_json_contract() {
        let ok = r#"{"name":"ai-voice-interconnector","version":"0.46.1","schema_version":"6"}"#;
        check_version_json(ok, "0.46.1", "6").unwrap();
        let stale = check_version_json(&ok.replace("0.46.1", "0.45.2"), "0.46.1", "6");
        assert!(stale.unwrap_err().to_string().contains("version"));
        let schema = check_version_json(ok, "0.46.1", "7");
        assert!(schema.unwrap_err().to_string().contains("schema_version"));
        assert!(check_version_json(r#"{"version":"0.46.1"}"#, "0.46.1", "6").is_err());
        assert!(check_version_json("ai-voice-interconnector 0.46.1", "0.46.1", "6").is_err());
    }

    /// `voice list --json` pasa solo si registra `default`.
    #[test]
    fn voice_list_json_requires_default() {
        check_voice_list_json(r#"{"voices":["default","mia"]}"#).unwrap();
        assert!(check_voice_list_json(r#"{"voices":["mia"]}"#).is_err());
        assert!(check_voice_list_json(r#"{"status":"ok"}"#).is_err());
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

    /// Las comillas simples se duplican para no cerrar el literal de PowerShell.
    #[test]
    fn powershell_literal_escapes_single_quotes() {
        assert_eq!(powershell_single_quoted("a'b"), "a''b");
        assert_eq!(powershell_single_quoted("sin comillas"), "sin comillas");
    }

    /// La URL del paquete NuGet usa el id y la versión en minúsculas del
    /// contenedor plano de nuget.org, y las librerías esperadas del bundle son
    /// las canónicas de cada plataforma.
    #[test]
    fn ort_nupkg_download_url() {
        let (file, url) = ort_nupkg_download("1.28.0");
        assert_eq!(file, "microsoft.ml.onnxruntime.1.28.0.nupkg");
        assert_eq!(
            url,
            "https://api.nuget.org/v3-flatcontainer/microsoft.ml.onnxruntime/1.28.0/microsoft.ml.onnxruntime.1.28.0.nupkg"
        );
        assert!(ort_expected_libs("x86_64-pc-windows-msvc")
            .unwrap()
            .contains(&"onnxruntime.dll"));
        assert!(ort_expected_libs("x86_64-unknown-linux-gnu")
            .unwrap()
            .contains(&"libonnxruntime.so"));
    }

    /// Cada triple soportado tiene su entrada en el paquete NuGet y uno no
    /// soportado es un error.
    #[test]
    fn ort_package_entry_per_target() {
        assert_eq!(
            ort_package_entry("x86_64-pc-windows-msvc").unwrap(),
            "runtimes/win-x64/native/onnxruntime.dll"
        );
        assert_eq!(
            ort_package_entry("x86_64-unknown-linux-gnu").unwrap(),
            "runtimes/linux-x64/native/libonnxruntime.so"
        );
        assert_eq!(
            ort_package_entry("aarch64-unknown-linux-gnu").unwrap(),
            "runtimes/linux-arm64/native/libonnxruntime.so"
        );
        assert_eq!(
            ort_package_entry("aarch64-apple-darwin").unwrap(),
            "runtimes/osx-arm64/native/libonnxruntime.dylib"
        );
        assert!(ort_package_entry("riscv64gc-unknown-linux-gnu").is_err());
    }

    /// El tag de BtbN fijado es del último día de su mes: solo esos builds
    /// se conservan dos años; los diarios se borran tras 14.
    #[test]
    fn btbn_tag_is_the_last_day_of_its_month() {
        let parts: Vec<&str> = FFMPEG_BTBN_TAG
            .strip_prefix("autobuild-")
            .unwrap_or_default()
            .split('-')
            .collect();
        assert!(
            parts.len() == 5 && parts.iter().all(|p| p.parse::<u32>().is_ok()),
            "el tag de BtbN debe tener la forma autobuild-AAAA-MM-DD-HH-MM: {FFMPEG_BTBN_TAG}"
        );
        let year: u32 = parts[0].parse().unwrap();
        let month: u32 = parts[1].parse().unwrap();
        let day: u32 = parts[2].parse().unwrap();
        let leap =
            (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400);
        let last_day = match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 if leap => 29,
            2 => 28,
            _ => panic!("mes fuera de rango en el tag de BtbN: {FFMPEG_BTBN_TAG}"),
        };
        assert_eq!(
            day, last_day,
            "el tag de BtbN debe ser del último día de su mes ({year}-{month:02}-{last_day:02}); BtbN solo conserva esos builds dos años: {FFMPEG_BTBN_TAG}"
        );
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
        assert_eq!(file, "ffmpeg-n9.0.2-39-g7adfbe40dc-linux64-gpl-9.0.tar.xz");
        assert!(url.ends_with(&file) && url.contains(FFMPEG_BTBN_TAG));
        let (file, _) = ffmpeg_download("aarch64-unknown-linux-gnu", &version).unwrap();
        assert_eq!(
            file,
            "ffmpeg-n9.0.2-39-g7adfbe40dc-linuxarm64-gpl-9.0.tar.xz"
        );
        let (file, url) = ffmpeg_download("aarch64-apple-darwin", &version).unwrap();
        assert_eq!(file, "ffmpeg.zip");
        assert!(url.contains(&version));
    }

    /// Los assets de fuentes salen de la versión fijada y del commit de BtbN, no de literales.
    #[test]
    fn ffmpeg_sources_follow_the_pinned_version() {
        let sources = ffmpeg_sources("9.0.2");
        assert_eq!(sources[0].0, "ffmpeg-9.0.2.tar.xz");
        assert_eq!(
            sources[0].1,
            "https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz"
        );
        assert_eq!(
            sources[1].0,
            format!("ffmpeg-builds-{FFMPEG_BTBN_TAG}.tar.gz")
        );
        assert!(sources[1].1.contains(FFMPEG_BTBN_COMMIT));
        let other = ffmpeg_sources("10.1");
        assert_eq!(other[0].0, "ffmpeg-10.1.tar.xz");
        assert_eq!(other[1].0, sources[1].0);
    }

    /// Los hashes de `pins.json` corresponden exactamente a los assets de fuentes.
    #[test]
    fn the_repository_pins_a_hash_for_every_source_asset() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
        let pins = pins::load_from_root(&root).unwrap();
        let mut names: Vec<String> = ffmpeg_sources(&pins.ffmpeg)
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        names.sort();
        let pinned: Vec<String> = pins.ffmpeg_source_sha256.keys().cloned().collect();
        assert_eq!(pinned, names);
    }

    /// Sin hash fijado para un asset no se descarga nada: falla antes de tocar la red.
    #[test]
    fn fetching_sources_fails_when_a_hash_is_missing() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
        let mut pins = pins::load_from_root(&root).unwrap();
        let removed = format!("ffmpeg-{}.tar.xz", pins.ffmpeg);
        pins.ffmpeg_source_sha256.remove(&removed);
        let out = tempfile::tempdir().unwrap();
        let err = fetch_ffmpeg_sources(out.path(), &pins).unwrap_err();
        assert!(err.to_string().contains(&removed), "{err}");
        assert!(std::fs::read_dir(out.path()).unwrap().next().is_none());
    }

    /// Un hash fijado que ya no corresponde a ningún asset es una fijación obsoleta.
    #[test]
    fn fetching_sources_fails_on_a_pin_without_asset() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
        let mut pins = pins::load_from_root(&root).unwrap();
        pins.ffmpeg_source_sha256
            .insert("ffmpeg-0.0.tar.xz".to_string(), "0".repeat(64));
        let out = tempfile::tempdir().unwrap();
        let err = fetch_ffmpeg_sources(out.path(), &pins).unwrap_err();
        assert!(err.to_string().contains("ffmpeg-0.0.tar.xz"), "{err}");
    }

    #[test]
    fn the_buildconf_file_is_named_by_version_and_triple() {
        assert_eq!(
            ffmpeg_buildconf_name("9.0.2", "x86_64-pc-windows-msvc"),
            "ffmpeg-9.0.2-buildconf-x86_64-pc-windows-msvc.txt"
        );
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
        std::fs::write(bundle.join(ffmpeg_binary_name(triple).unwrap()), b"").unwrap();
        assert!(!ffmpeg_bundle_is_fresh(&bundle, triple, &pinned).unwrap());
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

    /// Cada target del manifiesto canónico tiene su entrada en el paquete NuGet
    /// de ONNX Runtime, el paquete tiene su SHA-256 fijado y bien formado, y la
    /// sección de cada target exige en el bundle todas las
    /// librerías que aporta `ort-bundle/` (en Windows, también las cinco DLL
    /// de VC++): empaquetado, instalación y pines no pueden divergir.
    #[test]
    fn manifest_targets_have_pinned_hash_and_bundle_libs() {
        let pins = pins::parse_text(include_str!("../../../packaging/pins.json"))
            .expect("pins.json válido");
        let sections =
            parse_manifest_text(include_str!("../../../packaging/bundle-manifest.json")).unwrap();
        for (triple, section) in &sections {
            ort_package_entry(triple).unwrap();
            for lib in ort_expected_libs(triple).unwrap() {
                assert!(
                    section.required.iter().any(|r| r == lib),
                    "el manifiesto de {triple} debe exigir {lib}"
                );
            }
        }
        pinned_ort_nupkg_sha256(&pins).unwrap();
        let windows = sections
            .get("x86_64-pc-windows-msvc")
            .expect("el manifiesto declara Windows x64");
        for dll in VC_DLLS {
            assert!(
                windows.required.iter().any(|r| r == dll),
                "el manifiesto de Windows debe exigir {dll}"
            );
        }
        assert!(VC_DLLS.contains(&"vcomp140.dll"));
        // La ONNX Runtime oficial de Windows importa `MSVCP140_1.dll`, que no
        // forma parte de Windows: sin ella no carga en un sistema sin el
        // Redistributable.
        assert!(VC_DLLS.contains(&"msvcp140_1.dll"));
    }

    /// El hash del paquete NuGet debe existir y ser hexadecimal en minúsculas
    /// de 64 dígitos: el fijado vale y uno ausente o mal formado es un error.
    #[test]
    fn pinned_hash_must_exist_and_be_lowercase_hex() {
        let mut pins = pins::parse_text(include_str!("../../../packaging/pins.json"))
            .expect("pins.json válido");
        assert!(pinned_ort_nupkg_sha256(&pins).is_ok());
        pins.ort_nupkg_sha256 = String::new();
        assert!(pinned_ort_nupkg_sha256(&pins).is_err());
        pins.ort_nupkg_sha256 = "AB".repeat(32);
        assert!(pinned_ort_nupkg_sha256(&pins).is_err());
        pins.ort_nupkg_sha256 = "ab".repeat(31);
        assert!(pinned_ort_nupkg_sha256(&pins).is_err());
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

    /// Un `ort-bundle/` con las librerías y el marcador con el hash del paquete
    /// se reutiliza; sin marcador, con un hash distinto o con librerías ausentes
    /// o vacías, no. Las DLL de VC++ no cuentan: se actualizan en cada ejecución.
    #[test]
    fn ort_bundle_freshness() {
        assert!(ort_expected_libs("x86_64-pc-windows-msvc")
            .unwrap()
            .iter()
            .all(|lib| !VC_DLLS.contains(lib)));
        let expected = "ab".repeat(32);
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
        // Sin marcador el origen es desconocido: no es fresco.
        assert!(!ort_bundle_is_fresh(&base, triple, &expected).unwrap());
        std::fs::write(base.join(ORT_MARKER), "cd".repeat(32)).unwrap();
        assert!(!ort_bundle_is_fresh(&base, triple, &expected).unwrap());
        std::fs::write(base.join(ORT_MARKER), &expected).unwrap();
        assert!(ort_bundle_is_fresh(&base, triple, &expected).unwrap());
        std::fs::remove_file(base.join(ort_expected_libs(triple).unwrap()[0])).unwrap();
        assert!(!ort_bundle_is_fresh(&base, triple, &expected).unwrap());
        // Vacía (descarga truncada) tampoco es fresca: fuerza reconstruir.
        std::fs::write(base.join(ort_expected_libs(triple).unwrap()[0]), b"").unwrap();
        assert!(!ort_bundle_is_fresh(&base, triple, &expected).unwrap());
    }

    /// Monta en `installation` un redist sintético de la versión `version` con
    /// las carpetas `Microsoft.VC143.CRT` y `Microsoft.VC143.OpenMP` y las DLL
    /// de [`VC_DLLS`]; devuelve el directorio `x64`.
    fn fake_vc_redist(installation: &Path, version: &str) -> PathBuf {
        let build = relative_path(installation, "VC/Auxiliary/Build");
        std::fs::create_dir_all(&build).unwrap();
        std::fs::write(
            build.join("Microsoft.VCRedistVersion.default.txt"),
            format!("{version}\r\n"),
        )
        .unwrap();
        let arch_dir = relative_path(installation, "VC/Redist/MSVC")
            .join(version)
            .join("x64");
        let crt = arch_dir.join("Microsoft.VC143.CRT");
        let openmp = arch_dir.join("Microsoft.VC143.OpenMP");
        std::fs::create_dir_all(&crt).unwrap();
        std::fs::create_dir_all(&openmp).unwrap();
        for dll in VC_DLLS {
            let dir = if *dll == VC_OPENMP_DLL { &openmp } else { &crt };
            std::fs::write(dir.join(dll), dll.as_bytes()).unwrap();
        }
        arch_dir
    }

    /// Una instalación válida resuelve las cinco DLL: `vcomp140.dll` de la
    /// carpeta OpenMP y las demás de la carpeta CRT de la versión declarada.
    #[test]
    fn vc_redist_resolves_dlls_from_declared_version() {
        let dir = tempfile::tempdir().unwrap();
        let arch_dir = fake_vc_redist(dir.path(), "14.44.35112");
        let dlls = vc_redist_dlls(dir.path()).unwrap();
        assert_eq!(dlls.len(), VC_DLLS.len());
        for (path, dll) in dlls.iter().zip(VC_DLLS) {
            let folder = if *dll == VC_OPENMP_DLL {
                "Microsoft.VC143.OpenMP"
            } else {
                "Microsoft.VC143.CRT"
            };
            assert_eq!(path, &arch_dir.join(folder).join(dll));
        }
    }

    /// Sin el archivo de versión, con una DLL ausente o con dos carpetas
    /// `Microsoft.VC*.CRT` la resolución falla nombrando la ruta.
    #[test]
    fn vc_redist_rejects_missing_version_dll_or_ambiguous_folder() {
        let dir = tempfile::tempdir().unwrap();
        let err = vc_redist_dlls(dir.path()).unwrap_err().to_string();
        assert!(
            err.contains("Microsoft.VCRedistVersion.default.txt"),
            "{err}"
        );

        let arch_dir = fake_vc_redist(dir.path(), "14.44.35112");
        std::fs::remove_file(arch_dir.join("Microsoft.VC143.CRT").join("msvcp140_1.dll")).unwrap();
        let err = vc_redist_dlls(dir.path()).unwrap_err().to_string();
        assert!(err.contains("msvcp140_1.dll"), "{err}");

        std::fs::create_dir_all(arch_dir.join("Microsoft.VC150.CRT")).unwrap();
        let err = vc_redist_dlls(dir.path()).unwrap_err().to_string();
        assert!(err.contains("Microsoft.VC*.CRT"), "{err}");
    }

    /// La copia no escribe si el destino es idéntico y sobrescribe si difiere.
    #[test]
    fn copy_if_differs_writes_only_changed_content() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.dll");
        let dest = dir.path().join("dest.dll");
        std::fs::write(&src, b"nueva").unwrap();
        assert!(copy_if_differs(&src, &dest).unwrap(), "destino ausente");
        assert!(!copy_if_differs(&src, &dest).unwrap(), "destino idéntico");
        std::fs::write(&dest, b"vieja").unwrap();
        assert!(copy_if_differs(&src, &dest).unwrap(), "destino distinto");
        assert_eq!(std::fs::read(&dest).unwrap(), b"nueva");
    }

    fn write_notices(dir: &Path, license: &str, third_party: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("LICENSE"), license).unwrap();
        std::fs::write(dir.join("ThirdPartyNotices.txt"), third_party).unwrap();
    }

    /// Paquete NuGet sintético con la estructura del real: la biblioteca de
    /// cada plataforma bajo `runtimes/`, los avisos en la raíz y un `LICENSE`
    /// homónimo en un subdirectorio que no debe tomarse.
    fn write_ort_nupkg(path: &Path, entries: &[(&str, &str)]) {
        use std::io::Write;
        let file = std::fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        for (name, data) in entries {
            zip.start_file(*name, options).unwrap();
            zip.write_all(data.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }

    /// Entradas de un paquete NuGet completo con la biblioteca del triple.
    fn full_nupkg_entries<'a>(
        library_entry: &'a str,
        license: &'a str,
        third_party: &'a str,
    ) -> Vec<(&'a str, &'a str)> {
        vec![
            ("runtimes/otra/native/onnxruntime.dll", "OTRA"),
            (library_entry, "REAL"),
            ("LICENSE", license),
            ("ThirdPartyNotices.txt", third_party),
            ("docs/LICENSE", "homónimo de un subdirectorio"),
        ]
    }

    /// La extracción toma la biblioteca exacta del triple, en Windows y en
    /// Linux, la escribe con su nombre canónico y acepta los avisos con CRLF
    /// frente a las copias versionadas en LF.
    #[test]
    fn extract_takes_the_triple_entry_and_accepts_crlf_notices() {
        for (triple, entry, canonical) in [
            (
                "x86_64-pc-windows-msvc",
                "runtimes/win-x64/native/onnxruntime.dll",
                "onnxruntime.dll",
            ),
            (
                "x86_64-unknown-linux-gnu",
                "runtimes/linux-x64/native/libonnxruntime.so",
                "libonnxruntime.so",
            ),
        ] {
            let base_guard = tempfile::tempdir().unwrap();
            let base = base_guard.path();
            let archive = base.join("ort.nupkg");
            write_ort_nupkg(
                &archive,
                &full_nupkg_entries(entry, "linea1\r\nlinea2\r\n", "avisos\r\n"),
            );
            let notices = base.join("notices");
            write_notices(&notices, "linea1\nlinea2\n", "avisos\n");
            extract_ort_library(&archive, &base.join("bundle"), triple, "1.28.0", &notices)
                .unwrap();
            assert_eq!(
                std::fs::read(base.join("bundle").join(canonical)).unwrap(),
                b"REAL",
                "{triple}"
            );
        }
    }

    /// Un aviso distinto en el paquete falla nombrando el archivo, y no deja
    /// librería extraída. El `LICENSE` de un subdirectorio no se toma.
    #[test]
    fn extract_rejects_notices_that_differ_from_the_versioned_copies() {
        let base_guard = tempfile::tempdir().unwrap();
        let base = base_guard.path();
        let archive = base.join("ort.nupkg");
        write_ort_nupkg(
            &archive,
            &full_nupkg_entries(
                "runtimes/win-x64/native/onnxruntime.dll",
                "licencia nueva",
                "avisos",
            ),
        );
        let notices = base.join("notices");
        write_notices(&notices, "licencia vieja", "avisos");
        let err = extract_ort_library(
            &archive,
            &base.join("bundle"),
            "x86_64-pc-windows-msvc",
            "1.28.0",
            &notices,
        )
        .unwrap_err();
        assert!(err.to_string().contains("LICENSE"), "{err}");
        assert!(!base.join("bundle/onnxruntime.dll").exists());
    }

    /// Un paquete sin los avisos en la raíz también falla: no se da por bueno
    /// lo que no se puede comparar, y un homónimo de subdirectorio no los suple.
    #[test]
    fn extract_rejects_a_package_without_notices() {
        let base_guard = tempfile::tempdir().unwrap();
        let base = base_guard.path();
        let archive = base.join("ort.nupkg");
        write_ort_nupkg(
            &archive,
            &[
                ("runtimes/win-x64/native/onnxruntime.dll", "DLL"),
                ("docs/LICENSE", "homónimo de un subdirectorio"),
            ],
        );
        let notices = base.join("notices");
        write_notices(&notices, "a", "b");
        let err = extract_ort_library(
            &archive,
            &base.join("bundle"),
            "x86_64-pc-windows-msvc",
            "1.28.0",
            &notices,
        )
        .unwrap_err();
        assert!(err.to_string().contains("no trae LICENSE"), "{err}");
    }

    /// Falta la biblioteca del triple o mide cero bytes: error que nombra la entrada.
    #[test]
    fn extract_rejects_a_missing_or_empty_library() {
        let base_guard = tempfile::tempdir().unwrap();
        let base = base_guard.path();
        let notices = base.join("notices");
        write_notices(&notices, "a", "b");
        let entry = "runtimes/win-x64/native/onnxruntime.dll";
        for (entries, expected) in [
            (
                vec![
                    ("runtimes/linux-x64/native/libonnxruntime.so", "SO"),
                    ("LICENSE", "a"),
                    ("ThirdPartyNotices.txt", "b"),
                ],
                "no trae la entrada",
            ),
            (
                vec![
                    (entry, ""),
                    ("LICENSE", "a"),
                    ("ThirdPartyNotices.txt", "b"),
                ],
                "mide 0 bytes",
            ),
        ] {
            let archive = base.join("ort.nupkg");
            write_ort_nupkg(&archive, &entries);
            let err = extract_ort_library(
                &archive,
                &base.join("bundle"),
                "x86_64-pc-windows-msvc",
                "1.28.0",
                &notices,
            )
            .unwrap_err();
            let text = err.to_string();
            assert!(text.contains(entry) && text.contains(expected), "{text}");
            assert!(!base.join("bundle/onnxruntime.dll").exists());
        }
    }

    /// Las copias versionadas del repositorio existen para la versión fijada.
    #[test]
    fn the_pinned_onnx_runtime_notices_exist_in_the_repository() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
        let pins = pins::load_from_root(&root).unwrap();
        let dir = ort_notices_dir(&root, &pins.ort);
        for name in ORT_NOTICE_FILES {
            assert!(
                dir.join(name).is_file(),
                "falta {}",
                dir.join(name).display()
            );
        }
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
        std::fs::create_dir_all(&stage).unwrap();
        std::fs::write(stage.join("ai-voice-interconnector"), b"bin").unwrap();
        std::fs::write(stage.join("LICENSE"), b"lic").unwrap();
        std::fs::write(stage.join("qwen_tts"), b"motor").unwrap();
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
        assert_eq!(names, ["LICENSE", "ai-voice-interconnector", "qwen_tts"]);
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
            Vec::new(),
        )
        .expect_err("`--flat` sin `--no-compress` debe fallar");
        assert!(
            err.to_string().contains("--flat"),
            "el mensaje nombra la bandera: {err}"
        );
    }

    /// Triple del host para nombrar el ejecutable de FFmpeg en las pruebas.
    fn host_ffmpeg_name() -> (&'static str, &'static str) {
        let triple = host_triple().unwrap();
        (triple, ffmpeg_binary_name(triple).unwrap())
    }

    /// La colocación crea la carpeta de destino inexistente y deja allí una
    /// copia byte a byte del ejecutable del bundle.
    #[test]
    fn stage_ffmpeg_beside_creates_dest_and_copies() {
        let (triple, name) = host_ffmpeg_name();
        let bundle = tempfile::tempdir().unwrap();
        std::fs::write(bundle.path().join(name), b"ffmpeg-falso").unwrap();
        let out = tempfile::tempdir().unwrap();
        let dest_dir = out.path().join("target").join("debug");
        let staged = stage_ffmpeg_beside(bundle.path(), &dest_dir, triple).unwrap();
        assert_eq!(staged, dest_dir.join(name));
        assert_eq!(std::fs::read(&staged).unwrap(), b"ffmpeg-falso");
    }

    /// Una segunda colocación sobrescribe la copia previa, de modo que un
    /// cambio de pin no deja un FFmpeg viejo junto al binario de prueba.
    #[test]
    fn stage_ffmpeg_beside_overwrites_previous_copy() {
        let (triple, name) = host_ffmpeg_name();
        let bundle = tempfile::tempdir().unwrap();
        let dest_dir = tempfile::tempdir().unwrap();
        std::fs::write(bundle.path().join(name), b"version-vieja").unwrap();
        stage_ffmpeg_beside(bundle.path(), dest_dir.path(), triple).unwrap();
        std::fs::write(bundle.path().join(name), b"version-nueva").unwrap();
        let staged = stage_ffmpeg_beside(bundle.path(), dest_dir.path(), triple).unwrap();
        assert_eq!(std::fs::read(&staged).unwrap(), b"version-nueva");
    }

    /// En Unix la copia queda ejecutable aunque el origen no lo fuera.
    #[cfg(unix)]
    #[test]
    fn stage_ffmpeg_beside_sets_executable_mode() {
        use std::os::unix::fs::PermissionsExt;
        let (triple, name) = host_ffmpeg_name();
        let bundle = tempfile::tempdir().unwrap();
        let source = bundle.path().join(name);
        std::fs::write(&source, b"ffmpeg-falso").unwrap();
        std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o644)).unwrap();
        let dest_dir = tempfile::tempdir().unwrap();
        let staged = stage_ffmpeg_beside(bundle.path(), dest_dir.path(), triple).unwrap();
        let mode = std::fs::metadata(&staged).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755);
    }

    /// La carpeta antepuesta queda primera y las entradas originales se
    /// conservan en su orden; sin `PATH` original solo queda la carpeta.
    #[test]
    fn path_with_prepended_puts_dir_first_and_keeps_order() {
        let dir = Path::new("bundle-ffmpeg");
        let original = std::env::join_paths([Path::new("uno"), Path::new("dos")]).unwrap();
        let joined = path_with_prepended(dir, Some(&original)).unwrap();
        let entries: Vec<PathBuf> = std::env::split_paths(&joined).collect();
        assert_eq!(
            entries,
            vec![
                PathBuf::from("bundle-ffmpeg"),
                PathBuf::from("uno"),
                PathBuf::from("dos")
            ]
        );
        let alone = path_with_prepended(dir, None).unwrap();
        let entries: Vec<PathBuf> = std::env::split_paths(&alone).collect();
        assert_eq!(entries, vec![PathBuf::from("bundle-ffmpeg")]);
    }

    /// En una raíz sin bundles, todos los recursos fijados se informan como no
    /// frescos, sin error y sin crear nada.
    #[test]
    fn pinned_bundles_status_reports_missing_bundles_as_stale() {
        let root = tempfile::tempdir().unwrap();
        let pins = pins::parse_text(include_str!("../../../packaging/pins.json"))
            .expect("pins.json válido");
        let triple = host_triple().unwrap();
        let PinnedBundles {
            ort,
            ffmpeg,
            engine,
        } = pinned_bundles_status(root.path(), triple, &pins);
        assert!(!ort.unwrap());
        assert!(!ffmpeg.unwrap());
        assert!(!engine.unwrap());
        assert!(!ort_bundle_dir(root.path()).exists());
        assert!(!ffmpeg_bundle_dir(root.path()).exists());
        assert!(!engine_bundle_dir(root.path()).exists());
    }

    /// Un triple sin hash de FFmpeg fijado es un error solo de FFmpeg: el estado
    /// de ONNX Runtime se sigue informando.
    #[test]
    fn pinned_bundles_status_isolates_a_missing_ffmpeg_pin() {
        let root = tempfile::tempdir().unwrap();
        let mut pins = pins::parse_text(include_str!("../../../packaging/pins.json"))
            .expect("pins.json válido");
        let triple = host_triple().unwrap();
        pins.ffmpeg_sha256.remove(triple);
        let PinnedBundles {
            ort,
            ffmpeg,
            engine,
        } = pinned_bundles_status(root.path(), triple, &pins);
        assert!(!ort.unwrap());
        assert!(ffmpeg.is_err());
        assert!(!engine.unwrap());
    }

    /// Nombre y URL del paquete del motor para cada target de AVI: el de
    /// Windows es el artefacto GNU (es el único que el motor publica), el
    /// resto conserva su triple, y solo Windows usa `.zip`.
    #[test]
    fn engine_download_per_target() {
        for (triple, extension) in [
            ("x86_64-pc-windows-msvc", "zip"),
            ("x86_64-unknown-linux-gnu", "tar.gz"),
            ("aarch64-unknown-linux-gnu", "tar.gz"),
            ("aarch64-apple-darwin", "tar.gz"),
        ] {
            let (file, url) = engine_download(triple, "1.0.0").unwrap();
            let engine = engine_triple(triple).unwrap();
            assert_eq!(file, format!("qwen-tts-engine-v1.0.0-{engine}.{extension}"));
            assert_eq!(
                url,
                format!("https://github.com/{ENGINE_REPO}/releases/download/v1.0.0/{file}")
            );
        }
        assert_eq!(
            engine_triple("x86_64-pc-windows-msvc").unwrap(),
            "x86_64-pc-windows-gnu"
        );
        assert!(engine_download("riscv64gc-unknown-linux-gnu", "1.0.0").is_err());
    }

    /// El bundle del motor está fresco solo con el ejecutable no vacío y el
    /// marcador del hash fijado; cambiar el pin o vaciar el binario lo
    /// invalida.
    #[test]
    fn engine_bundle_freshness_requires_binary_and_matching_marker() {
        let root = tempfile::tempdir().unwrap();
        let bundle = engine_bundle_dir(root.path());
        std::fs::create_dir_all(&bundle).unwrap();
        let triple = host_triple().unwrap();
        let binary = bundle.join(engine_binary_name(triple).unwrap());
        let pinned = "a".repeat(64);
        assert!(!engine_bundle_is_fresh(&bundle, triple, &pinned).unwrap());
        std::fs::write(&binary, b"motor").unwrap();
        std::fs::write(bundle.join(ENGINE_MARKER), &pinned).unwrap();
        assert!(engine_bundle_is_fresh(&bundle, triple, &pinned).unwrap());
        std::fs::write(bundle.join(ENGINE_MARKER), "b".repeat(64)).unwrap();
        assert!(!engine_bundle_is_fresh(&bundle, triple, &pinned).unwrap());
        std::fs::write(&binary, b"").unwrap();
        std::fs::write(bundle.join(ENGINE_MARKER), &pinned).unwrap();
        assert!(!engine_bundle_is_fresh(&bundle, triple, &pinned).unwrap());
    }

    /// Un target sin hash del motor fijado es un error nombrado, no una
    /// descarga sin verificar.
    #[test]
    fn pinned_engine_sha256_rejects_missing_and_malformed_hashes() {
        let mut pins = pins::parse_text(include_str!("../../../packaging/pins.json"))
            .expect("pins.json válido");
        let triple = host_triple().unwrap();
        pinned_engine_sha256(&pins, triple).unwrap();
        pins.qwen_tts_engine_sha256.remove(triple);
        let err = pinned_engine_sha256(&pins, triple).unwrap_err().to_string();
        assert!(
            err.contains(triple) && err.contains("qwen-tts-engine"),
            "{err}"
        );
        pins.qwen_tts_engine_sha256
            .insert(triple.to_string(), "ABC".to_string());
        assert!(pinned_engine_sha256(&pins, triple).is_err());
    }

    /// Paquete sintético del motor: la carpeta con la versión y el triple trae
    /// el ejecutable y los tres avisos, y la extracción deja los cuatro en la
    /// raíz del bundle.
    #[test]
    fn extract_engine_bundle_flattens_the_package_folder() {
        let root = tempfile::tempdir().unwrap();
        let version = "1.0.0";
        let engine = "x86_64-unknown-linux-gnu";
        let folder = format!("qwen-tts-engine-v{version}-{engine}");
        let archive = root.path().join(format!("{folder}.tar.gz"));
        {
            let file = std::fs::File::create(&archive).unwrap();
            let gz = flate2::write::GzEncoder::new(file, flate2::Compression::default());
            let mut tar = tar::Builder::new(gz);
            for (name, data) in [
                ("qwen_tts", "MOTOR"),
                ("LICENSE", "MPL"),
                ("NOTICE", "aviso"),
                ("THIRD-PARTY-LICENSES.md", "terceros"),
            ] {
                let mut header = tar::Header::new_gnu();
                header.set_size(data.len() as u64);
                header.set_mode(0o644);
                header.set_cksum();
                tar.append_data(&mut header, format!("{folder}/{name}"), data.as_bytes())
                    .unwrap();
            }
            let gz = tar.into_inner().unwrap();
            gz.finish().unwrap();
        }
        let bundle = engine_bundle_dir(root.path());
        std::fs::create_dir_all(&bundle).unwrap();
        extract_engine_bundle(&archive, &bundle, "x86_64-unknown-linux-gnu").unwrap();
        for (name, data) in [
            ("qwen_tts", "MOTOR"),
            ("LICENSE", "MPL"),
            ("NOTICE", "aviso"),
            ("THIRD-PARTY-LICENSES.md", "terceros"),
        ] {
            assert_eq!(
                std::fs::read_to_string(bundle.join(name)).unwrap(),
                data,
                "{name}"
            );
        }
    }

    /// El paquete de Windows es un zip y la extracción también deja los cuatro
    /// archivos en la raíz; un paquete al que le falta un aviso se rechaza.
    #[test]
    fn extract_engine_bundle_reads_zip_and_rejects_incomplete_packages() {
        use std::io::Write;
        let root = tempfile::tempdir().unwrap();
        let version = "1.0.0";
        let folder = format!("qwen-tts-engine-v{version}-x86_64-pc-windows-gnu");
        let archive = root.path().join(format!("{folder}.zip"));
        let write_zip = |members: &[(&str, &str)]| {
            let file = std::fs::File::create(&archive).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let options = zip::write::SimpleFileOptions::default();
            for (name, data) in members {
                zip.start_file(format!("{folder}/{name}"), options).unwrap();
                zip.write_all(data.as_bytes()).unwrap();
            }
            zip.finish().unwrap();
        };
        write_zip(&[
            ("qwen_tts.exe", "MOTOR"),
            ("LICENSE", "MPL"),
            ("NOTICE", "aviso"),
            ("THIRD-PARTY-LICENSES.md", "terceros"),
        ]);
        let bundle = engine_bundle_dir(root.path());
        std::fs::create_dir_all(&bundle).unwrap();
        extract_engine_bundle(&archive, &bundle, "x86_64-pc-windows-msvc").unwrap();
        assert_eq!(
            std::fs::read_to_string(bundle.join("qwen_tts.exe")).unwrap(),
            "MOTOR"
        );
        assert!(bundle.join("NOTICE").is_file());

        let incomplete = engine_bundle_dir(&root.path().join("otro"));
        std::fs::create_dir_all(&incomplete).unwrap();
        write_zip(&[("qwen_tts.exe", "MOTOR"), ("LICENSE", "MPL")]);
        let err = extract_engine_bundle(&archive, &incomplete, "x86_64-pc-windows-msvc")
            .unwrap_err()
            .to_string();
        assert!(err.contains("NOTICE"), "{err}");
    }
}
