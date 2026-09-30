//! Prueba de integración del layout de caché de `hf-hub` a través de su API
//! pública, contra un servidor HTTP local (sin red externa).

use tokio::io::{AsyncReadExt, AsyncWriteExt};

const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";
const ETAG: &str = "d41d8cd98f00b204e9800998ecf8427e";
const BODY: &[u8] = b"{\"modelo\": \"de prueba\"}";

/// Atiende conexiones indefinidamente: responde a `HEAD` solo con cabeceras y a
/// `GET` con el cuerpo, ambos con los metadatos que `hf-hub` exige para una
/// descarga no-xet (`ETag`, `x-repo-commit` y `Content-Length`).
async fn serve_model_file() -> String {
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

/// El puntero de `snapshots/` y el blob de `blobs/` deben ser el mismo archivo:
/// en Unix lo prueba el symlink y en Windows el enlace duro que aplica el parche
/// vendorizado de `hf-hub`. Si fueran copias, cada modelo ocuparía el doble de
/// disco.
#[tokio::test(flavor = "current_thread")]
async fn pointer_and_blob_are_the_same_file() {
    let endpoint = serve_model_file().await;
    let cache = std::env::temp_dir().join(format!("avi_store_hf_layout_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&cache);

    let client = hf_hub::HFClient::builder()
        .endpoint(endpoint)
        .cache_dir(&cache)
        .build()
        .expect("el cliente se construye con endpoint y caché explícitos");
    let pointer = client
        .model("propietario", "modelo")
        .download_file()
        .filename("config.json")
        .revision("main")
        .send()
        .await
        .expect("la descarga contra el servidor local termina");

    let repo_dir = cache.join("models--propietario--modelo");
    let blob = repo_dir.join("blobs").join(ETAG);
    assert_eq!(
        pointer,
        repo_dir.join("snapshots").join(COMMIT).join("config.json")
    );
    assert_eq!(std::fs::read(&blob).expect("el blob existe"), BODY);
    assert!(
        same_file::is_same_file(&pointer, &blob).expect("puntero y blob son comparables"),
        "el puntero debe ser el mismo archivo que el blob, no una copia"
    );

    let _ = std::fs::remove_dir_all(&cache);
}
