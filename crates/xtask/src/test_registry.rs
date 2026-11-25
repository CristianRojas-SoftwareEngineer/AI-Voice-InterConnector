// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! Claves de registro de las pruebas de ciclo de vida (solo Windows).
//!
//! Las pruebas crean claves directamente bajo `HKCU\Software` con el PID del proceso de
//! pruebas detrás del prefijo y las borran al terminar. Una ejecución interrumpida deja la
//! suya: este módulo encuentra las de PID muerto y las borra, para que `xtask clean` las
//! retire con el resto de los residuos de prueba. Fuera de Windows no hay nada equivalente.

use avi_shared::paths::TEST_REGISTRY_KEY_PREFIXES;

/// PID que lleva el nombre de una clave de prueba: los dígitos que siguen al prefijo, hasta
/// el siguiente `-`. Exige al menos un dígito y un `-` detrás; si no, `None`.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn test_key_pid(name: &str) -> Option<u32> {
    let rest = TEST_REGISTRY_KEY_PREFIXES
        .iter()
        .find_map(|prefix| name.strip_prefix(prefix))?;
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 || rest.as_bytes().get(digits) != Some(&b'-') {
        return None;
    }
    rest[..digits].parse().ok()
}

/// Claves de prueba huérfanas de `HKCU\Software` (`Software\<nombre>`): las de un PID que
/// ya no vive.
#[cfg(windows)]
pub(crate) fn orphan_test_keys() -> Vec<String> {
    use windows_sys::Win32::Foundation::{ERROR_NO_MORE_ITEMS, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegEnumKeyExW, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, KEY_READ,
    };

    let software: Vec<u16> = "Software\0".encode_utf16().collect();
    let mut hkey: HKEY = 0;
    // SAFETY: el puntero es a un `Vec<u16>` terminado en NUL vivo durante la llamada y
    // `hkey` es una variable local que recibe el handle.
    if unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, software.as_ptr(), 0, KEY_READ, &mut hkey) }
        != ERROR_SUCCESS
    {
        return Vec::new();
    }
    let mut keys = Vec::new();
    for index in 0u32.. {
        // El nombre de una subclave mide como mucho 255 caracteres.
        let mut buf = [0u16; 256];
        let mut len = buf.len() as u32;
        // SAFETY: `buf` y `len` son locales y `len` es su capacidad en caracteres; los
        // punteros opcionales nulos están permitidos.
        let code = unsafe {
            RegEnumKeyExW(
                hkey,
                index,
                buf.as_mut_ptr(),
                &mut len,
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if code == ERROR_NO_MORE_ITEMS {
            break;
        }
        if code != ERROR_SUCCESS {
            continue;
        }
        let name = String::from_utf16_lossy(&buf[..len as usize]);
        if test_key_pid(&name).is_some_and(|pid| !avi_process::pid_alive_exact(pid)) {
            keys.push(format!(r"Software\{name}"));
        }
    }
    // SAFETY: `hkey` se abrió arriba y no se usa después.
    unsafe { RegCloseKey(hkey) };
    keys
}

/// Fuera de Windows las pruebas no tocan nada equivalente.
#[cfg(not(windows))]
pub(crate) fn orphan_test_keys() -> Vec<String> {
    Vec::new()
}

/// Borra una clave de prueba con todo lo que cuelga de ella.
#[cfg(windows)]
pub(crate) fn delete_test_key(subkey: &str) -> std::io::Result<()> {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{RegDeleteTreeW, HKEY_CURRENT_USER};

    let wide: Vec<u16> = subkey.encode_utf16().chain(Some(0)).collect();
    // SAFETY: el puntero es a un `Vec<u16>` terminado en NUL vivo durante la llamada.
    let code = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, wide.as_ptr()) };
    if code == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(std::io::Error::from_raw_os_error(code as i32))
    }
}

/// Fuera de Windows no hay claves que borrar.
#[cfg(not(windows))]
pub(crate) fn delete_test_key(_subkey: &str) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_pid_accepts_both_prefixes_and_rejects_others() {
        assert_eq!(
            test_key_pid("AI-Voice-InterConnector-sandbox-1234-tag"),
            Some(1234)
        );
        assert_eq!(
            test_key_pid("AI-Voice-InterConnector-lifecycle-test-77-otra"),
            Some(77)
        );
        // Nombre ajeno, sin PID, con PID no numérico, sin `-` tras el PID o fuera de u32.
        assert_eq!(test_key_pid("Microsoft"), None);
        assert_eq!(test_key_pid("AI-Voice-InterConnector-sandbox--tag"), None);
        assert_eq!(
            test_key_pid("AI-Voice-InterConnector-sandbox-abc-tag"),
            None
        );
        assert_eq!(
            test_key_pid("AI-Voice-InterConnector-sandbox-12x-tag"),
            None
        );
        assert_eq!(test_key_pid("AI-Voice-InterConnector-sandbox-1234"), None);
        assert_eq!(
            test_key_pid("AI-Voice-InterConnector-sandbox-99999999999-tag"),
            None
        );
    }

    /// Borra la clave al soltarse, aunque la prueba falle.
    #[cfg(windows)]
    struct KeyGuard(&'static str);

    #[cfg(windows)]
    impl Drop for KeyGuard {
        fn drop(&mut self) {
            let _ = delete_test_key(self.0);
        }
    }

    #[cfg(windows)]
    #[test]
    fn orphan_key_is_listed_and_deleted() {
        use windows_sys::Win32::Foundation::ERROR_SUCCESS;
        use windows_sys::Win32::System::Registry::{
            RegCloseKey, RegCreateKeyExW, HKEY, HKEY_CURRENT_USER, KEY_WRITE,
            REG_OPTION_NON_VOLATILE,
        };

        // PID que no puede existir.
        const KEY: &str = r"Software\AI-Voice-InterConnector-sandbox-4294967000-xtask-test";
        let _guard = KeyGuard(KEY);
        let wide: Vec<u16> = KEY.encode_utf16().chain(Some(0)).collect();
        let mut hkey: HKEY = 0;
        // SAFETY: el puntero es a un `Vec<u16>` terminado en NUL vivo durante la llamada;
        // los punteros opcionales nulos están permitidos y `hkey` es local.
        let code = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                wide.as_ptr(),
                0,
                std::ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE,
                std::ptr::null(),
                &mut hkey,
                std::ptr::null_mut(),
            )
        };
        assert_eq!(code, ERROR_SUCCESS, "no se pudo crear la clave de prueba");
        // SAFETY: `hkey` se acaba de abrir y no se usa después.
        unsafe { RegCloseKey(hkey) };

        assert!(orphan_test_keys().iter().any(|k| k == KEY));
        delete_test_key(KEY).unwrap();
        assert!(!orphan_test_keys().iter().any(|k| k == KEY));
    }
}
