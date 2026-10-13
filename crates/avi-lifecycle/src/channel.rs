// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! Canal de una copia del binario: por dónde se instaló y quién la gestiona.
//!
//! El canal decide qué puede hacer cada comando sobre una instalación. Los
//! canales son `script` (instalación desde un archivo publicado o extraído a
//! mano, también la que deja `cargo xtask install`), `homebrew` (una copia del
//! Cask, que no se actualiza ni se desinstala desde aquí sino con `brew`) y
//! `unmanaged` (un binario ejecutado fuera de una instalación).
//!
//! **Precedencia**: `homebrew` si el ejecutable resuelto está bajo el
//! prefijo de Homebrew; si no, el canal que declara el recibo; si no hay recibo,
//! `unmanaged`. Homebrew gana sobre el recibo porque el Cask no deja recibo —lo
//! gestiona otra herramienta— y porque una copia del Cask ejecutándose dentro de
//! otra instalación seguiría siendo de Homebrew: es el prefijo el que manda, no
//! el papel del directorio.
//!
//! Aparte del canal, `self update` y `self uninstall` operan siempre sobre la
//! **instalación registrada** (el recibo en su ubicación), sea cual sea la copia
//! del binario que ejecute el comando; eso es [`registered_install_dir`].

use crate::receipt::InstallReceipt;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Componente de ruta que delata una instalación gestionada por Homebrew. Se
/// nombra como criterio, no como prefijo fijo, porque el prefijo cambia
/// entre `/opt/homebrew` y `/usr/local` según la arquitectura.
const HOMEBREW_MARKER: &str = "Caskroom";

/// Vía por la que se instaló la copia.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    /// Bootstrap o `self install` desde un archivo extraído a mano.
    Script,
    /// Cask de Homebrew, detectado sin recibo.
    Homebrew,
    /// Binario ejecutado fuera de una instalación: bundle sin instalar,
    /// `target/`.
    Unmanaged,
}

impl Channel {
    /// Nombre de contrato del canal, el mismo que va al recibo y al sobre.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Script => "script",
            Self::Homebrew => "homebrew",
            Self::Unmanaged => "unmanaged",
        }
    }

    /// Lee el nombre de un canal; `None` si no es ninguno de los tres.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "script" => Some(Self::Script),
            "homebrew" => Some(Self::Homebrew),
            "unmanaged" => Some(Self::Unmanaged),
            _ => None,
        }
    }
}

impl std::fmt::Display for Channel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// `true` si `exe` está bajo el prefijo de Homebrew.
///
/// Se busca `Caskroom` como **componente** de la ruta, no como subcadena: una
/// instalación en `/opt/homebrew/bin/ai-voice-interconnector` no es un Cask del
/// proyecto y no debe quedar marcada como `homebrew`.
pub fn is_homebrew_path(exe: &Path) -> bool {
    exe.components().any(|component| {
        component
            .as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case(HOMEBREW_MARKER)
    })
}

/// Canal de la copia que se está ejecutando, con la precedencia de la tabla:
/// Homebrew sobre el recibo, el recibo sobre `unmanaged`.
pub fn detect(exe: &Path, receipt: Option<&InstallReceipt>) -> Channel {
    if is_homebrew_path(exe) {
        return Channel::Homebrew;
    }
    match receipt {
        Some(receipt) => receipt.channel,
        None => Channel::Unmanaged,
    }
}

/// Directorio de programa sobre el que un comando de ciclo de vida opera: el
/// de la instalación registrada si hay recibo, y el de la convención si no lo
/// hay. Que sea independiente de la copia que ejecuta el comando es lo
/// que permite que `self uninstall` quite la instalación aunque se invoque desde
/// `target/`.
pub fn registered_install_dir(receipt: Option<&InstallReceipt>) -> std::io::Result<PathBuf> {
    match receipt {
        Some(receipt) => Ok(receipt.install_dir.clone()),
        None => crate::install_dir(),
    }
}

/// Directorio de programa registrado ahora mismo: el del recibo que hay en el
/// directorio de la convención de rutas, o ese mismo si no hay recibo.
/// Es la única forma de resolverlo desde el entorno, y de ella salen la ruta del
/// bloqueo de ciclo de vida y la de todas las operaciones, para que ninguna tome
/// el bloqueo de un sitio y opere sobre otro.
///
/// Un recibo presente pero ilegible es un error que nombra el fichero: no se
/// confunde con la ausencia de recibo.
pub fn registered_program_dir() -> anyhow::Result<PathBuf> {
    let receipt = crate::receipt::read_from(&crate::install_dir()?)?;
    Ok(registered_install_dir(receipt.as_ref())?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::receipt::PathIntegration;

    fn receipt(channel: Channel, install_dir: &str) -> InstallReceipt {
        InstallReceipt::new(
            "0.24.0",
            "x86_64-unknown-linux-gnu",
            channel,
            Path::new(install_dir),
            vec!["ai-voice-interconnector".to_string()],
            PathIntegration::none(),
            crate::receipt::Roots {
                data_dir: PathBuf::from("/home/ana/data"),
                cache_dir: PathBuf::from("/home/ana/cache"),
            },
            None,
        )
    }

    /// Los tres canales, con la precedencia de `Caskroom` sobre la existencia
    /// de recibo y el caso de un ejecutable de `target/` sin instalación
    /// registrada.
    #[test]
    fn channel_detection_covers_all_three() {
        let script = receipt(
            Channel::Script,
            "/home/ana/.local/opt/ai-voice-interconnector",
        );

        // 1. `script`: recibo y ejecutable fuera de Homebrew.
        assert_eq!(
            detect(
                Path::new("/home/ana/.local/opt/ai-voice-interconnector/ai-voice-interconnector"),
                Some(&script)
            ),
            Channel::Script
        );

        // 2. `homebrew` gana sobre el recibo: el Cask no deja recibo, y si se
        //    encuentra uno la copia sigue siendo de Homebrew.
        assert_eq!(
            detect(
                Path::new(
                    "/opt/homebrew/Caskroom/ai-voice-interconnector/0.24.0/ai-voice-interconnector"
                ),
                Some(&script)
            ),
            Channel::Homebrew,
            "Caskroom precede al recibo"
        );
        assert_eq!(
            detect(
                Path::new("/usr/local/Caskroom/ai-voice-interconnector/ai-voice-interconnector"),
                None
            ),
            Channel::Homebrew
        );
        // Un prefijo de Homebrew que no es un Cask del proyecto no es un Cask.
        assert_eq!(
            detect(
                Path::new("/opt/homebrew/bin/ai-voice-interconnector"),
                Some(&script)
            ),
            Channel::Script,
            "Cellar no es Caskroom"
        );

        // 3. `unmanaged`: binario ejecutado fuera de una instalación.
        assert_eq!(
            detect(
                Path::new(
                    "/home/ana/src/AI-Voice-InterConnector/target/release/ai-voice-interconnector"
                ),
                None
            ),
            Channel::Unmanaged
        );
        assert_eq!(
            detect(
                Path::new("C:\\src\\target\\debug\\ai-voice-interconnector.exe"),
                None
            ),
            Channel::Unmanaged
        );

        // Nombres de contrato y su ida y vuelta.
        for channel in [Channel::Script, Channel::Homebrew, Channel::Unmanaged] {
            assert_eq!(Channel::from_name(channel.as_str()), Some(channel));
            assert_eq!(channel.to_string(), channel.as_str());
        }
        assert_eq!(Channel::from_name("snap"), None, "canal desconocido");
        assert_eq!(Channel::from_name("dev"), None, "`dev` no es un canal");

        // Precedencia entre canal e instalación registrada: un binario `unmanaged`
        // sigue operando sobre la instalación que declara el recibo.
        assert_eq!(
            registered_install_dir(Some(&script)).unwrap(),
            PathBuf::from("/home/ana/.local/opt/ai-voice-interconnector")
        );
        assert_eq!(
            registered_install_dir(None).unwrap(),
            crate::install_dir().unwrap()
        );
    }
}
