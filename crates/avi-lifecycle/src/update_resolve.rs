//! Resolución de la versión objetivo de `self update`, los pasos 3 a 5 de la
//! actualización.
//!
//! Fija qué versión es la objetivo y cómo se compara con la instalada, antes de
//! descargar nada. La comparación reutiliza `compare_versions` de `install` (sin
//! crate `semver`), y la última estable se obtiene siguiendo la redirección de
//! `releases/latest`, con la API REST solo como respaldo. El cliente HTTP es el
//! mismo que usa la descarga (`update_fetch::http_client`).
//!
//! Nada de este módulo escribe en disco: `--check` devuelve estos mismos datos y
//! es U4 quien decide no crear el staging.

use crate::install::compare_versions;
use crate::{update_fetch, LifecycleError};

/// Repositorio del producto, el mismo que publica los releases.
pub const GITHUB_REPO: &str = "CristianRojas-SoftwareEngineer/AI-Voice-InterConnector";

/// Variable que sustituye la base de descarga (espejos, redes aisladas, pruebas).
/// Es contrato de máquina y no se traduce.
pub const DOWNLOAD_BASE_ENV: &str = "AVI_DOWNLOAD_BASE_URL";

/// Entrada de la resolución: versión instalada del recibo, `--version` opcional y
/// `--force`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolveRequest {
    pub installed: String,
    pub explicit: Option<String>,
    pub force: bool,
}

/// Veredicto de la comparación entre la instalada y la objetivo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Iguales sin `--force`: `already_up_to_date`, sin descargar (lo emite U4).
    UpToDate,
    /// Objetivo mayor, o igual con `--force` (reinstalación).
    Upgrade,
    /// Objetivo menor: solo con `--version` explícito y marca destructiva.
    Downgrade,
}

/// Salida de la resolución: versión objetivo, veredicto y marca destructiva.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    pub target: String,
    pub verdict: Verdict,
    pub destructive: bool,
}

/// Resuelve la versión objetivo contra la instalada.
///
/// `latest` es la última estable (de `latest_stable`, salvo `--version`
/// explícito, que manda). Una `v` inicial se tolera en ambos. Un objetivo menor
/// sin `--version` explícito es un error de uso: degradar exige pedirlo.
pub fn resolve(request: &ResolveRequest, latest: &str) -> Result<Resolution, LifecycleError> {
    let from_explicit = request.explicit.is_some();
    let raw = request.explicit.as_deref().unwrap_or(latest);
    let target = normalize_version(raw).ok_or_else(|| {
        LifecycleError::usage_error(format!(
            "versión objetivo inválida '{raw}': debe ser X.Y.Z, con `v` inicial opcional"
        ))
    })?;
    let installed = normalize_version(&request.installed).ok_or_else(|| {
        LifecycleError::usage_error(format!(
            "la versión instalada '{}' no es X.Y.Z: el recibo está corrupto",
            request.installed
        ))
    })?;
    match compare_versions(&target, &installed) {
        std::cmp::Ordering::Equal => {
            if request.force {
                Ok(Resolution {
                    target,
                    verdict: Verdict::Upgrade,
                    destructive: false,
                })
            } else {
                Ok(Resolution {
                    target,
                    verdict: Verdict::UpToDate,
                    destructive: false,
                })
            }
        }
        std::cmp::Ordering::Greater => Ok(Resolution {
            target,
            verdict: Verdict::Upgrade,
            destructive: false,
        }),
        std::cmp::Ordering::Less => {
            if from_explicit {
                Ok(Resolution {
                    target,
                    verdict: Verdict::Downgrade,
                    destructive: true,
                })
            } else {
                Err(LifecycleError::usage_error(format!(
                    "la última estable ({target}) es anterior a la instalada ({installed}): \
                     degradar exige `--version {target}` explícito"
                )))
            }
        }
    }
}

/// Base de descarga de releases: `AVI_DOWNLOAD_BASE_URL` si está definida, y si
/// no el GitHub del producto.
pub fn download_base_url() -> String {
    let trimmed = std::env::var(DOWNLOAD_BASE_ENV)
        .ok()
        .map(|base| base.trim().trim_end_matches('/').to_string())
        .filter(|base| !base.is_empty());
    trimmed.unwrap_or_else(|| format!("https://github.com/{GITHUB_REPO}"))
}

/// URL de la página `releases/latest`, cuya redirección final nombra el tag.
pub fn latest_page_url() -> String {
    format!("{}/releases/latest", download_base_url())
}

/// URL de respaldo de la API REST de releases.
pub fn releases_api_url() -> String {
    let base = download_base_url();
    if base.contains("github.com") {
        format!("https://api.github.com/repos/{GITHUB_REPO}/releases/latest")
    } else {
        format!("{base}/api/releases/latest")
    }
}

/// Última estable: tag de la URL final tras seguir la redirección de
/// `releases/latest`, o `tag_name` de la API REST como respaldo.
pub async fn latest_stable(client: &reqwest::Client) -> Result<String, LifecycleError> {
    latest_stable_from(client, &latest_page_url(), &releases_api_url()).await
}

/// Núcleo comprobable de [`latest_stable`], con las dos URLs como dato para que
/// las pruebas apunten al servidor local.
pub async fn latest_stable_from(
    client: &reqwest::Client,
    page_url: &str,
    api_url: &str,
) -> Result<String, LifecycleError> {
    if let Ok(page) = update_fetch::get_with_retry(client, page_url).await {
        if let Some(tag) = parse_tag_version(page.url().as_str()) {
            return Ok(tag);
        }
    }
    let reply = update_fetch::get_with_retry(client, api_url).await?;
    let body = reply.bytes().await.map_err(|e| {
        LifecycleError::network_error(format!("la respuesta de {api_url} no se pudo leer: {e}"))
    })?;
    let json: serde_json::Value = serde_json::from_slice(&body).map_err(|e| {
        LifecycleError::network_error(format!("la respuesta de {api_url} no es JSON: {e}"))
    })?;
    json.get("tag_name")
        .and_then(|tag| tag.as_str())
        .and_then(parse_tag_version)
        .ok_or_else(|| {
            LifecycleError::network_error(format!(
                "la respuesta de {api_url} no trae `tag_name` con X.Y.Z"
            ))
        })
}

/// Extrae `X.Y.Z` del tag de una URL de release (`…/tag/v0.24.0`) o de un tag
/// suelto (`v0.24.0`). La `v` inicial es opcional; cualquier otra forma da `None`.
pub fn parse_tag_version(url_or_tag: &str) -> Option<String> {
    let tag = url_or_tag.rsplit('/').next().unwrap_or_default();
    normalize_version(tag)
}

/// Normaliza una versión a `X.Y.Z` numérico con `v` inicial opcional, o `None`
/// si no tiene forma de versión.
fn normalize_version(raw: &str) -> Option<String> {
    let bare = raw.trim().strip_prefix('v').unwrap_or(raw.trim());
    if bare.is_empty() || !is_version_shape(bare) {
        return None;
    }
    Some(bare.to_string())
}

/// `true` si el texto son componentes numéricos separados por puntos.
fn is_version_shape(bare: &str) -> bool {
    !bare.is_empty()
        && bare
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{serve_responses, FakeResponse};

    /// Petición sobre la instalada 0.23.1, la versión del contrato vigente.
    fn request(installed: &str, explicit: Option<&str>, force: bool) -> ResolveRequest {
        ResolveRequest {
            installed: installed.to_string(),
            explicit: explicit.map(str::to_string),
            force,
        }
    }

    /// `--version` explícito mayor que la instalada: actualización no destructiva.
    #[test]
    fn update_resolve_explicit_upgrade() {
        let outcome = resolve(&request("0.23.1", Some("0.24.0"), false), "0.24.0").unwrap();
        assert_eq!(outcome.target, "0.24.0");
        assert_eq!(outcome.verdict, Verdict::Upgrade);
        assert!(!outcome.destructive);
    }

    /// La `v` inicial del tag se tolera tanto en `--version` como en la instalada.
    #[test]
    fn update_resolve_v_prefix_is_tolerated() {
        let outcome = resolve(&request("v0.23.1", Some("v0.24.0"), false), "v0.24.0").unwrap();
        assert_eq!(outcome.target, "0.24.0");
        assert_eq!(outcome.verdict, Verdict::Upgrade);
    }

    /// Igualdad sin `--force`: al día, sin marca destructiva.
    #[test]
    fn update_resolve_equal_is_up_to_date_without_force() {
        let outcome = resolve(&request("0.24.0", None, false), "0.24.0").unwrap();
        assert_eq!(outcome.verdict, Verdict::UpToDate);
        assert!(!outcome.destructive);
    }

    /// Igualdad con `--force`: reinstalación autorizada, no destructiva.
    #[test]
    fn update_resolve_equal_with_force_reinstalls() {
        let outcome = resolve(&request("0.24.0", None, true), "0.24.0").unwrap();
        assert_eq!(outcome.target, "0.24.0");
        assert_eq!(outcome.verdict, Verdict::Upgrade);
        assert!(!outcome.destructive);
    }

    /// Objetivo menor con `--version` explícito: degradación destructiva.
    #[test]
    fn update_resolve_explicit_downgrade_is_destructive() {
        let outcome = resolve(&request("0.24.0", Some("0.23.1"), false), "0.99.0").unwrap();
        assert_eq!(outcome.target, "0.23.1");
        assert_eq!(outcome.verdict, Verdict::Downgrade);
        assert!(outcome.destructive);
    }

    /// Objetivo menor sin `--version` explícito: se rechaza como error de uso.
    #[test]
    fn update_resolve_implicit_downgrade_is_rejected() {
        let err = resolve(&request("0.24.0", None, false), "0.23.1").unwrap_err();
        assert_eq!(err.reason, "usage_error");
        assert_eq!(err.exit_code, 2);
    }

    /// `--version` con forma inválida: se rechaza como error de uso.
    #[test]
    fn update_resolve_invalid_explicit_is_rejected() {
        for raw in ["zzz", "v", "", "1.2.x", "0.24.0-rc.1-extra..1"] {
            let err = resolve(&request("0.23.1", Some(raw), false), "0.24.0").unwrap_err();
            assert_eq!(err.reason, "usage_error", "versión {raw}");
        }
    }

    /// La redirección de `releases/latest` fija la última estable por el tag de
    /// la URL final, sin tocar la API.
    #[tokio::test]
    async fn update_resolve_redirect_yields_latest() {
        let client = update_fetch::http_client().unwrap();
        let (base, seen, handle) = serve_responses(vec![
            FakeResponse::redirect("/releases/tag/v0.24.0"),
            FakeResponse::new(200, "cuerpo del release"),
        ])
        .await;
        let latest = latest_stable_from(
            &client,
            &format!("{base}/releases/latest"),
            &format!("{base}/api"),
        )
        .await
        .unwrap();
        assert_eq!(latest, "0.24.0");
        handle.await.expect("el servidor local termina");
        assert_eq!(
            seen.lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone(),
            vec!["/releases/latest", "/releases/tag/v0.24.0"]
        );
    }

    /// La resolución pide solo los extremos de metadatos (página y API): nunca
    /// el archivo ni el `SHA256SUMS.txt`.
    #[tokio::test]
    async fn update_resolve_requests_only_metadata_endpoints() {
        let client = update_fetch::http_client().unwrap();
        let (base, seen, handle) = serve_responses(vec![
            FakeResponse::new(404, "sin redirección"),
            FakeResponse::new(200, r#"{"tag_name": "v0.25.0"}"#),
        ])
        .await;
        let latest = latest_stable_from(
            &client,
            &format!("{base}/releases/latest"),
            &format!("{base}/api"),
        )
        .await
        .unwrap();
        assert_eq!(latest, "0.25.0");
        handle.await.expect("el servidor local termina");
        let seen = seen
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        assert_eq!(seen, vec!["/releases/latest", "/api"]);
        for path in &seen {
            assert!(
                !path.contains("SHA256SUMS")
                    && !path.ends_with(".zip")
                    && !path.ends_with(".tar.gz"),
                "la resolución no descarga archivos: {path}"
            );
        }
    }

    /// Sin redirección útil, la API REST de respaldo aporta el `tag_name`.
    #[tokio::test]
    async fn update_resolve_api_fallback_yields_latest() {
        let client = update_fetch::http_client().unwrap();
        let (base, seen, handle) = serve_responses(vec![
            FakeResponse::new(200, "página sin tag en la URL"),
            FakeResponse::new(200, r#"{"tag_name": "v0.25.0"}"#),
        ])
        .await;
        let latest = latest_stable_from(&client, &format!("{base}/page"), &format!("{base}/api"))
            .await
            .unwrap();
        assert_eq!(latest, "0.25.0");
        handle.await.expect("el servidor local termina");
        assert_eq!(
            seen.lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone(),
            vec!["/page", "/api"]
        );
    }

    /// Si ni la página ni la API dan versión, el fallo es `network_error`.
    #[tokio::test]
    async fn update_resolve_double_failure_is_network_error() {
        let client = update_fetch::http_client().unwrap();
        let (base, _, handle) = serve_responses(vec![
            FakeResponse::new(200, "página sin tag en la URL"),
            FakeResponse::new(200, r#"{"sin_tag": true}"#),
        ])
        .await;
        let err = latest_stable_from(&client, &format!("{base}/page"), &format!("{base}/api"))
            .await
            .unwrap_err();
        assert_eq!(err.reason, "network_error");
        assert_eq!(err.exit_code, 20);
        handle.await.expect("el servidor local termina");
    }

    /// `parse_tag_version` extrae el tag de URLs y de tags sueltos, y rechaza lo
    /// demás.
    #[test]
    fn update_resolve_tag_parsing() {
        assert_eq!(
            parse_tag_version("https://github.com/o/r/releases/tag/v0.24.0"),
            Some("0.24.0".to_string())
        );
        assert_eq!(parse_tag_version("v0.24.0"), Some("0.24.0".to_string()));
        assert_eq!(parse_tag_version("0.24.0"), Some("0.24.0".to_string()));
        assert_eq!(
            parse_tag_version("https://github.com/o/r/releases/latest"),
            None
        );
        assert_eq!(parse_tag_version("v"), None);
        assert_eq!(parse_tag_version(""), None);
    }
}
