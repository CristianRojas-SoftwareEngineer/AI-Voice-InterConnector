//! Runtime de las pruebas síncronas. Lo declaran con
//! `#[path = "support/runtime.rs"] mod runtime;`.

/// Runtime de un solo hilo: las operaciones del motor son asíncronas pero no
/// necesitan concurrencia, y un runtime por prueba evita el coste y el
/// `block_on` anidado de un runtime global.
pub fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime de la prueba")
}
