//! Descarga, verificación y extracción del bundle objetivo: los pasos 7 y 8 de la
//! actualización.
//!
//! Convierte una versión objetivo en un bundle nuevo arrancable en staging, con
//! integridad garantizada por `SHA256SUMS.txt` (sin firma, por D7). El staging es
//! hermano del directorio de programa (mismo volumen) con permisos solo del
//! usuario, y se borra ante cualquier fallo posterior a su creación: un fallo
//! antes del traspaso deja la instalación intacta.
//!
//! El transporte es HTTPS con TLS ≥ 1.2 (`reqwest` con `rustls`, sin OpenSSL del
//! sistema: el mínimo lo impone `rustls` por defecto), reintentos acotados con
//! espera creciente y respeto de `HTTPS_PROXY`, `NO_PROXY` y el proxy del sistema
//! en Windows (feature `system-proxy`).

use crate::{manifest, target, LifecycleError, STAGING_DIR_PREFIX};
#[cfg(unix)]
use avi_shared::manifest::relative_path;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Intentos máximos por petición HTTP, incluida la primera.
pub const MAX_ATTEMPTS: u32 = 4;

/// Espera base entre reintentos: el intento `n` espera `n` veces esta base.
pub const RETRY_BASE_DELAY_MS: u64 = 250;

/// Plazo máximo de una petición HTTP completa.
pub const REQUEST_TIMEOUT_SECS: u64 = 30;

/// Nombre del inventario de hashes del release, el que publica GitHub.
pub const SUMS_FILE_NAME: &str = "SHA256SUMS.txt";

/// Entrada de la descarga: directorio de programa (para el staging hermano),
/// triple del target y versión objetivo ya resuelta.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchRequest {
    pub program_dir: PathBuf,
    pub target: String,
    pub version: String,
}

/// Bundle nuevo verificado y extraído en staging, listo para el arranque de
/// comprobación y el traspaso.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedBundle {
    pub staging: PathBuf,
    pub asset_name: String,
}

/// Cliente HTTPS compartido por la resolución y la descarga, con TLS de `rustls`
/// (mínimo 1.2 por defecto), `User-Agent` propio (lo exige la API de GitHub),
/// plazo acotado y redirecciones limitadas. Los proxies del entorno y del
/// sistema los aplica `reqwest` por defecto.
pub fn http_client() -> Result<reqwest::Client, LifecycleError> {
    reqwest::Client::builder()
        .user_agent("ai-voice-interconnector self-update")
        .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .map_err(|e| {
            LifecycleError::network_error(format!("no se pudo crear el cliente HTTPS: {e}"))
        })
}

/// Espera antes del reintento `attempt` (empezando en 1): creciente y acotada.
pub(crate) fn retry_delay(attempt: u32) -> Duration {
    Duration::from_millis(RETRY_BASE_DELAY_MS * attempt.max(1) as u64)
}

/// `GET` con reintentos acotados: se reintenta el error de transporte y el 5xx;
/// el resto de estados no exitosos falla ya con `network_error`.
pub(crate) async fn get_with_retry(
    client: &reqwest::Client,
    url: &str,
) -> Result<reqwest::Response, LifecycleError> {
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        match client.get(url).send().await {
            Ok(reply) => {
                if reply.status().is_success() {
                    return Ok(reply);
                }
                if !reply.status().is_server_error() || attempt >= MAX_ATTEMPTS {
                    return Err(LifecycleError::network_error(format!(
                        "la descarga de {url} devolvió {}",
                        reply.status()
                    )));
                }
            }
            Err(e) => {
                if attempt >= MAX_ATTEMPTS {
                    return Err(LifecycleError::network_error(format!(
                        "la descarga de {url} falló tras {attempt} intentos: {e}"
                    )));
                }
            }
        }
        tokio::time::sleep(retry_delay(attempt)).await;
    }
}

/// URL del archivo de release para `version` en `triple`, con la convención de la
/// tabla de targets soportados (`releases/download/vX.Y.Z/…`).
pub fn asset_url(version: &str, triple: &str) -> Result<String, LifecycleError> {
    let asset = target::release_asset_name(triple, version)?;
    Ok(format!(
        "{}/releases/download/v{version}/{asset}",
        crate::update_resolve::download_base_url()
    ))
}

/// URL del `SHA256SUMS.txt` del release de `version`.
pub fn sums_url(version: &str) -> String {
    format!(
        "{}/releases/download/v{version}/{SUMS_FILE_NAME}",
        crate::update_resolve::download_base_url()
    )
}

/// Directorio de staging: hermano del programa (mismo volumen, para que el
/// traspaso no cruce volúmenes) con el prefijo propio de la tabla de rutas.
///
/// Es seguro por construcción: la versión viaja pegada al prefijo dentro de un
/// único componente, así que no puede escapar al padre haga lo que haga.
pub fn staging_dir_for(program_dir: &Path, version: &str) -> PathBuf {
    let parent = program_dir.parent().unwrap_or_else(|| Path::new("."));
    parent.join(format!("{STAGING_DIR_PREFIX}update-{version}"))
}

/// Descarga el archivo y el `SHA256SUMS.txt` de `request`, verifica la
/// integridad, extrae con permisos 0755/0644 y valida contra el manifiesto.
///
/// Ante cualquier fallo tras crear el staging, lo borra y devuelve el error:
/// nada más queda modificado.
pub async fn fetch(
    client: &reqwest::Client,
    request: &FetchRequest,
) -> Result<StagedBundle, LifecycleError> {
    let staging = staging_dir_for(&request.program_dir, &request.version);
    if staging.exists() {
        std::fs::remove_dir_all(&staging).map_err(|e| {
            LifecycleError::network_error(format!(
                "no se pudo limpiar el staging previo {}: {e}",
                staging.display()
            ))
        })?;
    }
    std::fs::create_dir_all(&staging).map_err(|e| {
        LifecycleError::network_error(format!(
            "no se pudo crear el staging {}: {e}",
            staging.display()
        ))
    })?;
    restrict_staging(&staging);
    let outcome = fetch_into(client, request, &staging).await;
    if outcome.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    outcome
}

/// Núcleo de [`fetch`] con el staging ya creado. Separado para que el borrado
/// ante fallo viva en un solo sitio.
async fn fetch_into(
    client: &reqwest::Client,
    request: &FetchRequest,
    staging: &Path,
) -> Result<StagedBundle, LifecycleError> {
    let asset = target::release_asset_name(&request.target, &request.version)?;
    let asset_reply =
        get_with_retry(client, &asset_url(&request.version, &request.target)?).await?;
    let asset_path = staging.join(&asset);
    write_streamed(asset_reply, &asset_path).await?;
    let sums_reply = get_with_retry(client, &sums_url(&request.version)).await?;
    let sums_bytes = sums_reply.bytes().await.map_err(|e| {
        LifecycleError::network_error(format!("el {SUMS_FILE_NAME} no se pudo leer: {e}"))
    })?;
    let sums_text = String::from_utf8_lossy(&sums_bytes);
    verify_sums(&sums_text, &asset, &asset_path)?;
    extract_bundle(&asset_path, staging)?;
    apply_permissions(&request.target, staging)?;
    manifest::validate_bundle(&request.target, staging)?;
    Ok(StagedBundle {
        staging: staging.to_path_buf(),
        asset_name: asset,
    })
}

/// Arranca `<staging>/… --version` y exige que informe `version`.
///
/// Un binario que no ejecuta, falla o informa otra versión es
/// `binary_incompatible`, con el diagnóstico que nombra el triple y remite a la
/// documentación de compilación.
pub async fn verify_boot(
    staging: &Path,
    triple: &str,
    version: &str,
) -> Result<(), LifecycleError> {
    let section = manifest::target_section(triple)?;
    let exe = staging.join(section.executable_path());
    let output = tokio::process::Command::new(&exe)
        .arg("--version")
        .output()
        .await
        .map_err(|e| {
            LifecycleError::binary_incompatible(format!(
                "el binario descargado ({triple}) no se puede ejecutar ({}): {e}. \
                 Compila {APP} desde el código fuente (docs/BUILD.md)",
                exe.display(),
            ))
        })?;
    if !output.status.success() {
        return Err(LifecycleError::binary_incompatible(format!(
            "el binario descargado ({triple}) terminó con estado {} al comprobar \
             `--version`: compila {APP} desde el código fuente (docs/BUILD.md)",
            output.status.code().unwrap_or(-1),
        )));
    }
    let reported = String::from_utf8_lossy(&output.stdout);
    if reported.contains(version) {
        return Ok(());
    }
    let first: String = reported
        .lines()
        .next()
        .unwrap_or_default()
        .chars()
        .take(200)
        .collect();
    Err(LifecycleError::binary_incompatible(format!(
        "el binario descargado ({triple}) informa '{first}' en vez de {version}: \
         compila {APP} desde el código fuente (docs/BUILD.md)"
    )))
}

/// Guarda el cuerpo de `reply` en `dest` por tramos, sin cargarlo entero en
/// memoria.
async fn write_streamed(mut reply: reqwest::Response, dest: &Path) -> Result<(), LifecycleError> {
    use tokio::io::AsyncWriteExt;
    let mut file = tokio::fs::File::create(dest).await.map_err(|e| {
        LifecycleError::network_error(format!("no se pudo escribir {}: {e}", dest.display()))
    })?;
    loop {
        let chunk = reply.chunk().await.map_err(|e| {
            LifecycleError::network_error(format!("la descarga se interrumpió: {e}"))
        })?;
        let Some(bytes) = chunk else {
            break;
        };
        file.write_all(&bytes).await.map_err(|e| {
            LifecycleError::network_error(format!("no se pudo escribir {}: {e}", dest.display()))
        })?;
    }
    file.flush().await.map_err(|e| {
        LifecycleError::network_error(format!("no se pudo escribir {}: {e}", dest.display()))
    })?;
    Ok(())
}

/// Verifica el archivo contra el texto de `SHA256SUMS.txt`.
///
/// La entrada se busca por coincidencia exacta del nombre (cadenas, sin regex),
/// tolerando solo el marcador `*` de modo binario de `sha256sum`; el hash se
/// compara en minúsculas. Entrada ausente o hash distinto es `checksum_mismatch`.
pub fn verify_sums(
    sums_text: &str,
    asset_name: &str,
    asset_path: &Path,
) -> Result<(), LifecycleError> {
    let bytes = std::fs::read(asset_path).map_err(|e| {
        LifecycleError::network_error(format!(
            "no se pudo leer el archivo descargado {}: {e}",
            asset_path.display()
        ))
    })?;
    verify_sums_bytes(sums_text, asset_name, &bytes)
}

/// Núcleo comprobable de [`verify_sums`], con los bytes como dato para que las
/// pruebas usen fixtures locales sin disco.
pub fn verify_sums_bytes(
    sums_text: &str,
    asset_name: &str,
    asset_bytes: &[u8],
) -> Result<(), LifecycleError> {
    let expected = sums_text
        .lines()
        .filter_map(parse_sums_line)
        .find(|(_, name)| name == asset_name)
        .map(|(hash, _)| hash)
        .ok_or_else(|| {
            LifecycleError::checksum_mismatch(format!(
                "la entrada de {asset_name} falta en {SUMS_FILE_NAME}: \
                 el release no publica su hash"
            ))
        })?;
    let actual = hex_sha256(asset_bytes);
    if expected == actual {
        return Ok(());
    }
    Err(LifecycleError::checksum_mismatch(format!(
        "el hash de {asset_name} no coincide con {SUMS_FILE_NAME}: el archivo \
         puede estar corrupto o manipulado"
    )))
}

/// Una línea de `SHA256SUMS.txt` como `(hash en minúsculas, nombre)`, o `None`
/// si no tiene forma de entrada.
fn parse_sums_line(line: &str) -> Option<(String, String)> {
    let mut parts = line.split_whitespace();
    let hash = parts.next()?;
    let name = parts.next()?;
    if parts.next().is_some() || hash.len() != 64 || !hash.bytes().all(is_hex) {
        return None;
    }
    let name = name.strip_prefix('*').unwrap_or(name).to_string();
    Some((hash.to_lowercase(), name))
}

/// `true` si el byte es un dígito hexadecimal.
fn is_hex(byte: u8) -> bool {
    byte.is_ascii_hexdigit()
}

/// SHA-256 en hexadecimal minúsculo.
fn hex_sha256(bytes: &[u8]) -> String {
    use sha2::Digest;
    use std::fmt::Write;
    let digest = sha2::Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Extrae el archivo descargado en `staging`. `.zip` en Windows, `.tar.gz` en
/// Unix; las entradas que escapen de la raíz se rechazan con `bundle_invalid`.
fn extract_bundle(asset_path: &Path, staging: &Path) -> Result<(), LifecycleError> {
    let name = asset_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if name.ends_with(".zip") {
        extract_zip(asset_path, staging)
    } else if name.ends_with(".tar.gz") {
        extract_tar_gz(asset_path, staging)
    } else {
        Err(LifecycleError::bundle_invalid(format!(
            "el archivo descargado {name} no es `.zip` ni `.tar.gz`"
        )))
    }
}

/// Extrae un `.zip`, rechazando las entradas fuera de la raíz (`ZipSlip`).
fn extract_zip(asset_path: &Path, staging: &Path) -> Result<(), LifecycleError> {
    let file = std::fs::File::open(asset_path).map_err(|e| {
        LifecycleError::network_error(format!("no se pudo abrir {}: {e}", asset_path.display()))
    })?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| {
        LifecycleError::bundle_invalid(format!("el `.zip` descargado no se puede abrir: {e}"))
    })?;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|e| {
            LifecycleError::bundle_invalid(format!("el `.zip` descargado está corrupto: {e}"))
        })?;
        let Some(path) = entry.enclosed_name() else {
            return Err(LifecycleError::bundle_invalid(
                "el `.zip` descargado trae una entrada fuera de su raíz".to_string(),
            ));
        };
        let dest = staging.join(path);
        if entry.is_dir() {
            std::fs::create_dir_all(&dest).map_err(|e| {
                LifecycleError::bundle_invalid(format!(
                    "no se pudo extraer en {}: {e}",
                    dest.display()
                ))
            })?;
        } else {
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    LifecycleError::bundle_invalid(format!(
                        "no se pudo extraer en {}: {e}",
                        dest.display()
                    ))
                })?;
            }
            let mut out = std::fs::File::create(&dest).map_err(|e| {
                LifecycleError::bundle_invalid(format!(
                    "no se pudo extraer en {}: {e}",
                    dest.display()
                ))
            })?;
            std::io::copy(&mut entry, &mut out).map_err(|e| {
                LifecycleError::bundle_invalid(format!(
                    "no se pudo extraer en {}: {e}",
                    dest.display()
                ))
            })?;
        }
    }
    Ok(())
}

/// Extrae un `.tar.gz`, rechazando las entradas absolutas o con `..`.
fn extract_tar_gz(asset_path: &Path, staging: &Path) -> Result<(), LifecycleError> {
    let file = std::fs::File::open(asset_path).map_err(|e| {
        LifecycleError::network_error(format!("no se pudo abrir {}: {e}", asset_path.display()))
    })?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    let entries = archive.entries().map_err(|e| {
        LifecycleError::bundle_invalid(format!("el `.tar.gz` descargado está corrupto: {e}"))
    })?;
    for entry in entries {
        let mut entry = entry.map_err(|e| {
            LifecycleError::bundle_invalid(format!("el `.tar.gz` descargado está corrupto: {e}"))
        })?;
        let path = entry.path().map_err(|e| {
            LifecycleError::bundle_invalid(format!(
                "el `.tar.gz` descargado trae una ruta ilegible: {e}"
            ))
        })?;
        if path.is_absolute() || path.components().any(is_parent) {
            return Err(LifecycleError::bundle_invalid(
                "el `.tar.gz` descargado trae una entrada fuera de su raíz".to_string(),
            ));
        }
        entry.unpack_in(staging).map_err(|e| {
            LifecycleError::bundle_invalid(format!(
                "no se pudo extraer en {}: {e}",
                staging.display()
            ))
        })?;
    }
    Ok(())
}

/// `true` si el componente es `..`.
fn is_parent(component: std::path::Component<'_>) -> bool {
    matches!(component, std::path::Component::ParentDir)
}

/// Permisos 0755/0644 como `install`: ejecutable y `vendor/` ejecutables, el
/// resto lectura. En Windows no hay modo Unix y se omite.
fn apply_permissions(triple: &str, staging: &Path) -> Result<(), LifecycleError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let section = manifest::target_section(triple)?;
        let entries = walk(staging);
        for relative in entries {
            let mode = if relative == section.executable || relative.starts_with("vendor/") {
                0o755
            } else {
                0o644
            };
            let path = staging.join(relative_path(&relative));
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode));
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = (triple, staging);
        Ok(())
    }
}

/// Entradas de `staging` como rutas relativas con `/`, en el orden del
/// manifiesto primero y luego el resto. Solo la necesitan los permisos de Unix.
#[cfg(unix)]
fn walk(staging: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![staging.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
                stack.push(path);
                continue;
            }
            let relative = path
                .strip_prefix(staging)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            out.push(relative);
        }
    }
    out.sort();
    out
}

/// Deja el staging solo para el usuario en Unix (0700). En Windows hereda la
/// ACL del padre, que en las raíces del producto ya es del usuario.
fn restrict_staging(staging: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(staging, std::fs::Permissions::from_mode(0o700));
    }
    #[cfg(not(unix))]
    {
        let _ = staging;
    }
}

/// Nombre del producto para los diagnósticos, sin acoplar el módulo a `avi-store`.
const APP: &str = "ai-voice-interconnector";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{scratch, serve_responses, FakeResponse};
    use avi_core::exit_codes::ExitCode;

    /// Serializa las pruebas que redefinen `AVI_DOWNLOAD_BASE_URL`: el entorno
    /// es global y dos pruebas que lo muten a la vez se contaminarían. Es un
    /// `Mutex` de `tokio` y no uno estándar, que es el que un test asíncrono no
    /// debe sostener a través de un `await`.
    static DOWNLOAD_BASE_GUARD: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// Suma de mentira sobre unos bytes: el hash real de `b"contenido"`.
    fn sums_for(asset_name: &str, asset_bytes: &[u8]) -> String {
        format!("{}  {asset_name}\n", hex_sha256(asset_bytes))
    }

    /// La entrada que coincide se acepta.
    #[test]
    fn update_fetch_verify_accepts_matching_entry() {
        let bytes = b"contenido del archivo";
        let asset = "ai-voice-interconnector-0.24.0-x86_64-linux.tar.gz";
        assert!(verify_sums_bytes(&sums_for(asset, bytes), asset, bytes).is_ok());
    }

    /// El hash se compara en minúsculas: el inventario en mayúsculas vale.
    #[test]
    fn update_fetch_verify_accepts_uppercase_hash() {
        let bytes = b"contenido del archivo";
        let asset = "ai-voice-interconnector-0.24.0-x86_64-linux.tar.gz";
        let sums = format!("{}  {asset}\n", hex_sha256(bytes).to_uppercase());
        assert!(verify_sums_bytes(&sums, asset, bytes).is_ok());
    }

    /// El marcador `*` de modo binario de `sha256sum` se tolera.
    #[test]
    fn update_fetch_verify_accepts_binary_mode_marker() {
        let bytes = b"contenido del archivo";
        let asset = "ai-voice-interconnector-0.24.0-x86_64-linux.tar.gz";
        let sums = format!("{} *{asset}\n", hex_sha256(bytes));
        assert!(verify_sums_bytes(&sums, asset, bytes).is_ok());
    }

    /// Hash distinto: `checksum_mismatch` con el 21 del plan.
    #[test]
    fn update_fetch_verify_rejects_tampered_hash() {
        let asset = "ai-voice-interconnector-0.24.0-x86_64-linux.tar.gz";
        let sums = format!("{}  {asset}\n", "0".repeat(64));
        let err = verify_sums_bytes(&sums, asset, b"contenido manipulado").unwrap_err();
        assert_eq!(err.reason, "checksum_mismatch");
        assert_eq!(ExitCode::from_reason(err.reason).code(), 21);
    }

    /// Entrada ausente: `checksum_mismatch`, y el staging se borra sin tocar lo demás.
    #[test]
    fn update_fetch_verify_rejects_missing_entry() {
        let asset = "ai-voice-interconnector-0.24.0-x86_64-linux.tar.gz";
        let sums = format!("{}  otro-archivo.tar.gz\n", "0".repeat(64));
        let err = verify_sums_bytes(&sums, asset, b"contenido").unwrap_err();
        assert_eq!(err.reason, "checksum_mismatch");
        assert_eq!(ExitCode::from_reason(err.reason).code(), 21);
    }

    /// El nombre se exige exacto: un sufijo de más no cuela.
    #[test]
    fn update_fetch_verify_rejects_inexact_name() {
        let bytes = b"contenido del archivo";
        let asset = "ai-voice-interconnector-0.24.0-x86_64-linux.tar.gz";
        let sums = format!("{}  {asset}.sig\n", hex_sha256(bytes));
        let err = verify_sums_bytes(&sums, asset, bytes).unwrap_err();
        assert_eq!(err.reason, "checksum_mismatch");
    }

    /// El staging es hermano del programa, con el prefijo propio de la tabla de rutas.
    #[test]
    fn update_fetch_staging_is_sibling() {
        let program = Path::new("/home/ana/.local/opt/ai-voice-interconnector");
        assert_eq!(
            staging_dir_for(program, "0.24.0"),
            Path::new("/home/ana/.local/opt/.ai-voice-interconnector-staging-update-0.24.0")
        );
    }

    /// La espera entre reintentos crece con el intento.
    #[test]
    fn update_fetch_retry_delay_grows() {
        assert_eq!(retry_delay(1), Duration::from_millis(250));
        assert_eq!(retry_delay(2), Duration::from_millis(500));
        assert_eq!(retry_delay(3), Duration::from_millis(750));
    }

    /// Construye un `.tar.gz` en memoria con `files` (`ruta con /` → contenido).
    fn pack_tar_gz(files: &[(&str, &str)]) -> Vec<u8> {
        let mut tar = tar::Builder::new(Vec::new());
        for (name, content) in files {
            let mut header = tar::Header::new_gnu();
            header.set_mode(0o644);
            header.set_size(content.len() as u64);
            tar.append_data(&mut header, name, content.as_bytes())
                .expect("se empaqueta la fixture");
        }
        let raw = tar.into_inner().expect("se cierra el tar");
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        use std::io::Write;
        encoder.write_all(&raw).expect("se comprime la fixture");
        encoder.finish().expect("se cierra el gzip")
    }

    /// Construye un `.zip` en memoria con `files` (`ruta con /` → contenido).
    fn pack_zip(files: &[(&str, &str)]) -> Vec<u8> {
        use std::io::Write;
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, content) in files {
            writer
                .start_file(*name, options)
                .expect("se empaqueta la fixture");
            writer
                .write_all(content.as_bytes())
                .expect("se escribe la fixture");
        }
        writer.finish().expect("se cierra el zip").into_inner()
    }

    /// El `.zip` (formato de Windows) se extrae íntegro.
    #[test]
    fn update_fetch_zip_extracts() {
        let sandbox = scratch("fetch-zip");
        let asset = sandbox.join("bundle.zip");
        let files = [("a.txt", "alfa"), ("dir/b.txt", "beta")];
        std::fs::write(&asset, pack_zip(&files)).expect("se escribe la fixture");
        let dest = sandbox.join("out");
        std::fs::create_dir_all(&dest).expect("se crea el destino");
        extract_bundle(&asset, &dest).expect("el zip se extrae");
        assert_eq!(std::fs::read_to_string(dest.join("a.txt")).unwrap(), "alfa");
        assert_eq!(
            std::fs::read_to_string(dest.join("dir").join("b.txt")).unwrap(),
            "beta"
        );
        std::fs::remove_dir_all(&sandbox).ok();
    }

    /// El `.zip` con una entrada que escapa (`../`) se rechaza sin extraerla.
    #[test]
    fn update_fetch_zip_rejects_escape() {
        let sandbox = scratch("fetch-zip-slip");
        let asset = sandbox.join("evil.zip");
        std::fs::write(&asset, pack_zip(&[("../evil.txt", "x")])).expect("se escribe la fixture");
        let dest = sandbox.join("out");
        std::fs::create_dir_all(&dest).expect("se crea el destino");
        let err = extract_bundle(&asset, &dest).unwrap_err();
        assert_eq!(err.reason, "bundle_invalid");
        assert!(!sandbox.join("evil.txt").exists());
        std::fs::remove_dir_all(&sandbox).ok();
    }

    /// Nota sobre `..` en `.tar.gz`: `extract_tar_gz` lo rechaza con doble
    /// vallado (comprobación explícita más la propia de `unpack_in`); el
    /// `tar::Builder` de la fixture ni siquiera permite empaquetar esa entrada,
    /// así que no hay fixture local que lo ejercite.
    /// Contenido completo del bundle del host según el manifiesto, con el
    /// ejecutable como guion que informa la versión.
    fn bundle_files(version: &str) -> Vec<(String, String)> {
        let triple = target::host_triple();
        let section = manifest::target_section(triple).expect("el host tiene sección");
        section
            .required
            .iter()
            .map(|name| {
                let content = if *name == section.executable {
                    format!("#!/bin/sh\necho \"ai-voice-interconnector {version}\"\n")
                } else {
                    format!("contenido de {name}\n")
                };
                (name.clone(), content)
            })
            .collect()
    }

    /// Descarga, verificación, extracción y validación contra el servidor local:
    /// el staging queda con el bundle completo. La fixture viaja en el formato
    /// del host (`.zip` en Windows, `.tar.gz` fuera).
    #[tokio::test]
    async fn update_fetch_download_verifies_and_extracts() {
        let _guard = DOWNLOAD_BASE_GUARD.lock().await;
        let version = "0.24.0";
        let triple = target::host_triple();
        let asset = target::release_asset_name(triple, version).unwrap();
        let owned = bundle_files(version);
        let borrowed: Vec<(&str, &str)> = owned
            .iter()
            .map(|(name, content)| (name.as_str(), content.as_str()))
            .collect();
        let archive: Vec<u8> = if triple.contains("windows") {
            pack_zip(&borrowed)
        } else {
            pack_tar_gz(&borrowed)
        };
        let sums = sums_for(&asset, &archive);
        let client = http_client().unwrap();
        let sandbox = scratch("fetch-ok");
        let program_dir = sandbox.join("opt").join("ai-voice-interconnector");
        std::fs::create_dir_all(&program_dir).expect("se crea el programa previo");
        let (base, seen, handle) = serve_responses(vec![
            FakeResponse::new(200, archive),
            FakeResponse::new(200, sums),
        ])
        .await;
        std::env::set_var(crate::update_resolve::DOWNLOAD_BASE_ENV, &base);
        let outcome = fetch(
            &client,
            &FetchRequest {
                program_dir: program_dir.clone(),
                target: triple.to_string(),
                version: version.to_string(),
            },
        )
        .await;
        std::env::remove_var(crate::update_resolve::DOWNLOAD_BASE_ENV);
        let staged = outcome.expect("la descarga local se verifica y extrae");
        assert_eq!(staged.asset_name, asset);
        assert_eq!(staged.staging, staging_dir_for(&program_dir, version));
        let section = manifest::target_section(triple).unwrap();
        assert!(staged.staging.join(section.executable_path()).is_file());
        assert!(manifest::validate_bundle(triple, &staged.staging).is_ok());
        handle.await.expect("el servidor local termina");
        assert_eq!(
            seen.lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone(),
            vec![
                format!("/releases/download/v{version}/{asset}"),
                format!("/releases/download/v{version}/{SUMS_FILE_NAME}"),
            ]
        );
        // El arranque de comprobación necesita un ejecutable real: el guion de
        // la fixture solo ejecuta en Unix. En Windows lo cubre
        // `update_fetch_boot_mismatch_is_incompatible`.
        if cfg!(unix) {
            verify_boot(&staged.staging, triple, version)
                .await
                .expect("el guion informa la versión");
        }
        std::fs::remove_dir_all(&sandbox).ok();
        std::fs::remove_dir_all(&staged.staging).ok();
    }

    /// Archivo manipulado en tránsito: `checksum_mismatch`, staging borrado e
    /// instalación intacta.
    #[tokio::test]
    async fn update_fetch_checksum_failure_cleans_staging() {
        let _guard = DOWNLOAD_BASE_GUARD.lock().await;
        let version = "0.24.0";
        let triple = target::host_triple();
        let asset = target::release_asset_name(triple, version).unwrap();
        let owned = bundle_files(version);
        let borrowed: Vec<(&str, &str)> = owned
            .iter()
            .map(|(name, content)| (name.as_str(), content.as_str()))
            .collect();
        let archive: Vec<u8> = if triple.contains("windows") {
            pack_zip(&borrowed)
        } else {
            pack_tar_gz(&borrowed)
        };
        let sums = sums_for(&asset, b"bytes distintos a los servidos");
        let client = http_client().unwrap();
        let sandbox = scratch("fetch-tampered");
        let program_dir = sandbox.join("opt").join("ai-voice-interconnector");
        std::fs::create_dir_all(&program_dir).expect("se crea el programa previo");
        let marker = program_dir.join("conservado.txt");
        std::fs::write(&marker, "instalación previa").expect("se planta el marcador");
        let (base, _, handle) = serve_responses(vec![
            FakeResponse::new(200, archive),
            FakeResponse::new(200, sums),
        ])
        .await;
        std::env::set_var(crate::update_resolve::DOWNLOAD_BASE_ENV, &base);
        let err = fetch(
            &client,
            &FetchRequest {
                program_dir: program_dir.clone(),
                target: triple.to_string(),
                version: version.to_string(),
            },
        )
        .await
        .unwrap_err();
        std::env::remove_var(crate::update_resolve::DOWNLOAD_BASE_ENV);
        assert_eq!(err.reason, "checksum_mismatch");
        assert_eq!(ExitCode::from_reason(err.reason).code(), 21);
        assert!(
            !staging_dir_for(&program_dir, version).exists(),
            "el staging se borra"
        );
        assert_eq!(
            std::fs::read_to_string(&marker).expect("el marcador sigue"),
            "instalación previa"
        );
        handle.await.expect("el servidor local termina");
        std::fs::remove_dir_all(&sandbox).ok();
    }

    /// Un binario que no informa la versión es `binary_incompatible` con el 19.
    #[tokio::test]
    async fn update_fetch_boot_mismatch_is_incompatible() {
        let triple = target::host_triple();
        let sandbox = scratch("fetch-boot");
        let section = manifest::target_section(triple).unwrap();
        let exe = sandbox.join(section.executable_path());
        if let Some(parent) = exe.parent() {
            std::fs::create_dir_all(parent).expect("se crea el padre");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::write(&exe, "#!/bin/sh\necho \"ai-voice-interconnector 0.0.0\"\n")
                .expect("se planta el guion");
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755))
                .expect("se hace ejecutable");
            let err = verify_boot(&sandbox, triple, "0.24.0").await.unwrap_err();
            assert_eq!(err.reason, "binary_incompatible");
            assert_eq!(ExitCode::from_reason(err.reason).code(), 19);
        }
        #[cfg(windows)]
        {
            std::fs::write(&exe, "esto no es un ejecutable").expect("se planta el señuelo");
            let err = verify_boot(&sandbox, triple, "0.24.0").await.unwrap_err();
            assert_eq!(err.reason, "binary_incompatible");
            assert_eq!(ExitCode::from_reason(err.reason).code(), 19);
        }
        std::fs::remove_dir_all(&sandbox).ok();
    }
}
