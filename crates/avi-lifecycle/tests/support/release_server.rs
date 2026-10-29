// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! Servidor falso de releases. Lo declaran las pruebas de `self update` con
//! `#[path = "support/release_server.rs"] mod release_server;`.
//!
//! El `self update` resuelve y descarga contra `AVI_DOWNLOAD_BASE_URL` cuando está
//! definida (`update_resolve::download_base_url`), y si no contra el GitHub real. Este
//! servidor sustituye al GitHub real en las pruebas: sirve la página `releases/latest`
//! con su redirección al tag, la API de respaldo, el archivo de cada target (nombre de
//! `target::release_asset_name`) y su `SHA256SUMS.txt`, con contador de descargas de
//! archivo y variante corrupta. Corre en hilos propios con E/S bloqueante, así que
//! ninguna prueba necesita un runtime para levantarlo ni para pararlo: `Drop` lo apaga.
//!
//! Cada conexión se atiende en su propio hilo y el enrutado es por ruta, no por orden:
//! `reqwest` sigue la redirección con una conexión nueva y `fetch` pide archivo y sumas
//! en secuencia, así que un mock ordenado es frágil aquí y el de `test_support` sigue
//! siendo el que cubre la resolución fina a nivel de cliente.

use std::collections::HashMap;
use std::io::Write;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};

/// Estado compartido del servidor falso: lo que sirve y lo que ha visto.
struct ReleaseState {
    /// Versión que el servidor publica como última estable.
    version: String,
    /// Sirve el archivo manipulado (un byte alterado) manteniendo las sumas buenas.
    corrupt_asset: bool,
    /// Archivo por nombre de asset, ya empaquetado. Se genera bajo demanda.
    bundles: HashMap<String, Vec<u8>>,
    /// Descargas del archivo servidas, que cuando la instalada ya es la última deben
    /// quedar en cero.
    asset_downloads: usize,
    /// Descargas de `SHA256SUMS.txt` servidas.
    sums_downloads: usize,
}

/// Servidor HTTP local que suplanta los releases de GitHub para `self update`.
pub struct FakeReleaseServer {
    base_url: String,
    state: std::sync::Arc<Mutex<ReleaseState>>,
    stop: std::sync::Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl FakeReleaseServer {
    /// Levanta el servidor publicando `version` como última estable y devuelve su
    /// manipulador. Escucha en `127.0.0.1` con puerto efímero.
    pub fn serve(version: &str) -> Self {
        let listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("se enlaza el servidor falso");
        // El listener no bloquea solo para que el hilo vea la orden de parada entre
        // aceptaciones; cada conexión aceptada vuelve a modo bloqueante al atenderse,
        // porque en Windows heredaría el modo no bloqueante y su lectura fallaría con
        // `WouldBlock` si llegara antes que la petición.
        listener
            .set_nonblocking(true)
            .expect("el servidor falso no bloquea al aceptar");
        let addr = listener.local_addr().expect("dirección del servidor falso");
        let state = std::sync::Arc::new(Mutex::new(ReleaseState {
            version: version.to_string(),
            corrupt_asset: false,
            bundles: HashMap::new(),
            asset_downloads: 0,
            sums_downloads: 0,
        }));
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let thread = {
            let state = std::sync::Arc::clone(&state);
            let stop = std::sync::Arc::clone(&stop);
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let state = std::sync::Arc::clone(&state);
                            std::thread::spawn(move || handle_release_connection(stream, &state));
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(std::time::Duration::from_millis(5));
                        }
                        Err(e) => panic!("el servidor falso no acepta conexiones: {e}"),
                    }
                }
            })
        };
        Self {
            base_url: format!("http://{addr}"),
            state,
            stop,
            thread: Some(thread),
        }
    }

    /// URL base del servidor para asignarla solo al proceso bajo prueba.
    #[allow(dead_code)]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Declara la URL en el entorno para pruebas lifecycle serializadas.
    #[allow(dead_code)]
    pub fn seed_download_base(&self) {
        std::env::set_var(
            avi_lifecycle::update_resolve::DOWNLOAD_BASE_ENV,
            &self.base_url,
        );
    }

    /// Retira la URL del entorno tras la prueba lifecycle.
    #[allow(dead_code)]
    pub fn clear_download_base() {
        std::env::remove_var(avi_lifecycle::update_resolve::DOWNLOAD_BASE_ENV);
    }

    /// A partir de ahora sirve el archivo manipulado con las sumas buenas: la
    /// verificación debe fallar con `checksum_mismatch`.
    #[allow(dead_code)]
    pub fn serve_corrupt_asset(&self) {
        locked_state(&self.state).corrupt_asset = true;
    }

    /// Descargas del archivo servidas hasta ahora.
    #[allow(dead_code)]
    pub fn asset_downloads(&self) -> usize {
        locked_state(&self.state).asset_downloads
    }

    /// Descargas de `SHA256SUMS.txt` servidas hasta ahora.
    #[allow(dead_code)]
    pub fn sums_downloads(&self) -> usize {
        locked_state(&self.state).sums_downloads
    }

    /// Resumen de descargas para la comprobación de la CLI.
    #[allow(dead_code)]
    pub fn download_counts(&self) -> (usize, usize) {
        let state = locked_state(&self.state);
        (state.asset_downloads, state.sums_downloads)
    }
}

impl Drop for FakeReleaseServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            thread
                .join()
                .expect("el hilo del servidor falso no entra en pánico");
        }
    }
}

/// El candado del estado sin envenenarse si otra prueba lo dejó tomado al caer.
fn locked_state(
    state: &std::sync::Arc<Mutex<ReleaseState>>,
) -> std::sync::MutexGuard<'_, ReleaseState> {
    state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Atiende una conexión del servidor falso: lee hasta el fin de las cabeceras, anota
/// la ruta y responde con `Content-Length` y cierre de conexión.
fn handle_release_connection(
    mut stream: std::net::TcpStream,
    state: &std::sync::Arc<Mutex<ReleaseState>>,
) {
    use std::io::Read;
    stream
        .set_nonblocking(false)
        .expect("la conexión del servidor falso bloquea al leer");
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .expect("se fija el plazo de lectura de la conexión");
    let mut request = Vec::new();
    let mut chunk = [0u8; 1024];
    while request.len() <= 8192 {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => {
                request.extend_from_slice(&chunk[..read]);
                if request.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            Err(e) => panic!("el servidor falso no lee la petición: {e}"),
        }
    }
    let path = String::from_utf8_lossy(&request)
        .lines()
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .nth(1)
        .unwrap_or_default()
        .to_string();
    let (status, reason, headers, body) = release_response(state, &path);
    let mut head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    for (name, value) in &headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    // El cliente puede cortar la conexión antes de leer la respuesta entera (p. ej. al
    // rechazar un archivo por sus sumas): ese fallo de escritura no es un fallo del
    // servidor falso.
    if stream.write_all(head.as_bytes()).is_ok() {
        let _ = stream.write_all(&body);
    }
    let _ = stream.shutdown(std::net::Shutdown::Both);
}

/// Respuesta del servidor falso para `path`, con el estado ya actualizado
/// (contadores).
fn release_response(
    state: &std::sync::Arc<Mutex<ReleaseState>>,
    path: &str,
) -> (u16, &'static str, Vec<(String, String)>, Vec<u8>) {
    let mut locked = locked_state(state);
    let version = locked.version.clone();
    if path == "/releases/latest" {
        return (
            302,
            "Found",
            vec![("Location".to_string(), format!("/releases/tag/v{version}"))],
            Vec::new(),
        );
    }
    if path == format!("/releases/tag/v{version}") {
        return (
            200,
            "OK",
            Vec::new(),
            format!("release v{version}").into_bytes(),
        );
    }
    if path == "/api/releases/latest" {
        let body = format!(r#"{{"tag_name": "v{version}"}}"#).into_bytes();
        return (200, "OK", vec![json_header()], body);
    }
    if path == format!("/releases/download/v{version}/SHA256SUMS.txt") {
        locked.sums_downloads += 1;
        let mut lines = String::new();
        for triple in avi_lifecycle::target::SUPPORTED_TARGETS {
            let asset = avi_lifecycle::target::release_asset_name(triple, &version)
                .expect("el target soportado tiene archivo");
            let bytes = release_bundle(&mut locked, triple, &version);
            lines.push_str(&format!("{}  {asset}\n", sha256_hex(&bytes)));
        }
        return (200, "OK", Vec::new(), lines.into_bytes());
    }
    if let Some(asset) = path.strip_prefix(&format!("/releases/download/v{version}/")) {
        let triple = avi_lifecycle::target::SUPPORTED_TARGETS
            .iter()
            .find(|triple| {
                avi_lifecycle::target::release_asset_name(triple, &version)
                    .map(|name| name == *asset)
                    .unwrap_or(false)
            });
        if let Some(triple) = triple {
            locked.asset_downloads += 1;
            let mut bytes = release_bundle(&mut locked, triple, &version);
            if locked.corrupt_asset && bytes.len() > 16 {
                bytes[10] ^= 0xFF;
            }
            return (200, "OK", Vec::new(), bytes);
        }
    }
    (404, "Not Found", Vec::new(), b"sin release".to_vec())
}

/// Cabecera JSON de la API de respaldo.
fn json_header() -> (String, String) {
    ("Content-Type".to_string(), "application/json".to_string())
}

/// Archivo del release para `triple`, generado bajo demanda y memorizado: el bundle
/// completo del manifiesto, con el ejecutable como guion que informa la versión.
///
/// El guion solo ejecuta en Unix; en Windows la comprobación de arranque la cubre el
/// señuelo de `binary_incompatible`, como en las pruebas de `update_fetch`.
fn release_bundle(state: &mut ReleaseState, triple: &str, version: &str) -> Vec<u8> {
    let asset = avi_lifecycle::target::release_asset_name(triple, version)
        .expect("el target soportado tiene archivo");
    if let Some(bytes) = state.bundles.get(&asset) {
        return bytes.clone();
    }
    let section = avi_lifecycle::manifest::target_section(triple).expect("el target tiene sección");
    let mut files = Vec::new();
    for name in &section.required {
        let content = if *name == section.executable {
            format!("#!/bin/sh\necho \"ai-voice-interconnector {version}\"\n")
        } else {
            format!("contenido de {name}\n")
        };
        files.push((name.clone(), content));
    }
    let bytes = if triple.contains("windows") {
        pack_release_zip(&files)
    } else {
        pack_release_tar_gz(&files)
    };
    state.bundles.insert(asset, bytes.clone());
    bytes
}

/// Empaqueta `files` (`ruta con /` → contenido) como `.tar.gz` de release.
fn pack_release_tar_gz(files: &[(String, String)]) -> Vec<u8> {
    let mut tar = tar::Builder::new(Vec::new());
    for (name, content) in files {
        let mut header = tar::Header::new_gnu();
        header.set_mode(0o644);
        header.set_size(content.len() as u64);
        tar.append_data(&mut header, name, content.as_bytes())
            .expect("se empaquetan los ficheros del release");
    }
    let raw = tar.into_inner().expect("se cierra el tar");
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(&raw).expect("se comprime el release");
    encoder.finish().expect("se cierra el gzip")
}

/// Empaqueta `files` (`ruta con /` → contenido) como `.zip` de release.
fn pack_release_zip(files: &[(String, String)]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, content) in files {
        writer
            .start_file(name, options)
            .expect("se empaquetan los ficheros del release");
        writer
            .write_all(content.as_bytes())
            .expect("se escriben los ficheros del release");
    }
    writer.finish().expect("se cierra el zip").into_inner()
}

/// SHA-256 en hexadecimal minúsculo, el formato que `SHA256SUMS.txt` exige.
fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    use std::fmt::Write;
    let digest = sha2::Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        write!(out, "{byte:02x}").expect("escribir en un String no falla");
    }
    out
}
