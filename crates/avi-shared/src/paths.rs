// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! Directorios canónicos del producto, temporales y revisiones de modelos.
//!
//! Fuente única de las rutas que comparten `avi-store`, `avi-lifecycle`, el
//! daemon y `xtask`. Las funciones que parten del directorio del usuario
//! devuelven `io::Result`: si no se puede resolver (`HOME` o `USERPROFILE` sin
//! definir) es un error, nunca una ruta inventada en el directorio de trabajo.

use std::io;
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

/// Error de una ruta que no se puede resolver: nombra qué falta y qué
/// variable lo remedia.
fn unresolved(what: &str, remedy: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::NotFound,
        format!("No se pudo resolver {what}: {remedy}"),
    )
}

/// Directorio del usuario (`HOME` en Unix, Known Folder del perfil en Windows).
fn home_dir() -> io::Result<PathBuf> {
    directories::UserDirs::new()
        .map(|d| d.home_dir().to_path_buf())
        .ok_or_else(|| {
            unresolved(
                "el directorio del usuario",
                "defina HOME (USERPROFILE en Windows)",
            )
        })
}

/// `%LOCALAPPDATA%` de la Known Folder del usuario. Si la variable no está
/// definida se deriva de `HOME`: inventar una ruta en el directorio de trabajo
/// pondría el estado del producto dentro de `target/`. Las ramas que la usan
/// son `cfg!(windows)`, así que la función se compila también en Unix.
fn local_app_data() -> io::Result<PathBuf> {
    match relocated_dir("LOCALAPPDATA") {
        Some(dir) => Ok(dir),
        None => Ok(home_dir()
            .map_err(|_| unresolved("%LOCALAPPDATA%", "defina LOCALAPPDATA o USERPROFILE"))?
            .join("AppData")
            .join("Local")),
    }
}

/// `$XDG_CACHE_HOME` con el fallback de `~/.cache`.
fn cache_home() -> io::Result<PathBuf> {
    match relocated_dir("XDG_CACHE_HOME") {
        Some(dir) => Ok(dir),
        None => Ok(home_dir()
            .map_err(|_| unresolved("la caché del usuario", "defina XDG_CACHE_HOME o HOME"))?
            .join(".cache")),
    }
}

/// Raíz de datos de usuario y estado: voces, habla sintetizada, configuración,
/// `daemon.pid`; los logs viven en `logs_dir()`.
///
/// `AVI_DATA_DIR` la desvía. Sin la variable, en Windows es
/// `%LOCALAPPDATA%\ai-voice-interconnector\data`, en Linux
/// `$XDG_DATA_HOME/ai-voice-interconnector` y en macOS
/// `~/Library/Application Support/ai-voice-interconnector`, que es justo lo que
/// resuelve `directories` en los tres sistemas Unix. Si no se puede resolver
/// ninguna es un error que nombra las variables que lo remedian.
pub fn data_dir() -> io::Result<PathBuf> {
    if let Some(dir) = relocated_dir("AVI_DATA_DIR") {
        return Ok(dir);
    }
    if cfg!(windows) {
        Ok(local_app_data()?.join(APP_NAME).join("data"))
    } else {
        directories::ProjectDirs::from("", "", APP_NAME)
            .map(|d| d.data_dir().to_path_buf())
            .ok_or_else(|| {
                unresolved(
                    "el directorio de datos",
                    "defina AVI_DATA_DIR, XDG_DATA_HOME o HOME",
                )
            })
    }
}

/// Directorio de logs bajo una raíz de datos dada: es la única definición de
/// `<datos>/logs`, para quien trabaja con raíces reubicadas (`cleanup`).
pub fn logs_dir_in(data_dir: &Path) -> PathBuf {
    data_dir.join("logs")
}

/// Directorio de logs vigente: los del daemon (`daemon_*`), los del motor
/// (`qwen3-tts_*`) y los del limpiador (`cleaner_*`), con retención por familia
/// (ver `logs`).
pub fn logs_dir() -> io::Result<PathBuf> {
    Ok(logs_dir_in(&data_dir()?))
}

/// Directorio de programa: el bundle completo y el recibo. Existe en los
/// cuatro targets de distribución, no solo en Windows, y `AVI_INSTALL_DIR` lo
/// desvía. Dos `join` y no uno con separador embebido: una ruta con `/` mixto
/// rompe la comparación con el registro de Windows y con el resto del motor.
pub fn install_dir() -> io::Result<PathBuf> {
    if let Some(dir) = relocated_dir("AVI_INSTALL_DIR") {
        return Ok(dir);
    }
    if cfg!(windows) {
        Ok(local_app_data()?.join("Programs").join(APP_NAME))
    } else {
        Ok(home_dir()?.join(".local").join("opt").join(APP_NAME))
    }
}

/// Directorio real del programa en ejecución: el que contiene el ejecutable y
/// todo lo que el bundle trae junto a él (el motor y ONNX Runtime). En Unix la
/// app se invoca desde el symlink de `bin_dir()` y `current_exe()` no lo
/// resuelve en todos los sistemas (en macOS devuelve la ruta del symlink), así
/// que se resuelve aquí; en Windows no hay symlink y canonicalizar daría una
/// ruta `\\?\` que no coincide con las del resto del programa.
pub fn program_dir() -> io::Result<PathBuf> {
    real_parent(&std::env::current_exe()?)
}

/// Directorio que contiene `exe` tras resolver sus enlaces simbólicos en Unix.
fn real_parent(exe: &Path) -> io::Result<PathBuf> {
    #[cfg(unix)]
    let exe = &std::fs::canonicalize(exe)?;
    exe.parent().map(Path::to_path_buf).ok_or_else(|| {
        io::Error::other(format!(
            "el ejecutable {} no tiene directorio padre",
            exe.display()
        ))
    })
}

/// Directorio del comando en el PATH. En Unix es el directorio del enlace
/// simbólico; en Windows no hay enlace y la entrada que se escribe en
/// `HKCU\Environment\Path` es el propio directorio de programa, así que ambos
/// coinciden. `AVI_BIN_DIR` lo desvía.
pub fn bin_dir() -> io::Result<PathBuf> {
    if let Some(dir) = relocated_dir("AVI_BIN_DIR") {
        return Ok(dir);
    }
    if cfg!(windows) {
        install_dir()
    } else {
        Ok(home_dir()?.join(".local").join("bin"))
    }
}

/// Nombre lógico del modelo Base de clonado de voz, que sintetiza las voces
/// clonadas (la de fábrica `default` incluida). Como los demás modelos de la
/// tabla de pines, `setup` lo provisiona siempre.
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

/// Nombre de la compuerta de traspaso, hermana del bloqueo de ciclo de vida:
/// `~/.local/opt/.ai-voice-interconnector.gate`,
/// `%LOCALAPPDATA%\Programs\.ai-voice-interconnector.gate`.
pub const LIFECYCLE_GATE_NAME: &str = ".ai-voice-interconnector.gate";

/// Prefijo del staging, también hermano del directorio de programa:
/// `~/.local/opt/.ai-voice-interconnector-staging-<token>-<etiqueta>`. El
/// nombre completo lo construye `staging_names::install_staging`.
pub const STAGING_DIR_PREFIX: &str = ".ai-voice-interconnector-staging-";

/// Prefijo de los aparcados **dentro** del directorio de programa:
/// `<programa>/.old-<token>-<único>`. El nombre completo lo construye
/// `staging_names::parked`. Vive dentro a propósito: la reversión de un
/// reemplazo tiene que poder hacerse sin mover nada fuera del programa.
pub const PARKED_DIR_PREFIX: &str = ".old-";

/// Subdirectorio de `xet` dentro de la raíz de modelos exclusiva. Lo fija
/// `ModelStore::new()` en `HF_XET_CACHE`.
pub const MODELS_XET_SUBDIR: &str = "xet";

// Las constantes de nombres y prefijos de arriba son la fuente única de los recursos que la
// aplicación crea: todo recurso nuevo se declara aquí, en la tabla de rutas y en
// los planes de limpieza y desinstalación en el mismo cambio. Un recurso
// que solo aparece en un plan de limpieza es un recurso que sobrevive a la
// desinstalación.

/// Patrones de descarga por modelo (`snapshot_download` con `allow_patterns`).
/// Vacío = snapshot completo (repos pequeños/cohesivos). Para `parakeet-tdt-v3`
/// se acota a los 4 artefactos que consume `ParakeetEngine`; sin esto se bajarían
/// ~40 GB de formatos no usados.
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
/// caso su contenido es compartido y solo se borran entradas atribuibles a la
/// aplicación.
pub fn models_root_is_shared() -> bool {
    shared_hf_root().is_some()
}

/// Nombre de la hoja de modelos dentro del contenedor de caché.
pub const MODELS_SUBDIR: &str = "models";

/// Nombre de la hoja de temporales de la aplicación dentro del contenedor de
/// caché.
pub const SCRATCH_SUBDIR: &str = "tmp";

/// Contenedor de la caché regenerable de la aplicación: de él cuelgan los
/// modelos (`models`) y los temporales (`tmp`).
///
/// `AVI_CACHE_DIR` lo desvía y nombra **el contenedor**, no los modelos. Sin la
/// variable es `$XDG_CACHE_HOME/ai-voice-interconnector`,
/// `~/Library/Caches/ai-voice-interconnector` o
/// `%LOCALAPPDATA%\ai-voice-interconnector\cache`.
pub fn cache_container() -> io::Result<PathBuf> {
    if let Some(dir) = relocated_dir("AVI_CACHE_DIR") {
        return Ok(dir);
    }
    if cfg!(windows) {
        Ok(local_app_data()?.join(APP_NAME).join("cache"))
    } else if cfg!(target_os = "macos") {
        Ok(home_dir()?.join("Library").join("Caches").join(APP_NAME))
    } else {
        Ok(cache_home()?.join(APP_NAME))
    }
}

/// Raíz de modelos compartida que rige, si rige: la caché HF elegida por el
/// usuario, salvo que `AVI_CACHE_DIR` reubique la aplicación, que manda sobre
/// las variables de HuggingFace.
fn effective_shared_root() -> Option<PathBuf> {
    if relocated_dir("AVI_CACHE_DIR").is_some() {
        return None;
    }
    shared_hf_root()
}

/// Raíz de modelos: caché regenerable y **exclusiva de la aplicación** por
/// defecto, la hoja `models` del contenedor de caché. Snapshots, locks y `xet`
/// cuelgan todos de ella, y es la única ubicación de modelos que existe.
///
/// `AVI_CACHE_DIR` tiene precedencia sobre las variables de HuggingFace: es la
/// variable de reubicación de la aplicación y es lo que permite aislar una
/// prueba aunque el entorno tenga un `HF_HOME` global; con ella, los modelos
/// son `<AVI_CACHE_DIR>/models`.
///
/// Sin reubicación y con una caché HF compartida elegida por el usuario
/// (`HF_HUB_CACHE` o `HF_HOME`), la raíz es esa y su contenido es compartido.
/// En cualquier otro caso es `<contenedor de caché>/models`.
pub fn models_cache_dir() -> io::Result<PathBuf> {
    if let Some(shared) = effective_shared_root() {
        return Ok(shared);
    }
    Ok(cache_container()?.join(MODELS_SUBDIR))
}

/// Raíz de los temporales de la aplicación: `<contenedor de caché>/tmp`.
pub fn scratch_root() -> io::Result<PathBuf> {
    Ok(cache_container()?.join(SCRATCH_SUBDIR))
}

/// Papel de un nodo del árbol de propiedad de la aplicación.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    /// Directorio de la aplicación que solo contiene nodos suyos: se borra
    /// entero cuando sus hijos ya no existen.
    Container,
    /// Directorio de la aplicación cuyo contenido es todo suyo.
    Leaf,
    /// Ubicación que elige y posee el usuario (caché HF compartida): la
    /// aplicación solo borra en ella lo que puede atribuirse.
    External,
}

/// Nodo del árbol de propiedad: una ruta y su papel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnershipNode {
    pub path: PathBuf,
    pub kind: NodeKind,
}

fn node(path: PathBuf, kind: NodeKind) -> OwnershipNode {
    OwnershipNode { path, kind }
}

/// Árbol de propiedad vigente, con cada padre antes que sus hijos.
///
/// Windows: el contenedor raíz `%LOCALAPPDATA%\ai-voice-interconnector`, con la
/// hoja `data` y el contenedor `cache` (hojas `models` y `tmp`). El contenedor
/// raíz solo existe como nodo si alguna de las dos raíces cuelga de él, es
/// decir, si no están ambas reubicadas. Linux y macOS no tienen contenedor raíz:
/// la hoja de datos y el contenedor de caché son hermanos de los directorios
/// estándar del sistema.
///
/// `AVI_DATA_DIR` mueve la hoja de datos y `AVI_CACHE_DIR` el contenedor de
/// caché. La hoja `models` es siempre `Leaf`; una caché HF compartida vigente se
/// añade al final como nodo `External`. El
/// directorio de programa, `Programs`, `~/.local/opt` y `~/.local/bin` nunca
/// son nodos: el programa se modela aparte, con [`install_dir`].
pub fn ownership_tree() -> io::Result<Vec<OwnershipNode>> {
    let mut tree = Vec::new();
    let data_relocated = relocated_dir("AVI_DATA_DIR").is_some();
    let cache_relocated = relocated_dir("AVI_CACHE_DIR").is_some();
    if cfg!(windows) && !(data_relocated && cache_relocated) {
        tree.push(node(local_app_data()?.join(APP_NAME), NodeKind::Container));
    }
    tree.push(node(data_dir()?, NodeKind::Leaf));
    tree.push(node(cache_container()?, NodeKind::Container));
    tree.push(node(cache_container()?.join(MODELS_SUBDIR), NodeKind::Leaf));
    tree.push(node(scratch_root()?, NodeKind::Leaf));
    if let Some(shared) = effective_shared_root() {
        tree.push(node(shared, NodeKind::External));
    }
    Ok(tree)
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
pub fn xet_cache_dir() -> io::Result<PathBuf> {
    if models_root_is_shared() {
        if let Some(dir) = relocated_dir("HF_XET_CACHE") {
            return Ok(dir);
        }
        if let Some(home) = relocated_dir("HF_HOME") {
            return Ok(home.join("xet"));
        }
        return Ok(cache_home()?.join("huggingface").join("xet"));
    }
    Ok(models_cache_dir()?.join(MODELS_XET_SUBDIR))
}

// Las pruebas cuelgan sus raíces simuladas del temporal del sistema: no son
// código de producto y no tienen scratch.
#[allow(clippy::disallowed_methods)]
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// El entorno del proceso es global: las pruebas que lo tocan se serializan.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    const ENV_VARS: &[&str] = &[
        "AVI_DATA_DIR",
        "AVI_CACHE_DIR",
        "HF_HUB_CACHE",
        "HF_HOME",
        "LOCALAPPDATA",
        "HOME",
        "XDG_DATA_HOME",
        "XDG_CACHE_HOME",
    ];

    /// Aísla el entorno en `base` y lo restaura al soltarse. Toma el cerrojo.
    struct EnvSandbox {
        _lock: std::sync::MutexGuard<'static, ()>,
        saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
    }

    impl EnvSandbox {
        fn new(base: &Path) -> Self {
            let lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
            let saved = ENV_VARS.iter().map(|v| (*v, std::env::var_os(v))).collect();
            for var in ENV_VARS {
                std::env::remove_var(var);
            }
            std::env::set_var("LOCALAPPDATA", base.join("Local"));
            std::env::set_var("HOME", base.join("home"));
            std::env::set_var("XDG_DATA_HOME", base.join("xdg_data"));
            std::env::set_var("XDG_CACHE_HOME", base.join("xdg_cache"));
            Self { _lock: lock, saved }
        }
    }

    impl Drop for EnvSandbox {
        fn drop(&mut self) {
            for (var, value) in &self.saved {
                match value {
                    Some(v) => std::env::set_var(var, v),
                    None => std::env::remove_var(var),
                }
            }
        }
    }

    fn leaf(path: PathBuf) -> OwnershipNode {
        node(path, NodeKind::Leaf)
    }

    fn container(path: PathBuf) -> OwnershipNode {
        node(path, NodeKind::Container)
    }

    /// Árbol por defecto del sistema que compila la prueba, sobre `base`.
    fn default_tree(base: &Path) -> Vec<OwnershipNode> {
        #[cfg(windows)]
        {
            let root = base.join("Local").join(APP_NAME);
            let cache = root.join("cache");
            vec![
                container(root.clone()),
                leaf(root.join("data")),
                container(cache.clone()),
                leaf(cache.join("models")),
                leaf(cache.join("tmp")),
            ]
        }
        #[cfg(target_os = "macos")]
        {
            let library = base.join("home").join("Library");
            let cache = library.join("Caches").join(APP_NAME);
            vec![
                leaf(library.join("Application Support").join(APP_NAME)),
                container(cache.clone()),
                leaf(cache.join("models")),
                leaf(cache.join("tmp")),
            ]
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            let cache = base.join("xdg_cache").join(APP_NAME);
            vec![
                leaf(base.join("xdg_data").join(APP_NAME)),
                container(cache.clone()),
                leaf(cache.join("models")),
                leaf(cache.join("tmp")),
            ]
        }
    }

    /// Posición del nodo de datos, de la caché y de los modelos en el árbol por
    /// defecto: depende de que haya o no contenedor raíz.
    const DATA: usize = if cfg!(windows) { 1 } else { 0 };
    const CACHE: usize = DATA + 1;
    const MODELS: usize = DATA + 2;
    const SCRATCH: usize = DATA + 3;

    /// El árbol por defecto fija contenedor raíz (solo Windows), hoja de datos,
    /// contenedor de caché y sus hojas `models` y `tmp`; el programa no figura.
    #[test]
    fn ownership_tree_default_per_system() {
        let base = std::env::temp_dir().join("avi-paths-default");
        let _env = EnvSandbox::new(&base);
        let tree = ownership_tree().expect("árbol de propiedad");
        assert_eq!(tree, default_tree(&base));
        let program = install_dir().expect("directorio de programa");
        assert!(
            tree.iter().all(|n| !n.path.starts_with(&program)),
            "el directorio de programa no es un nodo: {program:?}"
        );
        assert_eq!(models_cache_dir().unwrap(), tree[MODELS].path);
        assert_eq!(scratch_root().unwrap(), tree[SCRATCH].path);
        assert_eq!(cache_container().unwrap(), tree[CACHE].path);
        assert_eq!(data_dir().unwrap(), tree[DATA].path);
    }

    /// `AVI_CACHE_DIR` nombra el contenedor: modelos y temporales cuelgan de él.
    #[test]
    fn avi_cache_dir_relocates_the_container() {
        let base = std::env::temp_dir().join("avi-paths-cache");
        let _env = EnvSandbox::new(&base);
        let reloc = base.join("reloc_cache");
        std::env::set_var("AVI_CACHE_DIR", &reloc);
        let mut expected = default_tree(&base);
        expected[CACHE] = container(reloc.clone());
        expected[MODELS] = leaf(reloc.join("models"));
        expected[SCRATCH] = leaf(reloc.join("tmp"));
        assert_eq!(ownership_tree().unwrap(), expected);
        assert_eq!(cache_container().unwrap(), reloc);
    }

    /// `AVI_DATA_DIR` mueve solo la hoja de datos. Con las dos raíces
    /// reubicadas ya nada cuelga del contenedor raíz de Windows.
    #[test]
    fn avi_data_dir_relocates_the_data_leaf() {
        let base = std::env::temp_dir().join("avi-paths-data");
        let _env = EnvSandbox::new(&base);
        let reloc = base.join("reloc_data");
        std::env::set_var("AVI_DATA_DIR", &reloc);
        let mut expected = default_tree(&base);
        expected[DATA] = leaf(reloc.clone());
        assert_eq!(ownership_tree().unwrap(), expected);

        let reloc_cache = base.join("reloc_cache");
        std::env::set_var("AVI_CACHE_DIR", &reloc_cache);
        let both = ownership_tree().unwrap();
        assert_eq!(both[0].kind, NodeKind::Leaf);
        assert_eq!(both.len(), expected.len() - usize::from(cfg!(windows)));
        assert_eq!(both[0].path, reloc);
    }

    /// La raíz HF compartida es `External` y se añade al árbol sin sustituir la hoja de
    /// modelos; `AVI_CACHE_DIR` manda sobre ella y la deja fuera del árbol.
    #[test]
    fn shared_hf_root_is_external() {
        let base = std::env::temp_dir().join("avi-paths-hf");
        let _env = EnvSandbox::new(&base);
        let hub = base.join("hf").join("hub");
        std::env::set_var("HF_HUB_CACHE", &hub);
        let mut expected = default_tree(&base);
        expected.push(node(hub.clone(), NodeKind::External));
        assert_eq!(ownership_tree().unwrap(), expected);
        assert_eq!(models_cache_dir().unwrap(), hub);

        let reloc = base.join("reloc_cache");
        std::env::set_var("AVI_CACHE_DIR", &reloc);
        let tree = ownership_tree().unwrap();
        assert!(tree.iter().all(|n| n.kind != NodeKind::External));
        assert_eq!(models_cache_dir().unwrap(), reloc.join("models"));
    }

    /// Invocado por un symlink, el directorio del programa es el del fichero
    /// real, no el del enlace.
    #[cfg(unix)]
    #[test]
    fn real_parent_resolves_symlink_to_real_directory() {
        let root = tempfile::tempdir().expect("directorio temporal");
        let real_dir = root.path().join("opt");
        let link_dir = root.path().join("bin");
        std::fs::create_dir_all(&real_dir).expect("se crea opt");
        std::fs::create_dir_all(&link_dir).expect("se crea bin");
        let exe = real_dir.join("app");
        std::fs::write(&exe, b"").expect("se crea el ejecutable");
        let link = link_dir.join("app");
        std::os::unix::fs::symlink(&exe, &link).expect("se crea el symlink");
        let resolved = real_parent(&link).expect("se resuelve el symlink");
        assert_eq!(
            resolved,
            std::fs::canonicalize(&real_dir).expect("ruta real de opt")
        );
    }

    /// El directorio del programa de las pruebas existe y contiene su ejecutable.
    #[test]
    fn program_dir_contains_current_exe() {
        let dir = program_dir().expect("directorio del programa");
        let exe = std::env::current_exe().expect("ejecutable actual");
        let name = exe.file_name().expect("nombre del ejecutable");
        assert!(
            dir.join(name).is_file(),
            "{} no contiene {name:?}",
            dir.display()
        );
    }

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
