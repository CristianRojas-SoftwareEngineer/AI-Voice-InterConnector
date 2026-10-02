//! Directorios canónicos del producto, temporales y revisiones de modelos.
//!
//! Fuente única de las rutas que antes vivían replicadas en `avi-store`,
//! `avi-lifecycle` y `xtask clean`: `avi-store` las reexporta para conservar su
//! API, y los llamadores no cambian.

use std::path::PathBuf;

/// Nombre canónico del producto: directorio de programa, directorio del
/// enlace, nombre del bloqueo y nombre del ejecutable. Fuente única.
pub const APP_NAME: &str = "ai-voice-interconnector";

/// Valor de una variable de entorno de reubicación, ignorando el vacío.
///
/// Además de permitir ubicaciones personalizadas es lo que hace posibles las
/// pruebas aisladas: en Windows las Known Folders ignoran `LOCALAPPDATA`.
fn relocated_dir(var: &str) -> Option<PathBuf> {
    std::env::var(var)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .map(PathBuf::from)
}

fn home_dir() -> PathBuf {
    directories::UserDirs::new()
        .map(|d| d.home_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// `%LOCALAPPDATA%` de la Known Folder del usuario. Si la variable no está
/// definida se deriva de `HOME`: inventar una ruta en el directorio de trabajo
/// pondría el estado del producto dentro de `target/`. Las ramas que la usan
/// son `cfg!(windows)`, así que la función se compila también en Unix.
fn local_app_data() -> PathBuf {
    relocated_dir("LOCALAPPDATA").unwrap_or_else(|| home_dir().join("AppData").join("Local"))
}

/// `$XDG_CACHE_HOME` con el fallback de `~/.cache`.
fn cache_home() -> PathBuf {
    relocated_dir("XDG_CACHE_HOME").unwrap_or_else(|| home_dir().join(".cache"))
}

/// Raíz de datos de usuario y estado: voces, habla sintetizada, configuración,
/// `daemon.pid` y logs.
///
/// `AVI_DATA_DIR` la desvía. Sin la variable, en Windows es
/// `%LOCALAPPDATA%\ai-voice-interconnector\data` (D4), en Linux
/// `$XDG_DATA_HOME/ai-voice-interconnector` y en macOS
/// `~/Library/Application Support/ai-voice-interconnector`, que es justo lo que
/// resuelve `directories` en los tres sistemas Unix.
pub fn data_dir() -> PathBuf {
    if let Some(dir) = relocated_dir("AVI_DATA_DIR") {
        return dir;
    }
    if cfg!(windows) {
        local_app_data().join(APP_NAME).join("data")
    } else {
        directories::ProjectDirs::from("", "", APP_NAME)
            .map(|d| d.data_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from(".").join(APP_NAME))
    }
}

/// Directorio de programa: el bundle completo y el recibo. Existe en los
/// cuatro targets de distribución, no solo en Windows, y `AVI_INSTALL_DIR` lo
/// desvía. Dos `join` y no uno con separador embebido: una ruta con `/` mixto
/// rompe la comparación con el registro de Windows y con el resto del motor.
pub fn install_dir() -> PathBuf {
    if let Some(dir) = relocated_dir("AVI_INSTALL_DIR") {
        return dir;
    }
    if cfg!(windows) {
        local_app_data().join("Programs").join(APP_NAME)
    } else {
        home_dir().join(".local").join("opt").join(APP_NAME)
    }
}

/// Directorio del comando en el PATH. En Unix es el directorio del enlace
/// simbólico; en Windows no hay enlace y la entrada que se escribe en
/// `HKCU\Environment\Path` es el propio directorio de programa, así que ambos
/// coinciden. `AVI_BIN_DIR` lo desvía.
pub fn bin_dir() -> PathBuf {
    if let Some(dir) = relocated_dir("AVI_BIN_DIR") {
        return dir;
    }
    if cfg!(windows) {
        install_dir()
    } else {
        home_dir().join(".local").join("bin")
    }
}

/// Nombre lógico del único modelo opt-in de la tabla de pines: el Base de clonado
/// de voz. Solo se provisiona si la selección del usuario lo pide
/// (`setup --with-voice-cloning`); los demás modelos de la tabla son obligatorios.
pub const CLONING_MODEL: &str = "qwen3-tts-0.6b-base";

/// Pin de un modelo: nombre lógico, repo de HuggingFace, revisión y tamaño.
///
/// La revisión es un **commit hash** de HuggingFace: mismo binario → mismos
/// bytes (reproducibilidad); actualizar un pin es una acción deliberada y
/// auditable en THIRD-PARTY-LICENSES.md.
///
/// `approx_bytes` es lo que se descarga, y por tanto lo que ocupa en disco, en
/// la revisión fijada: la suma de los archivos del repo, o solo la de los que
/// casan con `MODEL_FILE_PATTERNS` cuando el modelo los tiene. Hay que
/// actualizarlo al cambiar la revisión.
#[derive(Debug, Clone, Copy)]
pub struct ModelPin {
    pub name: &'static str,
    pub repo: &'static str,
    pub revision: &'static str,
    pub approx_bytes: u64,
}

/// Pines de los modelos que provisiona la aplicación.
pub const MODEL_REVISIONS: &[ModelPin] = &[
    // Motor TTS Qwen3-TTS 0.6B CustomVoice (pesos safetensors BF16)
    ModelPin {
        name: "qwen3-tts-0.6b",
        repo: "Qwen/Qwen3-TTS-12Hz-0.6B-CustomVoice",
        revision: "85e237c12c027371202489a0ec509ded67b5e4b5",
        approx_bytes: 2_498_388_392,
    },
    // Traducción es→en / en→es: Helsinki-NLP/opus-mt (CC-BY 4.0) convertido una
    // sola vez a CTranslate2 int8 y publicado en repos propios; el motor lo lee
    // directamente del snapshot, sin conversión local.
    ModelPin {
        name: "opus-mt-es-en",
        repo: "CristianRojaas/opus-mt-es-en-ct2-int8",
        revision: "6eabf2f7d9f92dd52e38fe95c9da29f219613c19",
        approx_bytes: 82_536_565,
    },
    ModelPin {
        name: "opus-mt-en-es",
        repo: "CristianRojaas/opus-mt-en-es-ct2-int8",
        revision: "452971fec59e5a4093f8630e55022909dc5beceb",
        approx_bytes: 82_536_565,
    },
    // STT Parakeet TDT 0.6B v3 int8 (export istupakov/onnx-asr; 4 artefactos
    // canónicos — el repo upstream completo pesa decenas de GB)
    ModelPin {
        name: "parakeet-tdt-v3",
        repo: "istupakov/parakeet-tdt-0.6b-v3-onnx",
        revision: "8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce",
        approx_bytes: 670_619_706,
    },
    // Modelo Base Qwen3-TTS 0.6B para clonado de voz (speaker encoder) — snapshot
    // completo. Repo público Qwen/Qwen3-TTS-12Hz-0.6B-Base verificado por dry-run:
    // config.json con "tts_model_type": "base" + speaker_encoder_config; artefactos
    // model.safetensors + speech_tokenizer/model.safetensors (no requiere allow_patterns).
    ModelPin {
        name: "qwen3-tts-0.6b-base",
        repo: "Qwen/Qwen3-TTS-12Hz-0.6B-Base",
        revision: "5d83992436eae1d760afd27aff78a71d676296fc",
        approx_bytes: 2_516_106_051,
    },
];

/// Nombre del bloqueo de ciclo de vida, hermano del directorio de programa:
/// `~/.local/opt/.ai-voice-interconnector.lock`,
/// `%LOCALAPPDATA%\Programs\.ai-voice-interconnector.lock`.
pub const LIFECYCLE_LOCK_NAME: &str = ".ai-voice-interconnector.lock";

/// Prefijo del staging, también hermano del directorio de programa:
/// `~/.local/opt/.ai-voice-interconnector-staging-<pid>-<ms>`.
pub const STAGING_DIR_PREFIX: &str = ".ai-voice-interconnector-staging-";

/// Prefijo de los aparcados **dentro** del directorio de programa:
/// `<programa>/.old-<timestamp>`. Vive dentro a propósito: la reversión de un
/// reemplazo tiene que poder hacerse sin mover nada fuera del programa.
pub const PARKED_DIR_PREFIX: &str = ".old-";

/// Subdirectorio de `xet` dentro de la raíz de modelos exclusiva. Lo fija
/// `ModelStore::new()` en `HF_XET_CACHE`.
pub const MODELS_XET_SUBDIR: &str = "xet";

/// Prefijos de los temporales de ejecución de la aplicación: `$TMPDIR` en
/// Unix y `%TEMP%` en Windows. En Windows los temporales del borrado diferido
/// (`avi-deferred-<pid>-<ms>.ps1` y su marca `.ready`) caen en `avi-`, de modo
/// que ningún otro prefijo puede sustituir a este conjunto.
pub const TEMP_PREFIXES: &[&str] = &["avi-", "avi_"];

// **R4.** Las cinco constantes anteriores son la fuente única de los recursos que la
// aplicación crea: todo recurso nuevo se declara aquí, en la tabla de rutas y en
// los planes de limpieza y desinstalación **en el mismo cambio**. Un recurso
// que solo aparece en un plan de limpieza es un recurso que sobrevive a la
// desinstalación.

/// Patrones de descarga por modelo (`snapshot_download` con `allow_patterns`).
/// Vacío = snapshot completo (repos pequeños/cohesivos). Para `parakeet-tdt-v3`
/// se acota a los 4 artefactos que consume `ParakeetEngine`
/// (`DEFAULT_PARAKEET_MODEL_DIR`); sin esto se bajarían ~40 GB de formatos no usados.
/// Los modelos de traducción se acotan a los cinco ficheros del modelo CTranslate2
/// y excluyen el README y `.gitattributes` del repo.
pub const MODEL_FILE_PATTERNS: &[(&str, &[&str])] = &[
    (
        "parakeet-tdt-v3",
        &[
            "encoder-model.int8.onnx",
            "decoder_joint-model.int8.onnx",
            "nemo128.onnx",
            "vocab.txt",
        ],
    ),
    (
        "opus-mt-es-en",
        &[
            "config.json",
            "model.bin",
            "shared_vocabulary.json",
            "source.spm",
            "target.spm",
        ],
    ),
    (
        "opus-mt-en-es",
        &[
            "config.json",
            "model.bin",
            "shared_vocabulary.json",
            "source.spm",
            "target.spm",
        ],
    ),
];

/// Raíz de la caché de HuggingFace que el usuario eligió compartir, si la
/// eligió: `HF_HUB_CACHE` o, en su defecto, `HF_HOME/hub`. `None` significa
/// que rige la raíz de modelos exclusiva de la aplicación.
///
/// `HF_HUB_CACHE` tiene precedencia porque es la que nombra la caché de `hub`
/// directamente; `HF_HOME` es la convención y su caché vive en `hub/` dentro.
pub fn shared_hf_root() -> Option<PathBuf> {
    if let Some(dir) = relocated_dir("HF_HUB_CACHE") {
        return Some(dir);
    }
    relocated_dir("HF_HOME").map(|home| home.join("hub"))
}

/// ¿La raíz de modelos es la caché HF compartida que eligió el usuario? En ese
/// caso su contenido es compartido y rige R3: solo se borran entradas
/// atribuibles a la aplicación.
pub fn models_root_is_shared() -> bool {
    shared_hf_root().is_some()
}

/// Raíz de modelos: caché regenerable y **exclusiva de la aplicación** por
/// defecto (D3). Snapshots, locks y `xet` cuelgan todos de
/// ella, y es la única ubicación de modelos que existe.
///
/// `AVI_CACHE_DIR` tiene precedencia sobre las variables de HuggingFace: es la
/// variable de reubicación de la aplicación y es lo que permite aislar una
/// prueba aunque el entorno tenga un `HF_HOME` global.
///
/// Sin reubicación y con una caché HF compartida elegida por el usuario
/// (`HF_HUB_CACHE` o `HF_HOME`), la raíz es esa y su contenido es compartido.
/// En cualquier otro caso es la columna de modelos de la tabla de rutas:
/// `$XDG_CACHE_HOME/ai-voice-interconnector/models`,
/// `~/Library/Caches/ai-voice-interconnector/models` o
/// `%LOCALAPPDATA%\ai-voice-interconnector\cache\models`.
pub fn models_cache_dir() -> PathBuf {
    if let Some(dir) = relocated_dir("AVI_CACHE_DIR") {
        return dir;
    }
    if let Some(shared) = shared_hf_root() {
        return shared;
    }
    if cfg!(windows) {
        local_app_data().join(APP_NAME).join("cache").join("models")
    } else if cfg!(target_os = "macos") {
        home_dir()
            .join("Library")
            .join("Caches")
            .join(APP_NAME)
            .join("models")
    } else {
        cache_home().join(APP_NAME).join("models")
    }
}

/// Directorio de la caché `xet` (shard-cache) del proceso que provisiona
/// modelos.
///
/// En la raíz de modelos exclusiva cuelga de ella, y `ModelStore::new()` fija
/// `HF_XET_CACHE` a este mismo sitio para que el borrado sea de directorio
/// entero. En la raíz compartida la variable es del usuario y no se toca, así
/// que se devuelve la cadena real que resuelve `xet-runtime`: `HF_XET_CACHE` →
/// `HF_HOME/xet` → `XDG_CACHE_HOME/huggingface/xet` →
/// `~/.cache/huggingface/xet`.
pub fn xet_cache_dir() -> PathBuf {
    if models_root_is_shared() {
        if let Some(dir) = relocated_dir("HF_XET_CACHE") {
            return dir;
        }
        if let Some(home) = relocated_dir("HF_HOME") {
            return home.join("xet");
        }
        return cache_home().join("huggingface").join("xet");
    }
    models_cache_dir().join(MODELS_XET_SUBDIR)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Los modelos de traducción se fijan a los repositorios publicados ya
    /// convertidos y se acotan a los cinco ficheros del modelo; no queda ningún
    /// pin `marian-*`.
    #[test]
    fn translation_pins_point_to_converted_repos() {
        let expected = [
            (
                "opus-mt-es-en",
                "CristianRojaas/opus-mt-es-en-ct2-int8",
                "6eabf2f7d9f92dd52e38fe95c9da29f219613c19",
            ),
            (
                "opus-mt-en-es",
                "CristianRojaas/opus-mt-en-es-ct2-int8",
                "452971fec59e5a4093f8630e55022909dc5beceb",
            ),
        ];
        for (name, repo, revision) in expected {
            let pin = MODEL_REVISIONS
                .iter()
                .find(|p| p.name == name)
                .unwrap_or_else(|| panic!("falta el pin {name}"));
            assert_eq!(pin.repo, repo);
            assert_eq!(pin.revision, revision);
            assert_eq!(pin.approx_bytes, 82_536_565);
            let (_, patterns) = MODEL_FILE_PATTERNS
                .iter()
                .find(|(n, _)| *n == name)
                .unwrap_or_else(|| panic!("faltan los patrones de {name}"));
            assert_eq!(
                *patterns,
                [
                    "config.json",
                    "model.bin",
                    "shared_vocabulary.json",
                    "source.spm",
                    "target.spm"
                ]
            );
        }
        assert!(
            MODEL_REVISIONS
                .iter()
                .all(|p| !p.name.starts_with("marian-")),
            "no queda ningún pin marian-*"
        );
    }
}
