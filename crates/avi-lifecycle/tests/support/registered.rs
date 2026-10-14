// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! Instalación registrada y estado de usuario del sandbox, para las pruebas de
//! desinstalación y limpieza. Lo declaran con
//! `#[path = "support/registered.rs"] mod registered;`; necesita también el módulo
//! `repo`.

use crate::repo::repo_dir;
use crate::support::{write, Sandbox};
use avi_lifecycle::channel::Channel;
use avi_lifecycle::cleanup;
use avi_lifecycle::receipt::{self, InstallReceipt, PathIntegration};
use avi_lifecycle::uninstall;
use avi_process::registry::{write_pid_file, PidFile};
use std::path::Path;

/// Removedor que borra ya. El caso diferido de Windows es el del paso 8 de la
/// desinstalación, y no es lo que se ejercita aquí.
pub struct Now;

impl uninstall::ProgramDirRemover for Now {
    fn exe_lives_inside(&self, _program_dir: &Path) -> anyhow::Result<bool> {
        Ok(false)
    }
    fn remove_now(&self, program_dir: &Path) -> anyhow::Result<()> {
        std::fs::remove_dir_all(program_dir)?;
        Ok(())
    }
    fn schedule(&self, _program_dir: &Path, _wait: &avi_process::OwnerToken) -> anyhow::Result<()> {
        Ok(())
    }
}

impl Sandbox {
    /// `Roots` de `cleanup` y de `self uninstall`.
    pub fn roots(&self) -> cleanup::Roots {
        cleanup::Roots {
            program_dir: self.program_dir.clone(),
            data_dir: self.data_dir.clone(),
            cache_container: self.cache_dir.clone(),
            app_container: Some(self.app_dir.clone()),
            models_dir: self.models_dir.clone(),
            scratch_root: self.scratch_root.clone(),
            bin_dir: self.bin_dir.clone(),
            home: self.home.clone(),
            models_shared: self.shared_hub.is_some(),
        }
    }

    /// `Env` de desinstalación. El directorio de programa lo da el recibo si lo hay,
    /// que es la regla: la operación actúa sobre la instalación registrada, no sobre la
    /// posición del ejecutable.
    pub fn env_uninstall<'a>(
        &self,
        receipt: Option<&'a InstallReceipt>,
        channel: Channel,
    ) -> uninstall::Env<'a> {
        uninstall::Env {
            roots: self.roots(),
            program_dir: receipt
                .map(|r| r.install_dir.clone())
                .unwrap_or_else(|| self.program_dir.clone()),
            receipt,
            channel,
            daemon_addr: "127.0.0.1:0".to_string(),
            home: self.home.clone(),
        }
    }

    /// Escribe el estado completo y, además, los recursos **compartidos** del entorno
    /// que ninguna operación puede borrar: un repo de otra herramienta dentro de la
    /// caché de modelos, `~/.cargo` y el directorio de `sccache`.
    ///
    /// Los locks se escriben dos veces a propósito: el del repo propio, que es atribuible
    /// a la aplicación, y el de otro, que no. La distinción es lo que se comprueba, y sin
    /// los dos directorios no se puede ver.
    pub fn seed_state(&self) {
        let hub = &self.models_dir;
        for pin in avi_shared::paths::MODEL_REVISIONS {
            // Snapshot propio, con el layout que `avi-store` resuelve.
            write(
                &hub.join(repo_dir(pin.repo))
                    .join("snapshots")
                    .join(pin.revision)
                    .join("weights.bin"),
                "pesos",
            );
            // Y su lock, atribuible a la aplicación.
            write(
                &hub.join(".locks").join(repo_dir(pin.repo)).join("lock"),
                "",
            );
        }
        // `xet` y el `.locks` completo: globales de la caché, nunca nuestros.
        write(&hub.join("xet").join("shard"), "xet");
        write(
            &hub.join(".locks").join("models--other--tool").join("lock"),
            "",
        );
        // Y el repo de otra herramienta, con su lock propio.
        write(
            &hub.join("models--other--tool").join("other.safetensors"),
            "ajeno",
        );
        // Estado de usuario: voces de fábrica y de usuario, habla, configuración, logs
        // y pidfile.
        for voice in ["default", "ryan", "mine"] {
            write(
                &self
                    .data_dir
                    .join("voices")
                    .join(voice)
                    .join("reference.qvoice"),
                "voice",
            );
        }
        for voice in ["default", "mine"] {
            write(
                &self.data_dir.join("speech").join(voice).join("hello.wav"),
                "wav",
            );
        }
        write(&self.data_dir.join("config.json"), "{}");
        write(&self.data_dir.join("logs").join("daemon.log"), "log");
        // Pidfile de un daemon que ya no vive: el esquema completo, con las claves de
        // identidad presentes y vacías.
        write_pid_file(
            &self.data_dir,
            &PidFile {
                pid: 999_999,
                addr: None,
                started_at: None,
                resident_pid: 0,
                identity: None,
                resident_identity: None,
                resident_port: None,
            },
        )
        .expect("se escribe el pidfile");
        // Recursos compartidos del entorno, que ninguna operación puede borrar.
        // `sccache` se escribe en dos sitios porque son los dos que existen en
        // producción: la caché del usuario y el temporal del sistema.
        write(
            &self.home.join(".cargo").join("registry").join("index"),
            "carga",
        );
        write(
            &self.home.join(".cache").join("sccache").join("object"),
            "sccache",
        );
        write(&self.temp_root.join("sccache").join("object"), "sccache");
    }

    /// Recibo de una instalación registrada, con las raíces del sandbox.
    pub fn receipt(&self, integration: PathIntegration) -> InstallReceipt {
        InstallReceipt::new(
            "0.24.0",
            avi_lifecycle::target::host_triple(),
            Channel::Script,
            &self.program_dir,
            vec![uninstall::executable_name_default()],
            integration,
            receipt::Roots {
                data_dir: self.data_dir.clone(),
                cache_dir: self.cache_dir.clone(),
            },
            None,
        )
    }

    /// Instala sin pasar por `self install`: escribe el ejecutable y el recibo.
    /// Es lo que necesitan las pruebas de desinstalación y limpieza, cuyo objeto es la
    /// desinstalación y no la colocación.
    pub fn install_registered(&self, integration: PathIntegration) -> InstallReceipt {
        let receipt = self.receipt(integration);
        write(
            &self.program_dir.join(uninstall::executable_name_default()),
            "binario",
        );
        receipt::write_to(&receipt, &self.program_dir).expect("se escribe el recibo");
        receipt
    }
}
