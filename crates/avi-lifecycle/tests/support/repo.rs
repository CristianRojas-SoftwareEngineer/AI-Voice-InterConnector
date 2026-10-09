//! Nombres de la caché de modelos. Lo declaran con
//! `#[path = "support/repo.rs"] mod repo;`.

/// Nombre del directorio de repo en la caché de HuggingFace, tal y como lo construye
/// `avi-store`.
pub fn repo_dir(repo: &str) -> String {
    format!("models--{}", repo.replace('/', "--"))
}
