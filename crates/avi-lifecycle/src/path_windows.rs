//! Integración de `PATH` en Windows: la entrada del directorio de programa en el
//! valor `Path` de `HKCU\Environment`.
//!
//! Aquí se elimina el defecto que el código anterior tenía, y el enunciado lo nombra
//! uno por uno:
//!
//! - **Se lee sin expandir.** `RegGetValueW` con `RRF_NOEXPAND` devuelve el valor
//!   tal como el usuario lo escribió. Leer expandido y volver a escribir es lo que
//!   aplana las entradas `%VAR%` y destruye información suya.
//! - **Se conserva el tipo.** Un `Path` que era `REG_EXPAND_SZ` sigue siéndolo, y
//!   uno que no existía se crea como `REG_EXPAND_SZ`. Escribir siempre `REG_SZ`
//!   era lo que hacía el código anterior.
//! - **La comparación es canónica**: sin distinguir mayúsculas, ignorando
//!   separadores finales y considerando también la forma expandida de cada
//!   entrada, de modo que una entrada escrita con `%USERPROFILE%\...` se reconoce
//!   y no se duplica.
//! - **Se difunde `WM_SETTINGCHANGE`** con tiempo límite, para que las
//!   exploradoras nuevas y los procesos que ya estaban abiertos vean el valor
//!   nuevo.
//!
//! **La ruta de la clave es un parámetro**: la prueba de integración
//! opera sobre una clave propia del test en lugar de la del usuario, y así el
//! criterio 7 se demuestra sin tocar el entorno de quien ejecuta.
//!
//! **La decisión está separada del acceso al registro.** `plan_integrate` y
//! `plan_revert` son funciones puras sobre el valor leído, de modo que la regla de
//! idempotencia y la de conservación del tipo se pueden afirmar sin escribir nada
//! en el registro del usuario.

use crate::LifecycleError;
use std::path::Path;

/// Subclave de `HKCU` donde vive el valor `Path` del usuario.
pub const ENV_SUBKEY: &str = "Environment";

/// Nombre del valor dentro de la subclave.
pub const PATH_VALUE: &str = "Path";

/// `SMTO_ABORTIFHUNG`: no esperar a una ventana colgada.
const SMTO_ABORTIFHUNG: u32 = 0x0002;

/// Tiempo límite de `WM_SETTINGCHANGE`, en milisegundos. El valor anterior usaba
/// 5000 y se conserva.
const BROADCAST_TIMEOUT_MS: u32 = 5000;

/// El valor `Path` tal como está en el registro: el texto **sin expandir** y su
/// tipo, que hay que conservar al escribir.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawPath {
    /// Valor tal cual, con las entradas `%VAR%` intactas.
    pub value: String,
    /// `REG_SZ` o `REG_EXPAND_SZ`, tal como se leyó.
    pub kind: u32,
}

/// Qué se escribiría al integrar o revertir, y si hace falta escribir.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// Texto completo a escribir.
    pub value: String,
    /// Tipo con el que se escribe: el leído, o `REG_EXPAND_SZ` si no existía.
    pub kind: u32,
    /// `false` cuando el valor ya era el que se iba a escribir y tocarlo sería
    /// reescribir el registro sin necesidad. Es la idempotencia del criterio 2.
    pub changed: bool,
}

/// Lectura del valor `Path` de `subkey` sin expandir. `Ok(None)` si la clave o el
/// valor no existen: es el caso en que se crea con `REG_EXPAND_SZ`.
pub fn read_path(subkey: &str) -> anyhow::Result<Option<RawPath>> {
    use windows_sys::Win32::Foundation::ERROR_FILE_NOT_FOUND;
    use windows_sys::Win32::System::Registry::{
        RegGetValueW, RRF_NOEXPAND, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ,
    };

    // La máscara acepta los dos tipos de cadena, porque el valor del usuario puede
    // ser de cualquiera de los dos y la regla manda conservarlo, no normalizarlo.
    const MASK: u32 = RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ | RRF_NOEXPAND;
    let (subkey_w, value_w) = (wide(subkey), wide(PATH_VALUE));

    let mut kind: u32 = 0;
    let mut bytes: u32 = 0;
    // SAFETY: `subkey_w` y `value_w` son punteros a `Vec<u16>` terminados en NUL
    // que viven hasta el final de la llamada. `pvdata` nulo con `pcbdata` no nulo
    // es la forma documentada de pedir solo tamaño y tipo.
    let code = unsafe {
        RegGetValueW(
            windows_sys::Win32::System::Registry::HKEY_CURRENT_USER,
            subkey_w.as_ptr(),
            value_w.as_ptr(),
            MASK,
            &mut kind,
            std::ptr::null_mut(),
            &mut bytes,
        )
    };
    if code == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    check(code, "leer")?;

    // La primera pasada prometió `bytes`; se reserva de más para el NUL y se
    // recorta al primer NUL, que es donde acaba el valor de cadena del registro.
    let mut buffer =
        vec![0u16; usize::try_from(bytes).unwrap_or(0) / std::mem::size_of::<u16>() + 1];
    let mut capacity = u32::try_from(buffer.len() * std::mem::size_of::<u16>()).unwrap_or(u32::MAX);
    // SAFETY: `buffer` tiene capacidad para `capacity` bytes, y la máscara lleva
    // `RRF_NOEXPAND`, que es lo que devuelve el texto tal como el usuario lo escribió.
    let code = unsafe {
        RegGetValueW(
            windows_sys::Win32::System::Registry::HKEY_CURRENT_USER,
            subkey_w.as_ptr(),
            value_w.as_ptr(),
            MASK,
            &mut kind,
            buffer.as_mut_ptr().cast(),
            &mut capacity,
        )
    };
    check(code, "leer")?;
    let utf16: Vec<u16> = buffer.into_iter().take_while(|c| *c != 0).collect();
    Ok(Some(RawPath {
        value: String::from_utf16_lossy(&utf16),
        kind,
    }))
}

/// Plan de integración de `entry`: se añade al final **solo si falta**.
///
/// La comparación es la canónica de `avi-store`, que también considera la forma
/// expandida de cada entrada: por eso una entrada escrita con `%USERPROFILE%\bin`
/// no se duplica cuando se busca la ruta ya expandida, y al revés.
///
/// **El valor que se lee para comparar y el que se escribe son la misma cadena.**
/// `canonical_path_key` —que es la que normaliza y quita el prefijo verbatim
/// `\\?\`— se usa **solo para comparar**: lo que se escribe es
/// `entry.display()` tal cual, sin transformar. La asimetría que eso deja es
/// deliberada y no molesta:
///
/// - Si el `Path` del usuario trae la entrada **con** prefijo y la nuestra no, la
///   comparación dice que ya está y **no** se añade una duplicada. En la reversión
///   la misma comparación la quita. Es el desenlace correcto en las dos direcciones.
/// - Si el usuario guardó la entrada en la forma normal y la nuestra llegara
///   verbatim, pasa lo mismo: se reconoce y no se duplica.
///
/// Lo que no puede pasar es que el motor escriba una ruta que luego no sepa
/// encontrar para revertir, y no puede porque **la comparación de la reversión es
/// la misma función**: `plan_revert` filtra con `canonical_path_entry_matches` y
/// con eso elimina tanto la forma verbatim como la normal, indistintamente.
///
/// Y en la práctica el motor nunca escribe una ruta verbatim: `entry` viene de
/// `bin_dir()`, que sale de `AVI_BIN_DIR` o de la convención de rutas, ninguna de las
/// dos con prefijo. El prefijo solo puede llegar **desde el registro**, escrito a mano
/// o por otra herramienta.
pub fn plan_integrate(actual: Option<&RawPath>, entry: &Path) -> Plan {
    let expand_sz = windows_sys::Win32::System::Registry::REG_EXPAND_SZ;
    let Some(raw) = actual else {
        return Plan {
            value: entry.display().to_string(),
            kind: expand_sz,
            changed: true,
        };
    };
    let wanted = entry.display().to_string();
    let already = raw
        .value
        .split(';')
        .filter(|s| !s.is_empty())
        .any(|s| crate::canonical_path_entry_matches(Path::new(s), entry));
    if already {
        return Plan {
            value: raw.value.clone(),
            kind: raw.kind,
            changed: false,
        };
    }
    let mut value = String::new();
    if !raw.value.is_empty() {
        value.push_str(&raw.value);
        if !raw.value.ends_with(';') {
            value.push(';');
        }
    }
    value.push_str(&wanted);
    Plan {
        value,
        kind: raw.kind,
        changed: true,
    }
}

/// Plan de reversión: se elimina **solo** la entrada del directorio de programa,
/// con la misma comparación canónica, conservando el tipo y el resto del texto.
pub fn plan_revert(actual: Option<&RawPath>, entry: &Path) -> Plan {
    let Some(raw) = actual else {
        return Plan {
            value: String::new(),
            kind: windows_sys::Win32::System::Registry::REG_EXPAND_SZ,
            changed: false,
        };
    };
    let remaining: Vec<&str> = raw
        .value
        .split(';')
        .filter(|s| !s.is_empty())
        .filter(|s| !crate::canonical_path_entry_matches(Path::new(s), entry))
        .collect();
    let value = remaining.join(";");
    let changed = value != raw.value;
    Plan {
        value,
        kind: raw.kind,
        changed,
    }
}

/// Resultado de integrar o revertir la entrada.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Outcome {
    /// `true` si el valor del registro se reescribió; `false` cuando ya estaba.
    pub changed: bool,
    /// `true` si se difundió `WM_SETTINGCHANGE`.
    pub broadcast: bool,
}

/// Añade la entrada del directorio de programa al final del valor `Path`, conserva
/// el tipo y las entradas `%VAR%` intactas, y difunde `WM_SETTINGCHANGE` cuando
/// escribe.
pub fn integrate(subkey: &str, entry: &Path) -> anyhow::Result<Outcome> {
    let plan = plan_integrate(read_path(subkey)?.as_ref(), entry);
    if plan.changed {
        write_path(subkey, &plan.value, plan.kind)?;
    }
    Ok(Outcome {
        changed: plan.changed,
        broadcast: plan.changed && broadcast_setting_change(),
    })
}

/// Elimina la entrada del directorio de programa con la misma comparación
/// canónica, conservando el tipo y las demás entradas, y vuelve a difundir
/// `WM_SETTINGCHANGE` si escribió.
pub fn revert(subkey: &str, entry: &Path) -> anyhow::Result<Outcome> {
    let plan = plan_revert(read_path(subkey)?.as_ref(), entry);
    if plan.changed {
        write_path(subkey, &plan.value, plan.kind)?;
    }
    Ok(Outcome {
        changed: plan.changed,
        broadcast: plan.changed && broadcast_setting_change(),
    })
}

/// Escribe el valor conservando `kind` y con el NUL final que el registro exige.
fn write_path(subkey: &str, value: &str, kind: u32) -> anyhow::Result<()> {
    use windows_sys::Win32::System::Registry::RegSetKeyValueW;
    let data = wide(value);
    // SAFETY: los tres punteros son a `Vec<u16>` terminados en NUL que viven hasta
    // el final de la llamada, y `cb` es su longitud real en bytes.
    let code = unsafe {
        RegSetKeyValueW(
            windows_sys::Win32::System::Registry::HKEY_CURRENT_USER,
            wide(subkey).as_ptr(),
            wide(PATH_VALUE).as_ptr(),
            kind,
            data.as_ptr().cast(),
            u32::try_from(data.len() * std::mem::size_of::<u16>()).unwrap_or(u32::MAX),
        )
    };
    check(code, "escribir")
}

/// Crea una clave propia del llamante, para pruebas, **sin ningún valor `Path`**, de
/// modo que la integración tenga que crearlo desde cero: es el caso de "el valor no
/// existía", con su `REG_EXPAND_SZ` de serie.
///
/// Devuelve la ruta de subclave bajo `HKCU`, que hay que borrar con [`delete_key`].
pub fn create_key(subkey: &str) -> anyhow::Result<()> {
    use windows_sys::Win32::System::Registry::{RegCreateKeyExW, HKEY, REG_OPTION_NON_VOLATILE};
    let mut handle: HKEY = 0;
    // SAFETY: el handle devuelto se cierra antes de salir. `lpclass` y
    // `lpdwdisposition` nulos son valores documentados.
    let code = unsafe {
        RegCreateKeyExW(
            windows_sys::Win32::System::Registry::HKEY_CURRENT_USER,
            wide(subkey).as_ptr(),
            0,
            std::ptr::null(),
            REG_OPTION_NON_VOLATILE,
            windows_sys::Win32::System::Registry::KEY_READ
                | windows_sys::Win32::System::Registry::KEY_WRITE,
            std::ptr::null(),
            &mut handle,
            std::ptr::null_mut(),
        )
    };
    check(code, "crear la clave de prueba")?;
    // SAFETY: `handle` es un valor devuelto válido y no se usa después de esto.
    unsafe { windows_sys::Win32::Foundation::CloseHandle(handle) };
    Ok(())
}

/// Crea una clave propia del llamante con un `Path` de partida, para probar la
/// conservación del tipo y de las entradas `%VAR%` (el criterio 7).
pub fn create_key_for_test(subkey: &str, value: &str, kind: u32) -> anyhow::Result<()> {
    create_key(subkey)?;
    write_path(subkey, value, kind)
}

/// Borra una clave creada por [`create_key_for_test`].
///
/// El registro de Windows no tiene borrado recursivo, así que la clave de prueba
/// tiene que quedar vacía de subclaves antes; el módulo no ofrece borrar en
/// cascada porque no necesita esa capacidad para nada y sería un poder que el
/// motor no tiene por qué tener.
pub fn delete_key(subkey: &str) -> anyhow::Result<()> {
    use windows_sys::Win32::System::Registry::RegDeleteKeyW;
    // SAFETY: el puntero es a un `Vec<u16>` terminado en NUL vivo durante la
    // llamada.
    let code = unsafe {
        RegDeleteKeyW(
            windows_sys::Win32::System::Registry::HKEY_CURRENT_USER,
            wide(subkey).as_ptr(),
        )
    };
    check(code, "borrar la clave de prueba")
}

/// Difunde `WM_SETTINGCHANGE` ("Environment") al escritorio entero con tiempo
/// límite. Es lo que hace que una terminal ya abierta vea el `PATH` nuevo.
pub fn broadcast_setting_change() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SendMessageTimeoutW, HWND_BROADCAST, WM_SETTINGCHANGE,
    };
    // SAFETY: el puntero es a un `Vec<u16>` terminado en NUL vivo durante la
    // llamada, y `lpdwresult` nulo está permitido.
    (unsafe {
        SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            0,
            wide("Environment").as_ptr() as isize,
            SMTO_ABORTIFHUNG,
            BROADCAST_TIMEOUT_MS,
            std::ptr::null_mut(),
        )
    }) != 0
}

/// Convierte a `Vec<u16>` con NUL final: la forma en que el registro guarda los
/// valores de cadena y los nombres de clave y valor.
fn wide(text: &str) -> Vec<u16> {
    let mut out: Vec<u16> = text.encode_utf16().collect();
    out.push(0);
    out
}

/// Traduce un código de `advapi32` a error, nombrando la clave implicada y el
/// código, que es lo que hace falta para diagnosticar sin abrir un canal de incidencias.
fn check(code: u32, what: &str) -> anyhow::Result<()> {
    if code == windows_sys::Win32::Foundation::ERROR_SUCCESS {
        return Ok(());
    }
    Err(LifecycleError::new(
        "path_conflict",
        14,
        format!("no se pudo {what} el valor {PATH_VALUE} de HKCU (código de Windows {code})"),
    )
    .into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use windows_sys::Win32::System::Registry::{REG_EXPAND_SZ, REG_SZ};

    /// La regla de idempotencia del criterio 2 sobre el lado de Windows, sin
    /// escribir en el registro: con la entrada ya presente, nada cambia.
    #[test]
    fn path_windows_plan_is_idempotent() {
        let entry = Path::new(r"C:\Users\ana\AppData\Local\Programs\ai-voice-interconnector");
        let actual = RawPath {
            value: r"C:\Windows;C:\Users\ana\AppData\Local\Programs\ai-voice-interconnector"
                .to_string(),
            kind: REG_EXPAND_SZ,
        };

        let plan = plan_integrate(Some(&actual), entry);
        assert!(!plan.changed, "la entrada ya estaba: no se escribe");
        assert_eq!(plan.value, actual.value, "ni se toca el texto");
        assert_eq!(plan.kind, REG_EXPAND_SZ, "ni el tipo");

        // Sin la entrada, se añade al final y el tipo se conserva.
        let without_entry = RawPath {
            value: r"C:\Windows;C:\Python".to_string(),
            kind: REG_EXPAND_SZ,
        };
        let plan = plan_integrate(Some(&without_entry), entry);
        assert!(plan.changed);
        assert_eq!(
            plan.value,
            r"C:\Windows;C:\Python;C:\Users\ana\AppData\Local\Programs\ai-voice-interconnector",
            "al final, con `;` y sin pisar lo anterior"
        );
        assert_eq!(plan.kind, REG_EXPAND_SZ, "el tipo leído se conserva");

        // Sin valor previo: se crea con `REG_EXPAND_SZ`, que es lo que fija la regla.
        let plan = plan_integrate(None, entry);
        assert!(plan.changed);
        assert_eq!(plan.value, entry.display().to_string());
        assert_eq!(plan.kind, REG_EXPAND_SZ);

        // Un valor que ya acaba en `;` no recibe otro.
        let with_separator = RawPath {
            value: r"C:\Windows;".to_string(),
            kind: REG_SZ,
        };
        let plan = plan_integrate(Some(&with_separator), entry);
        assert_eq!(
            plan.value,
            format!(r"C:\Windows;{}", entry.display()),
            "no se duplica el separador"
        );
        assert_eq!(
            plan.kind, REG_SZ,
            "un `REG_SZ` del usuario sigue siendo `REG_SZ`"
        );
    }

    /// La entrada escrita con `%VAR%` y la misma ruta expandida se reconocen como
    /// la misma, que es el punto del criterio 7: comparar solo la forma literal
    /// dejaría duplicado el directorio de programa.
    ///
    /// El perfil sale del entorno en vez de estar escrito a mano: si el literal
    /// fuera de otro usuario, la prueba compararía dos rutas distintas y pasaría sin
    /// afirmar nada.
    #[test]
    fn path_windows_canonical_comparison_sees_expanded_form() {
        let profile = std::env::var("USERPROFILE").expect("sesión de Windows con USERPROFILE");
        let entry = PathBuf::from(&profile).join("AppData/Local/Programs/ai-voice-interconnector");
        let with_value = RawPath {
            value: format!(
                r"%SystemRoot%\system32;%USERPROFILE%{}",
                entry.display().to_string().replacen(&profile, "", 1)
            ),
            kind: REG_EXPAND_SZ,
        };
        let plan = plan_integrate(Some(&with_value), &entry);
        assert!(
            !plan.changed,
            "la entrada con `%USERPROFILE%` ya es la del directorio de programa: {}",
            with_value.value
        );

        // Y al revés: la reversión quita la forma con variable.
        let plan = plan_revert(Some(&with_value), &entry);
        assert!(plan.changed);
        assert_eq!(
            plan.value, r"%SystemRoot%\system32",
            "la entrada ajena queda intacta y con su `%VAR%`"
        );
        assert_eq!(plan.kind, REG_EXPAND_SZ, "el tipo se conserva al revertir");

        // Una entrada que solo se parece no se quita.
        let neighbor = RawPath {
            value: format!(
                r"C:\Windows;{}\otro-programa",
                entry
                    .display()
                    .to_string()
                    .replacen(&entry.display().to_string(), "", 1)
            ),
            kind: REG_EXPAND_SZ,
        };
        let plan = plan_revert(Some(&neighbor), &entry);
        assert!(!plan.changed, "un directorio vecino no es el del programa");
        assert_eq!(plan.value, neighbor.value);

        // Sin valor previo, revertir no inventa nada.
        let plan = plan_revert(None, &entry);
        assert!(!plan.changed);
    }
}
