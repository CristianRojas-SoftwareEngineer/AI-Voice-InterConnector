// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! El silencio de lectura está acotado: un servidor que entrega las cabeceras,
//! suelta unos bytes y **no cierra el flujo** no puede dejar la descarga
//! esperando para siempre.
//!
//! Es el fallo observado en producción: la conexión queda medio abierta, `hf-hub`
//! consume el cuerpo con `stream.next().await` sin timeout propio, así que no
//! llegan datos ni error y el proceso espera de forma indefinida sin log ni
//! código de salida.

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";
const ETAG: &str = "d41d8cd98f00b204e9800998ecf8427e";
const TOTAL: usize = 4 * 1024 * 1024;

/// Servidor local que responde a `HEAD` con los metadatos que `hf-hub` exige
/// —`ETag`, `x-repo-commit` y `Content-Length`— y a `GET` con una cabecera de
/// longitud mayor que lo que entrega: escribe 64 bytes, no cierra la conexión y
/// se queda mudo.
async fn serve_stalled_body() -> String {
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
                let is_head = request.starts_with(b"HEAD ");
                let head = format!(
                    "HTTP/1.1 200 OK\r\nETag: \"{ETAG}\"\r\nx-repo-commit: {COMMIT}\r\n\
                     Content-Length: {TOTAL}\r\nConnection: close\r\n\r\n"
                );
                let _ = conn.write_all(head.as_bytes()).await;
                if !is_head {
                    let _ = conn.write_all(&[b'x'; 64]).await;
                    // Silencio deliberado: ni el resto del cuerpo ni el cierre.
                    // La tarea no termina, así que la conexión sigue abierta.
                    std::future::pending::<()>().await;
                }
            });
        }
    });
    format!("http://{addr}")
}

/// La descarga de un cuerpo que no termina falla, y falla dentro de un plazo
/// acotado: el cliente de la aplicación acota el silencio de lectura.
#[tokio::test(flavor = "current_thread")]
async fn stalled_body_fails_instead_of_hanging() {
    let endpoint = serve_stalled_body().await;
    let cache_guard = tempfile::Builder::new()
        .prefix("avi_store_download_stall_")
        .tempdir()
        .expect("directorio temporal de la caché");

    // El cliente es el de producción, con el plazo de silencio acortado para que
    // la prueba no espere el minuto real.
    let http = avi_store::download_http_client_with(Duration::from_secs(2))
        .expect("el cliente de descargas se construye");
    let client = hf_hub::HFClient::builder()
        .endpoint(endpoint)
        .cache_dir(cache_guard.path())
        .client(http)
        .build()
        .expect("el cliente se construye con endpoint y caché explícitos");

    let repo = client.model("owner", "model");
    let request = repo
        .download_file()
        .filename("model.safetensors")
        .revision("main");
    let outcome = tokio::time::timeout(Duration::from_secs(90), request.send()).await;

    let error = outcome
        .expect("el silencio de lectura acota la espera: la descarga no puede quedarse colgada")
        .expect_err("un cuerpo que no termina ni cierra debe fallar");
    let message = format!("{error}");
    assert!(
        message.to_lowercase().contains("timed out")
            || message.to_lowercase().contains("timeout")
            || message.to_lowercase().contains("error"),
        "el error debe nombrar el fallo de la descarga, no otra cosa: {message}"
    );
}
