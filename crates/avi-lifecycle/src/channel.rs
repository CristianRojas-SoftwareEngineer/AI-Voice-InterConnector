//! Canal de una copia del binario: por dónde se instaló y quién la gestiona
//! (§8.2).
//!
//! El canal decide qué puede hacer cada comando sobre una instalación. Dos
//! casos de la tabla de §8.2 se resuelven con esto: una copia de Homebrew no se
//! actualiza ni se desinstala desde aquí sino con `brew`, y una instalación de
//! desarrollo (`cargo xtask install`) no se actualiza por el canal publicado.
//!
//! **Precedencia** (§8.2): `homebrew` si el ejecutable resuelto está bajo el
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

/// Componente de ruta que delata una instalación gestionada por Homebrew. §8.2
/// lo nombra como criterio, no como prefijo fijo, porque el prefijo cambia
/// entre `/opt/homebrew` y `/usr/local` según la arquitectura.
const HOMEBREW_MARKER: &str = "Caskroom";

/// Vía por la que se instaló la copia (§8.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    /// Bootstrap o `self install` desde un archivo extraído a mano.
    Script,
    /// `cargo xtask install`, en el canal de desarrollo.
    Dev,
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
            Self::Dev => "dev",
            Self::Homebrew => "homebrew",
            Self::Unmanaged => "unmanaged",
        }
    }

    /// Lee el nombre de un canal; `None` si no es ninguno de los cuatro.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "script" => Some(Self::Script),
            "dev" => Some(Self::Dev),
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

/// Canal de la copia que se está ejecutando, con la precedencia de §8.2:
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
/// hay (§8.2). Que sea independiente de la copia que ejecuta el comando es lo
/// que permite que `self uninstall` quite la instalación aunque se invoque desde
/// `target/`.
pub fn registered_install_dir(receipt: Option<&InstallReceipt>) -> PathBuf {
    match receipt {
        Some(receipt) => receipt.install_dir.clone(),
        None => crate::install_dir(),
    }
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

    /// Los cuatro canales, con la precedencia de `Caskroom` sobre la existencia
    /// de recibo y el caso de un ejecutable de `target/` sin instalación
    /// registrada.
    #[test]
    fn channel_detection_covers_all_four() {
        let script = receipt(
            Channel::Script,
            "/home/ana/.local/opt/ai-voice-interconnector",
        );
        let dev = receipt(Channel::Dev, "/home/ana/.local/opt/ai-voice-interconnector");

        // 1. `script`: recibo y ejecutable fuera de Homebrew.
        assert_eq!(
            detect(
                Path::new("/home/ana/.local/opt/ai-voice-interconnector/ai-voice-interconnector"),
                Some(&script)
            ),
            Channel::Script
        );

        // 2. `dev`: mismo criterio, el canal lo declara el recibo.
        assert_eq!(
            detect(
                Path::new("/home/ana/.local/opt/ai-voice-interconnector/ai-voice-interconnector"),
                Some(&dev)
            ),
            Channel::Dev
        );

        // 3. `homebrew` gana sobre el recibo: el Cask no deja recibo, y si se
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

        // 4. `unmanaged`: binario ejecutado fuera de una instalación.
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
        for channel in [
            Channel::Script,
            Channel::Dev,
            Channel::Homebrew,
            Channel::Unmanaged,
        ] {
            assert_eq!(Channel::from_name(channel.as_str()), Some(channel));
            assert_eq!(channel.to_string(), channel.as_str());
        }
        assert_eq!(Channel::from_name("snap"), None, "canal desconocido");

        // Precedencia entre canal e instalación registrada: un binario `unmanaged`
        // sigue operando sobre la instalación que declara el recibo.
        assert_eq!(
            registered_install_dir(Some(&script)),
            PathBuf::from("/home/ana/.local/opt/ai-voice-interconnector")
        );
        assert_eq!(registered_install_dir(None), crate::install_dir());
    }
}
