// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! El cliente de descargas nunca envía credenciales del entorno, contra un
//! servidor HTTP local (sin red externa). El archivo contiene una sola prueba
//! porque modifica variables de entorno del proceso y así no afecta a otras
//! pruebas que se ejecuten en paralelo.

use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};

const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";
const ETAG: &str = "d41d8cd98f00b204e9800998ecf8427e";
const BODY: &[u8] = b"{\"modelo\": \"de prueba\"}";

/// Atiende conexiones indefinidamente y guarda la cabecera de cada petición:
/// responde a `HEAD` solo con cabeceras y a `GET` con el cuerpo, ambos con los
/// metadatos que `hf-hub` exige para una descarga no-xet (`ETag`,
/// `x-repo-commit` y `Content-Length`).
async fn serve_model_file(requests: Arc<Mutex<Vec<String>>>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("se puede enlazar el servidor local de la prueba");
    let addr = listener
        .local_addr()
        .expect("el servidor local tiene dirección");
    tokio::spawn(async move {
        loop {
            let Ok((mut conn, _)) = listener.accept().await else {
                return;
            };
            let requests = Arc::clone(&requests);
            tokio::spawn(async move {
                let mut request = Vec::new();
                let mut chunk = [0u8; 1024];
                while let Ok(read) = conn.read(&mut chunk).await {
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&chunk[..read]);
                    if request.len() > 8192 || request.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                requests
                    .lock()
                    .expect("el registro de peticiones no está envenenado")
                    .push(String::from_utf8_lossy(&request).to_ascii_lowercase());
                let is_head = request.starts_with(b"HEAD ");
                let head = format!(
                    "HTTP/1.1 200 OK\r\nETag: \"{ETAG}\"\r\nx-repo-commit: {COMMIT}\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n",
                    BODY.len()
                );
                let _ = conn.write_all(head.as_bytes()).await;
                if !is_head {
                    let _ = conn.write_all(BODY).await;
                }
            });
        }
    });
    format!("http://{addr}")
}

/// Con un token en `HF_TOKEN`, en `HF_TOKEN_PATH` y en el archivo `token` de
/// `HF_HOME`, ninguna petición del cliente de la aplicación lleva la cabecera
/// `Authorization`: los modelos fijados son públicos y un token caducado o
/// ajeno rompería o falsearía la descarga.
#[tokio::test(flavor = "current_thread")]
async fn download_never_sends_environment_token() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let endpoint = serve_model_file(Arc::clone(&requests)).await;

    // Directorios con dueño: se borran al salir, también si una aserción falla.
    let scratch = tempfile::Builder::new()
        .prefix("avi_store_hub_anonymous_")
        .tempdir()
        .expect("directorio temporal de la prueba");
    let hf_home = scratch.path().join("hf_home");
    std::fs::create_dir_all(&hf_home).expect("se crea HF_HOME");
    std::fs::write(hf_home.join("token"), "hf_token_del_archivo").expect("se escribe el token");
    let token_path = scratch.path().join("token_path");
    std::fs::write(&token_path, "hf_token_de_la_ruta").expect("se escribe HF_TOKEN_PATH");
    std::env::set_var("HF_TOKEN", "hf_token_del_entorno");
    std::env::set_var("HF_TOKEN_PATH", &token_path);
    std::env::set_var("HF_HOME", &hf_home);
    let cache = scratch.path().join("cache");

    let client = avi_store::hub_client_builder(
        avi_store::download_http_client().expect("el cliente HTTP de descargas se construye"),
    )
    .endpoint(endpoint)
    .cache_dir(&cache)
    .build()
    .expect("el cliente se construye con endpoint y caché explícitos");
    client
        .model("owner", "model")
        .download_file()
        .filename("config.json")
        .revision("main")
        .send()
        .await
        .expect("la descarga contra el servidor local termina");

    let requests = requests
        .lock()
        .expect("el registro de peticiones no está envenenado");
    assert!(!requests.is_empty(), "el servidor recibió peticiones");
    for request in requests.iter() {
        assert!(
            !request.contains("authorization:"),
            "ninguna petición debe llevar credenciales: {request}"
        );
    }
}
