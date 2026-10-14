// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! Nombres de los temporales y stagings de destino, con el token del propietario.
//!
//! Una escritura atómica prepara su contenido bajo un nombre provisional y lo
//! renombra al destino. Si el nombre provisional solo llevara el PID, un proceso
//! que reutilizara ese PID colisionaría con el residuo del anterior y un barrido
//! no podría distinguir un residuo huérfano de un temporal en uso. Por eso cada
//! familia lleva el [`OwnerToken`] del proceso que escribe: dos procesos
//! concurrentes nunca comparten nombre, y quien barre recupera al propietario del
//! nombre con el mismo parseo que lo produjo.
//!
//! Hay cuatro familias, cada una con su constructor (`name`) y su parser
//! (`owner_of`), que son inversos:
//!
//! - [`temp_file`]: fichero o enlace temporal hermano del destino,
//!   `<destino>.<token>.tmp`.
//! - [`voice_staging`]: directorio de preparación de una voz nueva,
//!   `.staging-<token>-<nombre>`.
//! - [`install_staging`]: directorio donde se descarga y extrae un bundle,
//!   hermano del directorio de programa,
//!   `.ai-voice-interconnector-staging-<token>-<etiqueta>`.
//! - [`parked`]: directorio donde una sustitución aparca el contenido anterior,
//!   dentro del directorio de programa, `.old-<token>-<único>`.
//!
//! Además, [`REMOVAL_FAILED_SUFFIX`] nombra el registro que el borrado diferido
//! deja junto a un destino que no pudo retirar.

use crate::paths::{PARKED_DIR_PREFIX, STAGING_DIR_PREFIX};
use crate::OwnerToken;
use std::path::{Path, PathBuf};

/// Sufijo del registro que el borrado diferido deja junto a un destino que no
/// pudo retirar: `<destino>.removal-failed.log`. No es un residuo de ninguna
/// familia: quien barre lo trata aparte y los parsers de las familias no deben
/// recibir estos nombres.
pub const REMOVAL_FAILED_SUFFIX: &str = ".removal-failed.log";

/// Separa un nombre `<token>-<resto>` en su propietario y el resto, que puede
/// contener guiones. `None` si el token es inválido o el resto está vacío.
fn split_token(text: &str) -> Option<(OwnerToken, &str)> {
    let mut fields = text.splitn(3, '-');
    let pid = fields.next()?;
    let start = fields.next()?;
    let rest = fields.next()?;
    if rest.is_empty() {
        return None;
    }
    Some((OwnerToken::parse(&format!("{pid}-{start}"))?, rest))
}

/// Fichero (o enlace) temporal hermano del destino: `<destino>.<token>.tmp`.
pub mod temp_file {
    use super::*;

    const SUFFIX: &str = ".tmp";

    /// Nombre del temporal del destino `dest_name` para el propietario `owner`.
    pub fn name(dest_name: &str, owner: &OwnerToken) -> String {
        format!("{dest_name}.{}{SUFFIX}", owner.format())
    }

    /// Ruta del temporal hermano de `dest`, en su mismo directorio. `None` si el
    /// destino no tiene nombre de fichero o este no es UTF-8.
    pub fn sibling(dest: &Path, owner: &OwnerToken) -> Option<PathBuf> {
        let dest_name = dest.file_name()?.to_str()?;
        Some(dest.with_file_name(name(dest_name, owner)))
    }

    /// Propietario de un nombre producido por [`name`]. `None` si el nombre no
    /// tiene esa forma: sin sufijo `.tmp`, sin destino, o con un token inválido.
    pub fn owner_of(file_name: &str) -> Option<OwnerToken> {
        let without_suffix = file_name.strip_suffix(SUFFIX)?;
        let (dest_name, token) = without_suffix.rsplit_once('.')?;
        if dest_name.is_empty() {
            return None;
        }
        OwnerToken::parse(token)
    }
}

/// Directorio de preparación de una voz nueva: `.staging-<token>-<nombre>`.
pub mod voice_staging {
    use super::*;

    const PREFIX: &str = ".staging-";

    /// Nombre del staging de la voz `voice` para el propietario `owner`.
    pub fn name(voice: &str, owner: &OwnerToken) -> String {
        format!("{PREFIX}{}-{voice}", owner.format())
    }

    /// Propietario de un nombre producido por [`name`]. `None` si el nombre no
    /// tiene esa forma: sin prefijo, con un token inválido o sin nombre de voz.
    pub fn owner_of(dir_name: &str) -> Option<OwnerToken> {
        // El token ocupa los dos primeros campos (`<pid>-<inicio>`); el resto,
        // que puede contener guiones, es el nombre de la voz.
        split_token(dir_name.strip_prefix(PREFIX)?).map(|(owner, _)| owner)
    }
}

/// Directorio de descarga y extracción de un bundle, hermano del directorio de
/// programa: `.ai-voice-interconnector-staging-<token>-<etiqueta>`.
pub mod install_staging {
    use super::*;

    /// Nombre del staging con la `label` dada (por ejemplo `update-0.24.0`) para
    /// el propietario `owner`.
    pub fn name(label: &str, owner: &OwnerToken) -> String {
        format!("{STAGING_DIR_PREFIX}{}-{label}", owner.format())
    }

    /// Propietario de un nombre producido por [`name`]. `None` si el nombre no
    /// tiene esa forma: sin prefijo, con un token inválido o sin etiqueta.
    pub fn owner_of(dir_name: &str) -> Option<OwnerToken> {
        split_token(dir_name.strip_prefix(STAGING_DIR_PREFIX)?).map(|(owner, _)| owner)
    }
}

/// Directorio de aparcado de una sustitución, dentro del directorio de
/// programa: `.old-<token>-<único>`, con `<único>` en dígitos decimales.
pub mod parked {
    use super::*;

    /// Nombre del aparcado de la sustitución `unique` del propietario `owner`.
    pub fn name(owner: &OwnerToken, unique: u128) -> String {
        format!("{PARKED_DIR_PREFIX}{}-{unique}", owner.format())
    }

    /// Propietario de un nombre producido por [`name`]. `None` si el nombre no
    /// tiene esa forma: sin prefijo, con un token inválido o con un sufijo que
    /// no son solo dígitos.
    pub fn owner_of(dir_name: &str) -> Option<OwnerToken> {
        let (owner, unique) = split_token(dir_name.strip_prefix(PARKED_DIR_PREFIX)?)?;
        unique.bytes().all(|b| b.is_ascii_digit()).then_some(owner)
    }
}

#[cfg(test)]
mod tests {
    use super::{install_staging, parked, temp_file, voice_staging};
    use crate::OwnerToken;
    use std::path::Path;

    fn token(pid: u32, start: &str) -> OwnerToken {
        OwnerToken::new(pid, start).expect("token válido")
    }

    /// El nombre del temporal lleva el token y el parser lo recupera.
    #[test]
    fn temp_file_name_roundtrips() {
        let owner = token(4242, "abc123");
        let name = temp_file::name("reference.qvoice", &owner);
        assert_eq!(name, "reference.qvoice.4242-abc123.tmp");
        assert_eq!(temp_file::owner_of(&name), Some(owner));
    }

    /// El temporal hermano queda en el mismo directorio que el destino.
    #[test]
    fn temp_file_sibling_stays_in_directory() {
        let owner = token(7, "z9");
        let sibling = temp_file::sibling(Path::new("dir/daemon.pid"), &owner).unwrap();
        assert_eq!(sibling, Path::new("dir/daemon.pid.7-z9.tmp"));
        assert_eq!(temp_file::sibling(Path::new("/"), &owner), None);
    }

    /// Lo que no es un temporal con token se rechaza, incluidos los nombres
    /// antiguos con nombre fijo.
    #[test]
    fn temp_file_owner_rejects_other_names() {
        for bad in [
            "",
            ".tmp",
            "reference.qvoice.tmp",
            "daemon.pid.tmp",
            "a.wav.tmp",
            "a.wav.4242.tmp",
            "a.wav.4242-ABC.tmp",
            ".4242-abc.tmp",
            "a.wav.4242-abc",
            "a.wav.4242-abc.tmp.bak",
            "a.wav.04242-abc.tmp",
        ] {
            assert_eq!(temp_file::owner_of(bad), None, "debía rechazarse: {bad:?}");
        }
    }

    /// Dos propietarios distintos nunca comparten nombre de temporal, ni
    /// siquiera con el mismo PID.
    #[test]
    fn temp_file_names_do_not_collide_between_owners() {
        let first = token(100, "aaa");
        let reused_pid = token(100, "bbb");
        let other_pid = token(101, "aaa");
        let names = [
            temp_file::name("x", &first),
            temp_file::name("x", &reused_pid),
            temp_file::name("x", &other_pid),
        ];
        assert_ne!(names[0], names[1]);
        assert_ne!(names[0], names[2]);
        assert_ne!(names[1], names[2]);
    }

    /// El nombre del staging lleva el token y el parser lo recupera, también
    /// con guiones en el nombre de la voz.
    #[test]
    fn voice_staging_name_roundtrips() {
        let owner = token(4242, "abc123");
        for voice in ["ana", "ana-maria", "a-1-b", "42-x"] {
            let name = voice_staging::name(voice, &owner);
            assert_eq!(name, format!(".staging-4242-abc123-{voice}"));
            assert_eq!(voice_staging::owner_of(&name), Some(owner.clone()));
        }
    }

    /// Lo que no es un staging con token se rechaza, incluido el nombre antiguo.
    #[test]
    fn voice_staging_owner_rejects_other_names() {
        for bad in [
            "",
            ".staging-",
            ".staging-4242-abc",
            ".staging-4242-abc-",
            ".staging-4242-ABC-ana",
            ".staging-04242-abc-ana",
            ".staging-ana",
            "staging-4242-abc-ana",
            "avi-voice-staging-ana-4242",
        ] {
            assert_eq!(
                voice_staging::owner_of(bad),
                None,
                "debía rechazarse: {bad:?}"
            );
        }
    }

    /// Dos propietarios distintos nunca comparten nombre de staging.
    #[test]
    fn voice_staging_names_do_not_collide_between_owners() {
        let first = voice_staging::name("ana", &token(100, "aaa"));
        let reused_pid = voice_staging::name("ana", &token(100, "bbb"));
        assert_ne!(first, reused_pid);
    }

    /// El staging de instalación lleva el token y el parser lo recupera, también
    /// con guiones y puntos en la etiqueta.
    #[test]
    fn install_staging_name_roundtrips() {
        let owner = token(4242, "abc123");
        let name = install_staging::name("update-0.24.0", &owner);
        assert_eq!(
            name,
            ".ai-voice-interconnector-staging-4242-abc123-update-0.24.0"
        );
        assert_eq!(install_staging::owner_of(&name), Some(owner));
    }

    /// Los nombres sin token (los de los instaladores de arranque) y los
    /// registros de borrado fallido no son un staging con propietario.
    #[test]
    fn install_staging_owner_rejects_other_names() {
        for bad in [
            "",
            ".ai-voice-interconnector-staging-",
            ".ai-voice-interconnector-staging-aB3dE9",
            ".ai-voice-interconnector-staging-0123456789abcdef0123456789abcdef",
            ".ai-voice-interconnector-staging-4242-abc-",
            ".ai-voice-interconnector-staging-4242-ABC-x",
            "ai-voice-interconnector-staging-4242-abc-x",
        ] {
            assert_eq!(
                install_staging::owner_of(bad),
                None,
                "debía rechazarse: {bad:?}"
            );
        }
    }

    /// El aparcado lleva el token y el parser lo recupera; un sufijo que no son
    /// dígitos (como el de un registro de borrado fallido) se rechaza.
    #[test]
    fn parked_name_roundtrips_and_rejects_other_names() {
        let owner = token(4242, "abc123");
        let name = parked::name(&owner, 1780000000000);
        assert_eq!(name, ".old-4242-abc123-1780000000000");
        assert_eq!(parked::owner_of(&name), Some(owner));
        for bad in [
            "",
            ".old-",
            ".old-1780000000000",
            ".old-4242-abc",
            ".old-4242-abc-",
            ".old-4242-abc-12x",
            ".old-4242-abc-12.removal-failed.log",
            ".old-04242-abc-12",
            "old-4242-abc-12",
        ] {
            assert_eq!(parked::owner_of(bad), None, "debía rechazarse: {bad:?}");
        }
    }
}
