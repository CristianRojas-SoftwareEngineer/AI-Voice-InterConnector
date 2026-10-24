//! Modelo DPDFNet: resolución del artefacto provisionado por AVI.
//!
//! El catálogo (`avi-shared::paths`) fija repo, revisión, tamaño y SHA256, y
//! `avi-store` descarga y verifica. Este módulo no mantiene caché ni descarga:
//! resuelve el archivo validado y falla si no está provisionado, de modo que
//! una operación de limpieza nunca dispara una descarga silenciosa.
//!
//! Licencia del modelo: Apache 2.0 (Ceva-IP/DPDFNet).

use crate::errors::E;
use avi_shared::paths::{DPDFNET_MODEL, DPDFNET_MODEL_FILE};
use std::path::PathBuf;

/// Rutas absolutas al archivo ONNX del modelo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelPaths {
    pub onnx: PathBuf,
}

/// Trait para abstracción de proveedores de modelos (permite `FakeProvider` en tests).
pub trait ModelsProvider: Send + Sync {
    fn ensure_models(&self) -> Result<ModelPaths, E>;
}

/// Proveedor de producción: resuelve y verifica el artefacto en la caché única
/// de AVI (`avi-store`). No descarga: la provisión es de `setup`.
pub struct AviModelsProvider;

impl ModelsProvider for AviModelsProvider {
    fn ensure_models(&self) -> Result<ModelPaths, E> {
        let store = avi_store::ModelStore::new().map_err(|e| {
            E::EModelMissing(format!("No se pudo abrir la caché de modelos de AVI: {e}"))
        })?;
        if !store.is_provisioned(DPDFNET_MODEL) {
            return Err(E::EModelMissing(format!(
                "El modelo {DPDFNET_MODEL} no está provisionado; ejecuta `ai-voice-interconnector setup` y comprueba el resultado con `ai-voice-interconnector doctor`"
            )));
        }
        store.verify_artifact(DPDFNET_MODEL).map_err(|e| {
            E::EModelMissing(format!(
                "El modelo {DPDFNET_MODEL} no supera la verificación de integridad: {e}; ejecuta `ai-voice-interconnector setup --force-update` para repararlo"
            ))
        })?;
        Ok(ModelPaths {
            onnx: store.model_dir(DPDFNET_MODEL).join(DPDFNET_MODEL_FILE),
        })
    }
}

/// Proveedor simulado para tests unitarios rápidos sin red ni ONNX real.
pub struct FakeProvider {
    pub should_fail: bool,
    pub fail_with_io: bool,
}

impl FakeProvider {
    pub fn failing_missing() -> Self {
        Self {
            should_fail: true,
            fail_with_io: false,
        }
    }

    pub fn failing_io() -> Self {
        Self {
            should_fail: true,
            fail_with_io: true,
        }
    }
}

impl ModelsProvider for FakeProvider {
    fn ensure_models(&self) -> Result<ModelPaths, E> {
        if self.should_fail {
            if self.fail_with_io {
                return Err(E::EIo(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "Simulated I/O permission denied on model directory",
                )));
            } else {
                return Err(E::EModelMissing(
                    "Modelo no encontrado (simulado); ejecuta `ai-voice-interconnector setup`"
                        .to_string(),
                ));
            }
        }

        Ok(ModelPaths {
            onnx: PathBuf::from("modelo-simulado.onnx"),
        })
    }
}
