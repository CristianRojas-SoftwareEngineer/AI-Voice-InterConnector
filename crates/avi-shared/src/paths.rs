//! Directorios canónicos del producto, temporales y revisiones de modelos.
//!
//! Fuente única de las rutas que antes vivían replicadas en `avi-store`,
//! `avi-lifecycle` y `xtask clean`: `avi-store` las reexporta para conservar su
//! API, y los llamadores no cambian.

use std::path::{Path, PathBuf};

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

/// Pines de modelos: `(nombre_lógico, repo HF, revisión)`.
/// La revisión es un **commit hash** de HuggingFace: mismo binario → mismos
/// bytes (reproducibilidad); actualizar un pin es una acción deliberada y
/// auditable en THIRD-PARTY-LICENSES.md.
pub const MODEL_REVISIONS: &[(&str, &str, &str)] = &[
    // Motor TTS Qwen3-TTS 0.6B CustomVoice (pesos safetensors BF16)
    (
        "qwen3-tts-0.6b",
        "Qwen/Qwen3-TTS-12Hz-0.6B-CustomVoice",
        "85e237c12c027371202489a0ec509ded67b5e4b5",
    ),
    // Traducción es→en / en→es (Marian opus-mt convertido a CTranslate2)
    (
        "marian-es-en",
        "Helsinki-NLP/opus-mt-es-en",
        "c96e2c5399ebfae4fc43d9669556b9afa74bb69d",
    ),
    (
        "marian-en-es",
        "Helsinki-NLP/opus-mt-en-es",
        "5bc4493d463cf000c1f0b50f8d56886a392ed4ab",
    ),
    // STT Parakeet TDT 0.6B v3 int8 (export istupakov/onnx-asr; 4 artefactos
    // canónicos — el repo upstream completo pesa decenas de GB)
    (
        "parakeet-tdt-v3",
        "istupakov/parakeet-tdt-0.6b-v3-onnx",
        "8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce",
    ),
    // Modelo Base Qwen3-TTS 0.6B para clonado de voz (speaker encoder) — snapshot
    // completo. Repo público Qwen/Qwen3-TTS-12Hz-0.6B-Base verificado por dry-run:
    // config.json con "tts_model_type": "base" + speaker_encoder_config; artefactos
    // model.safetensors + speech_tokenizer/model.safetensors (no requiere allow_patterns).
    (
        "qwen3-tts-0.6b-base",
        "Qwen/Qwen3-TTS-12Hz-0.6B-Base",
        "5d83992436eae1d760afd27aff78a71d676296fc",
    ),
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
/// Unix y `%TEMP%` en Windows. En Windows el helper de borrado diferido
/// (`avi-uninstall-<pid>-<ms>.ps1`) cae en `avi-`, de modo que ningún otro
/// prefijo puede sustituir a este conjunto.
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
pub const MODEL_FILE_PATTERNS: &[(&str, &[&str])] = &[(
    "parakeet-tdt-v3",
    &[
        "encoder-model.int8.onnx",
        "decoder_joint-model.int8.onnx",
        "nemo128.onnx",
        "vocab.txt",
    ],
)];

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
/// defecto (D3). Snapshots, derivado CT2, locks y `xet` cuelgan todos de
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

/// Directorio CT2 derivado obligatorio de Marian HF en
/// `models_cache_dir()/ct2`.
/// Layout: `models_cache_dir()/ct2/opus-mt-es-en` y `opus-mt-en-es`, cada uno con
/// `model.bin` CT2 más tokenizador utilizable por el loader (`tokenizer.json`,
/// o `source.spm` más `target.spm` copiados desde el snapshot por `setup`).
/// Invariante: provisionado equivale a lo que `setup` deposita y, por tanto, a
/// cargable por `Translator::new` — el gate `is_ct2_provisioned` acepta
/// exactamente los layouts que `convert_marian_to_ct2` produce (`src/main.rs`),
/// no todo lo que `auto::Tokenizer` sabría cargar; la idempotencia por `mtime`
/// solo aplica a dirs sanos (un dir roto es no provisionado y fuerza reconversión).
pub fn ct2_cache_dir() -> PathBuf {
    ct2_cache_dir_at(&models_cache_dir())
}
/// Caché de derivados CT2 bajo una raíz de modelos dada. La raíz del usuario es
/// un caso de esta función y no su fuente: un sandbox necesita la misma
/// disposición sin tocar la caché real.
pub fn ct2_cache_dir_at(models_root: &Path) -> PathBuf {
    models_root.join("ct2")
}
pub fn ct2_model_dir(pair: &str) -> PathBuf {
    ct2_model_dir_at(&models_cache_dir(), pair)
}
/// Directorio del derivado CT2 de un par bajo una raíz de modelos dada.
pub fn ct2_model_dir_at(models_root: &Path, pair: &str) -> PathBuf {
    ct2_cache_dir_at(models_root).join(format!("opus-mt-{}", pair))
}
/// Ficheros ausentes del derivado CT2 en `dir`: vacío equivale a cargable por
/// el loader (`model.bin` presente más tokenizador completo). Nombra cada
/// candidato ausente para errores accionables.
///
/// El tokenizador se da por válido con `tokenizer.json` (layout HF) o
/// `source.spm`+`target.spm` (SentencePiece/Marian). El layout BPE
/// `vocab.json`+`merges.txt` se rechaza a propósito: `convert_marian_to_ct2`
/// fija la salida a `source.spm`+`target.spm` (`--copy_files`) y aborta si el
/// snapshot no los trae (`src/main.rs`), así que ningún derivado de este
/// pipeline lo usa. Admitir esa rama sería especulativo y podría enmascarar un
/// dir incompleto; se añadiría solo si un pin de modelo futuro la exigiera.
pub fn ct2_dir_missing_files(dir: &Path) -> Vec<String> {
    let mut missing = Vec::new();
    if !dir.join("model.bin").is_file() {
        missing.push("model.bin".to_string());
    }
    let tokenizer_ok = dir.join("tokenizer.json").is_file()
        || (dir.join("source.spm").is_file() && dir.join("target.spm").is_file());
    if !tokenizer_ok {
        for candidate in ["tokenizer.json", "source.spm", "target.spm"] {
            if !dir.join(candidate).is_file() {
                missing.push(candidate.to_string());
            }
        }
    }
    missing
}
/// Ficheros ausentes del derivado CT2 del par (`es-en`/`en-es`).
pub fn ct2_missing_files(pair: &str) -> Vec<String> {
    ct2_dir_missing_files(&ct2_model_dir(pair))
}
/// Si el derivado CT2 de un par está completo bajo una raíz de modelos dada.
/// La comprobación viaja con la raíz para que quien describe un árbol de
/// modelos describa ese árbol y no el del usuario.
pub fn is_ct2_provisioned_at(models_root: &Path, pair: &str) -> bool {
    ct2_dir_missing_files(&ct2_model_dir_at(models_root, pair)).is_empty()
}
pub fn is_ct2_provisioned(pair: &str) -> bool {
    is_ct2_provisioned_at(&models_cache_dir(), pair)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// El derivado de un par vive en `ct2/opus-mt-<par>` bajo la raíz que recibe
    /// la función, y la variante global es la misma disposición aplicada a la raíz
    /// del usuario. La paridad importa porque ambas son una sola disposición
    /// declarada en un sitio, no dos: si divergieran, un reporte describiría un
    /// árbol y el motor leería otro.
    #[test]
    fn ct2_layout_depends_on_the_given_root() {
        let sandbox = Path::new("/sandbox/models");
        assert_eq!(
            ct2_model_dir_at(sandbox, "es-en"),
            Path::new("/sandbox/models/ct2/opus-mt-es-en")
        );
        assert_eq!(
            ct2_model_dir("es-en"),
            ct2_model_dir_at(&models_cache_dir(), "es-en")
        );
    }

    /// La provisión consulta la raíz que recibe y no otra: una raíz vacía no
    /// declara provisionado un derivado que sí existe en una raíz hermana. Esta
    /// es la comprobación que da valor a la anterior: si la función mirase la
    /// caché global, el sandbox vacío lo declararía completo en cualquier máquina
    /// que tenga el par convertido.
    #[test]
    fn ct2_provisioning_does_not_leak_from_a_sibling_root() {
        let base = std::env::temp_dir().join(format!("ct2-at-{}", std::process::id()));
        let empty = base.join("vacio");
        let full = base.join("lleno");
        std::fs::create_dir_all(&empty).expect("se crea la raíz vacía");
        let derived = ct2_model_dir_at(&full, "es-en");
        std::fs::create_dir_all(&derived).expect("se crea el derivado");
        std::fs::write(derived.join("model.bin"), b"pesos").expect("se escribe el modelo");
        std::fs::write(derived.join("source.spm"), b"tok").expect("se escribe el tokenizador");
        std::fs::write(derived.join("target.spm"), b"tok").expect("se escribe el tokenizador");

        assert!(
            is_ct2_provisioned_at(&full, "es-en"),
            "el derivado completo se declara provisionado en su propia raíz"
        );
        assert!(
            !is_ct2_provisioned_at(&empty, "es-en"),
            "una raíz sin el derivado no lo declara provisionado por tener otra raíz completo"
        );

        let _ = std::fs::remove_dir_all(&base);
    }
}
