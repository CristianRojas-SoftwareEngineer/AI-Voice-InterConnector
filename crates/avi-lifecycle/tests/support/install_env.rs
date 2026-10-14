// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! `Env` y opciones de `self install` sobre el sandbox. Lo declaran con
//! `#[path = "support/install_env.rs"] mod install_env;`; necesita también el módulo
//! `net`.

use crate::net::dead_port;
use crate::support::Sandbox;
use avi_lifecycle::install;
use std::path::Path;

impl Sandbox {
    /// Opciones de una instalación desatendida sin provisión de modelos.
    pub fn install_options() -> install::Options {
        install::Options {
            assume_yes: true,
            no_setup: true,
            no_modify_path: false,
            force: false,
        }
    }

    /// `Env` de instalación con las raíces del sandbox. `exe` decide el modo, así que
    /// es el parámetro que distingue instalar de reparar.
    pub fn install_env(&self, exe: &Path) -> install::Env {
        install::Env {
            exe: exe.to_path_buf(),
            version: "0.24.0".to_string(),
            target: avi_lifecycle::target::host_triple().to_string(),
            program_dir: self.program_dir.clone(),
            bin_dir: self.bin_dir.clone(),
            data_dir: self.data_dir.clone(),
            models_dir: self.models_dir.clone(),
            cache_dir: self.cache_dir.clone(),
            temp_root: self.temp_root.clone(),
            home: self.home.clone(),
            // Sin el directorio del enlace en el `PATH`: así el bloque de perfil sí se
            // escribe, que es la mitad de la integración del `PATH` en Unix.
            path_env: "/usr/bin:/bin".to_string(),
            shell: avi_lifecycle::path_unix::Shell::Bash,
            zdotdir: None,
            // Donde no hay registro, la subclave es la cadena vacía.
            #[cfg(windows)]
            registry_subkey: self.registry_subkey.clone(),
            #[cfg(not(windows))]
            registry_subkey: String::new(),
            // Estado de integración leído del sandbox al construir el entorno.
            #[cfg(windows)]
            registry_path: avi_lifecycle::path_windows::read_path(&self.registry_subkey)
                .expect("se lee la clave de registro de prueba"),
            #[cfg(unix)]
            link_state: avi_lifecycle::path_unix::classify_existing(
                &self.bin_dir.join("ai-voice-interconnector"),
                &self.program_dir.join(
                    avi_lifecycle::manifest::target_section(avi_lifecycle::target::host_triple())
                        .expect("el target del host tiene sección en el manifiesto")
                        .executable_path(),
                ),
            ),
            // Puerto donde no hay nada: la parada del daemon es un no-op.
            daemon_addr: dead_port(),
            source: None,
        }
    }
}
