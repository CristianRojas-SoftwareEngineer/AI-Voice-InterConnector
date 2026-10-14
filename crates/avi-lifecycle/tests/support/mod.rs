// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! Arnés común de las pruebas de integración de `avi-lifecycle`.
//!
//! Cada archivo de `tests/` es un crate distinto que incluye este módulo con
//! `mod support;`, y un helper que algún crate no usa sería código muerto en él. Por
//! eso este módulo solo lleva lo que usan **todos** los crates (el sandbox base y
//! `write`), y lo demás vive en módulos hermanos que cada prueba declara con `#[path]`
//! según lo que necesita: `bundle`, `inert`, `inspect`, `install_env`, `listing`,
//! `net`, `no_terminal`, `process_env`, `registered`, `release_server`, `repo`,
//! `runtime` y `shared`. Cada módulo hermano solo contiene elementos que usan todas las
//! pruebas que lo declaran, y esa condición se comprueba compilando las pruebas con y
//! sin el feature `faults`, porque algunas pruebas solo existen con él.
//!
//! El aislamiento exige que la integración corra con las raíces reubicadas a
//! temporales. En Windows las Known Folders ignoran `LOCALAPPDATA`, así que un sandbox
//! que se apoyara en ellas no estaría probando el mecanismo de reubicación sino el de la
//! convención del sistema. El sandbox hace las dos cosas a la vez, que es lo que hace
//! falta para que la prueba valga:
//!
//! 1. **Pasa las rutas como dato** en `install::Env`, `cleanup::Roots` y
//!    `uninstall::Env`, que es como las reciben en producción.
//! 2. **Declara las variables de reubicación** (`AVI_INSTALL_DIR`, `AVI_BIN_DIR`,
//!    `AVI_DATA_DIR` y `AVI_CACHE_DIR`) apuntando al sandbox con `seed_env`, del módulo
//!    `process_env`, para que lo que el motor resuelve por el entorno —`ModelStore::new()`
//!    en los pasos 4 y 11 de la instalación— caiga dentro del sandbox y no en la máquina
//!    que ejecuta la puerta. Si las dos mitades divergieran, la prueba estaría afirmando
//!    el sandbox y no el motor.
//!
//! La raíz de modelos tiene dos modos, y la diferencia no es cosmética: `AVI_CACHE_DIR`
//! **tiene precedencia** sobre `HF_HUB_CACHE` y `HF_HOME` en `avi-store`, así que el
//! modo compartido se declara **quitando** `AVI_CACHE_DIR` y apuntando `HF_HUB_CACHE`.
//! Un sandbox que pusiera las dos cosas estaría probando la raíz exclusiva y declararía
//! `shared_models_dir` sobre una ruta que la regla de recursos compartidos nunca
//! protege.
//!
//! El prefijo del directorio del sandbox es deliberadamente neutro: el barrido de
//! temporales solo toca entradas con los prefijos propios (`avi-`, `avi_`), y un sandbox
//! llamado `avi-…` se confundiría con un temporal del producto.

#![allow(clippy::disallowed_methods)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// Variables de entorno de reubicación que el sandbox declara o borra según el modo.
const ROOT_VARS: [&str; 6] = [
    "AVI_INSTALL_DIR",
    "AVI_BIN_DIR",
    "AVI_DATA_DIR",
    "AVI_CACHE_DIR",
    "HF_HUB_CACHE",
    "HF_HOME",
];

/// Sandbox con las siete raíces dentro de un directorio propio.
pub struct Sandbox {
    /// Raíz del sandbox, para borrarla entera al terminar.
    pub root: PathBuf,
    /// Directorio de programa.
    pub program_dir: PathBuf,
    /// Etiqueta con la que se creó el sandbox. Nombra el staging hermano y identifica el
    /// sandbox en los avisos.
    pub tag: String,
    /// Directorio del enlace (`~/.local/bin`).
    pub bin_dir: PathBuf,
    pub home: PathBuf,
    /// Contenedor raíz de la aplicación: padre de la raíz de datos y del contenedor de
    /// caché.
    pub app_dir: PathBuf,
    pub data_dir: PathBuf,
    /// Contenedor de caché: es lo que registra el recibo y lo que `AVI_CACHE_DIR`
    /// nombra en modo exclusivo.
    pub cache_dir: PathBuf,
    /// Raíz de modelos. En modo exclusivo es `<cache_dir>/models`; en modo
    /// compartido es la caché HF que eligió el usuario.
    pub models_dir: PathBuf,
    /// `Some` cuando la raíz de modelos es la caché HF compartida del usuario.
    pub shared_hub: Option<PathBuf>,
    /// Raíz de los scratch por proceso: la hoja `tmp` del contenedor de caché.
    pub scratch_root: PathBuf,
    /// Directorio de temporales del sistema, ajeno al producto: aloja recursos de
    /// terceros que ninguna operación puede tocar.
    pub temp_root: PathBuf,
    /// Subclave de registro propia del test. Solo existe en Windows, que es donde el
    /// `PATH` del usuario vive en el registro.
    #[cfg(windows)]
    pub registry_subkey: String,
    /// `false` cuando el sandbox no es suyo y no debe borrarse al soltarlo: el proceso
    /// hijo sin terminal opera sobre el disco del padre y tiene que dejarlo intacto para
    /// que el padre pueda compararlo.
    pub remove_on_drop: bool,
    /// `true` cuando `seed_env` declaró las variables de reubicación: solo entonces se
    /// restauran al soltar el sandbox, para no tocar el entorno de pruebas que no lo
    /// usan.
    pub env_seeded: AtomicBool,
}

impl std::fmt::Debug for Sandbox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug = f.debug_struct("Sandbox");
        debug
            .field("tag", &self.tag)
            .field("root", &self.root)
            .field("program_dir", &self.program_dir)
            .field("bin_dir", &self.bin_dir)
            .field("home", &self.home)
            .field("app_dir", &self.app_dir)
            .field("data_dir", &self.data_dir)
            .field("cache_dir", &self.cache_dir)
            .field("models_dir", &self.models_dir)
            .field("shared_hub", &self.shared_hub)
            .field("scratch_root", &self.scratch_root)
            .field("temp_root", &self.temp_root);
        #[cfg(windows)]
        debug.field("registry_subkey", &self.registry_subkey);
        debug.finish()
    }
}

impl Sandbox {
    /// Levanta un sandbox nuevo con la raíz de modelos en modo exclusivo.
    pub fn new(tag: &str) -> Self {
        let sandbox = Self::at(tag, Self::unique_root(tag), false);
        sandbox.create_directories();
        sandbox
    }

    /// Raíz nueva y única bajo el directorio de temporales del sistema.
    pub fn unique_root(tag: &str) -> PathBuf {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or_default();
        std::env::temp_dir().join(format!("sandbox-{tag}-{}-{n}", std::process::id()))
    }

    /// Sandbox sobre una raíz dada, sin crear nada en disco. Deriva todas las rutas de
    /// la etiqueta y de la raíz: todo lo que depende del instante de creación vive en
    /// `root`, así que reconstruir un sandbox a partir de su raíz y su etiqueta es
    /// determinista. `shared` elige la caché HF compartida como raíz de modelos.
    pub fn at(tag: &str, root: PathBuf, shared: bool) -> Self {
        let opt = root.join("opt");
        let home = root.join("home").join("ana");
        let hub = root.join("hub");
        // Árbol de propiedad real: `<root>/app/{data,cache/{models,tmp}}`, con `app`
        // y `cache` como contenedores. En modo compartido los modelos son la caché HF
        // del usuario, fuera del árbol, y el contenedor de caché sigue siendo propio.
        let app_dir = root.join("app");
        let cache_dir = app_dir.join("cache");
        let (models_dir, shared_hub) = if shared {
            (hub.clone(), Some(hub))
        } else {
            (cache_dir.join("models"), None)
        };
        Self {
            tag: tag.to_string(),
            program_dir: opt.join(avi_lifecycle::APP_NAME),
            bin_dir: home.join(".local").join("bin"),
            data_dir: app_dir.join("data"),
            app_dir,
            scratch_root: cache_dir.join("tmp"),
            cache_dir,
            models_dir,
            shared_hub,
            temp_root: root.join("tmp"),
            home,
            #[cfg(windows)]
            registry_subkey: format!(
                r"Software\AI-Voice-InterConnector\sandbox-{}-{tag}",
                std::process::id()
            ),
            root,
            remove_on_drop: true,
            env_seeded: AtomicBool::new(false),
        }
    }

    /// Crea **solo** los directorios que existen en una máquina donde la aplicación no
    /// está instalada: el padre del programa, `$HOME`, el directorio del enlace y el de
    /// temporales.
    ///
    /// El directorio de programa, la raíz de datos y la raíz de modelos **no** se crean,
    /// y es deliberado: `uninstall::run` decide entre `not_installed` y una
    /// desinstalación vacía mirando si el estado existe, y un sandbox que llegara con esas
    /// raíces vacías afirmaría un caso que en producción no existe. Las pruebas que las
    /// necesitan las crean al escribir el estado o al instalar.
    ///
    /// Fija también `AVI_DAEMON_PORT` a `0`: `cleanup::run` detiene el daemon en ese
    /// puerto, y el `0` no apunta a ningún daemon real, así que ninguna prueba conecta
    /// con el del usuario. El valor es constante, así que escribirlo desde pruebas
    /// concurrentes no altera el resultado.
    pub fn create_directories(&self) {
        std::env::set_var("AVI_DAEMON_PORT", "0");
        for dir in [
            self.program_dir
                .parent()
                .expect("el directorio de programa tiene padre"),
            &self.home,
            &self.bin_dir,
            &self.temp_root,
        ] {
            std::fs::create_dir_all(dir).expect("se crea el sandbox");
        }
        #[cfg(windows)]
        avi_lifecycle::path_windows::create_key(&self.registry_subkey)
            .expect("se crea la clave de registro de prueba");
    }

    /// Borra la clave de registro de prueba, restaura el entorno y elimina el árbol del
    /// sandbox. Es idempotente, para que se pueda llamar a mano y también desde `Drop`.
    ///
    /// Un sandbox en préstamo (`remove_on_drop` a `false`) no borra su árbol: es el
    /// proceso padre quien lo creó y quien tiene que encontrarlo después.
    pub fn clear(&self) {
        #[cfg(windows)]
        {
            if let Err(e) = avi_lifecycle::path_windows::delete_key(&self.registry_subkey) {
                eprintln!(
                    "Aviso: sandbox {}: no se pudo borrar la clave de registro {}: {e}",
                    self.tag, self.registry_subkey
                );
            }
        }
        if self.remove_on_drop {
            if self.env_seeded.load(Ordering::Relaxed) {
                for variable in ROOT_VARS {
                    std::env::remove_var(variable);
                }
            }
            if let Err(e) = std::fs::remove_dir_all(&self.root) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    eprintln!("Aviso: no se pudo borrar {self:?}: {e}");
                }
            }
        }
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        self.clear();
    }
}

/// Escribe un fichero, creando los directorios intermedios.
pub fn write(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("se crea el directorio padre");
    }
    std::fs::write(path, content).expect("se escribe el fichero");
}
