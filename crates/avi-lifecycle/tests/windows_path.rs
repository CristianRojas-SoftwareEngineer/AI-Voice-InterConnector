//! Criterio 7: el tipo del valor `Path` de `HKCU\Environment` y las entradas
//! `%VAR%` quedan intactos después de instalar y de desinstalar.
//!
//! La prueba opera sobre **una clave propia del test**, no sobre la del usuario:
//! El aislamiento lo exige y es la única forma de que una puerta de CI pueda demostrarla
//! sin
//! tocar el entorno de quien la ejecuta. La ruta de la clave es un parámetro de
//! `avi-lifecycle::path_windows` precisamente para esto.
//!
//! Corre en Windows; en Unix el archivo no compila nada porque todo lo que
//! afirma depende de `advapi32`. No hay ninguna marca de omitida: el criterio 7 no
//! **se salta** en Unix, sencillamente no tiene código que compilar allí, porque su
//! sujeto —el valor `Path` de `HKCU\Environment` y sus entradas `%VAR%`— no existe
//! fuera de Windows. La puerta de Unix sigue en verde sin que ninguna prueba se haya
//! declarado aplicable por el mismo motivo.

#![cfg(windows)]

mod support;

use avi_lifecycle::install::{self, Options as InstallOptions};
use avi_lifecycle::path_windows::{
    create_key_for_test, delete_key, integrate, plan_integrate, plan_revert, read_path, revert,
    RawPath,
};
use std::path::PathBuf;
use support::{Inert, Sandbox};
use windows_sys::Win32::System::Registry::{REG_EXPAND_SZ, REG_SZ};

/// Subclave propia de esta ejecución. El PID y un contador evitan que dos
/// ejecuciones simultáneas —o dos pruebas del mismo binario— se pisen.
fn test_key(tag: &str) -> String {
    format!(
        r"Software\AI-Voice-InterConnector\lifecycle-test-{}-{tag}",
        std::process::id()
    )
}

/// Valor de partida: un `Path` real de Windows, con las dos formas que el
/// criterio 7 nombra —una entrada con `%SystemRoot%` y otra con `%USERPROFILE%`— y
/// una ruta absoluta sin variable.
const INITIAL_VALUE: &str =
    r"%SystemRoot%\system32;%USERPROFILE%\AppData\Local\Programs;C:\herramientas\bin";

/// El directorio de programa que se integraría, escrito en la forma absoluta que
/// el motor usaría.
fn entry() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(r"C:\Users\ana\AppData\Local"));
    base.join("Programs").join("ai-voice-interconnector")
}

/// **Criterio 7, de punta a punta por el motor.** Instalar y desinstalar deja el tipo
/// del valor `Path` y sus entradas `%VAR%` exactamente como estaban.
///
/// La diferencia con `windows_user_path_type_and_vars_survive`, que está justo debajo, es
/// quién hace el trabajo: allí la integración y la reversión se llaman directamente, y
/// aquí la integración la hace `self install` leyendo la subclave que su `Env` declara y
/// escribiendo el recibo que dice qué revertir. Es la diferencia entre comprobar la
/// primitiva y comprobar el criterio.
///
/// La reversión se aplica con `path_windows::revert` sobre la **misma clave de prueba**,
/// y no llamando a `self uninstall`, por un motivo que conviene tener presente:
/// `uninstall::revert_path` no recibe la subclave —revierte contra
/// `path_windows::ENV_SUBKEY`, es decir `HKCU\Environment`—, de modo que invocar la
/// desinstalación real dejaría la prueba leyendo y escribiendo el `PATH` real de quien
/// ejecuta la puerta. `path_windows::revert` es exactamente la función a la que delega
/// esa rama, con la misma subclave que `install` usó, así que lo que se afirma es lo que
/// la desinstalación haría. Y como control, la prueba comprueba que el valor `Path` del
/// usuario no ha cambiado: si algo tocara `HKCU\Environment`, saldría aquí.
///
/// La columna de rutas para Windows dice que el directorio del enlace **es** el directorio
/// de programa, así que el sandbox lo dice con `bin_dir = program_dir` en vez de la ruta
/// de Unix.
#[test]
fn criterion_7_windows_user_path_type_and_vars_survive() {
    let _guard = support::exclusively();
    let mut sandbox = Sandbox::new("reversion-path-windows");
    sandbox.bin_dir = sandbox.program_dir.clone();
    sandbox.seed_env();
    let key = sandbox.registry_subkey.clone();

    // El `Path` del usuario real, para poder afirmar que la prueba no lo tocó.
    let real_before = read_path(avi_lifecycle::path_windows::ENV_SUBKEY)
        .ok()
        .flatten()
        .map(|raw| (raw.value, raw.kind));

    // Punto de partida: un `Path` de usuario con las dos formas con variable y una
    // absoluta, de tipo `REG_EXPAND_SZ`.
    create_key_for_test(&key, INITIAL_VALUE, REG_EXPAND_SZ).expect("se crea la clave de prueba");
    assert_eq!(
        read_path(&key).unwrap().unwrap().kind,
        REG_EXPAND_SZ,
        "criterio 7: el valor de partida es `REG_EXPAND_SZ`"
    );

    // ── Instalar ────────────────────────────────────────────────────────────────
    let exe = sandbox.write_bundle(&sandbox.staging);
    let runtime = support::runtime();
    let outcome = runtime
        .block_on(install::install(
            &sandbox.install_env(&exe),
            &InstallOptions {
                assume_yes: true,
                no_setup: true,
                no_modify_path: false,
                force: false,
                channel: None,
                with_voice_cloning: false,
            },
            &Inert,
        ))
        .expect("criterio 7: la instalación se completa");
    assert!(
        outcome.path_integrated(),
        "criterio 7: el `PATH` quedó integrado"
    );
    assert!(
        outcome.path_changed(),
        "criterio 7: y esta pasada lo escribió"
    );
    let entry = outcome
        .path_integration
        .registry_entry
        .clone()
        .expect("criterio 7: el recibo registra la entrada del registro");
    assert_eq!(
        entry, sandbox.program_dir,
        "criterio 7: en Windows la entrada es el directorio de programa"
    );
    assert!(
        outcome.path_integration.symlink.is_none(),
        "criterio 7: y no hay enlace simbólico, que es un mecanismo de Unix"
    );

    let after_install = read_path(&key)
        .unwrap()
        .expect("criterio 7: el valor sigue ahí");
    assert_eq!(
        after_install.kind, REG_EXPAND_SZ,
        "criterio 7: el tipo se conserva después de instalar: se lee sin expandir y se \
         escribe con el mismo tipo"
    );
    assert_eq!(
        after_install.value,
        format!("{INITIAL_VALUE};{}", entry.display()),
        "criterio 7: la entrada se añade al final sin tocar lo anterior"
    );
    assert!(
        after_install.value.contains("%SystemRoot%\\system32")
            && after_install
                .value
                .contains("%USERPROFILE%\\AppData\\Local\\Programs"),
        "criterio 7: las dos entradas `%VAR%` siguen sin expandir: {}",
        after_install.value
    );
    assert_eq!(
        after_install
            .value
            .matches(&entry.display().to_string())
            .count(),
        1,
        "criterio 7: y la entrada del programa aparece una sola vez"
    );

    // ── Desinstalar: la misma entrada, la misma clave ──────────────────────────
    let uninstall = revert(&key, &entry).expect("criterio 7: la reversión se aplica");
    assert!(uninstall.changed, "criterio 7: había entrada que quitar");
    assert!(
        uninstall.broadcast,
        "criterio 7: se vuelve a difundir `WM_SETTINGCHANGE`"
    );

    let after_uninstall = read_path(&key)
        .unwrap()
        .expect("criterio 7: el valor sigue ahí");
    assert_eq!(
        after_uninstall.kind, REG_EXPAND_SZ,
        "criterio 7: el tipo se conserva también al revertir"
    );
    assert_eq!(
        after_uninstall.value, INITIAL_VALUE,
        "criterio 7: el valor vuelve a ser byte a byte el de partida"
    );
    assert!(
        after_uninstall.value.contains("%SystemRoot%\\system32")
            && after_uninstall
                .value
                .contains("%USERPROFILE%\\AppData\\Local\\Programs"),
        "criterio 7: y las entradas `%VAR%` siguen intactas: {}",
        after_uninstall.value
    );

    // Y nada tocó el `Path` real del usuario, que es el control que hace que esta
    // prueba sea admisible en una puerta.
    let real_after = read_path(avi_lifecycle::path_windows::ENV_SUBKEY)
        .ok()
        .flatten()
        .map(|raw: RawPath| (raw.value, raw.kind));
    assert_eq!(
        real_after, real_before,
        "criterio 7: el `Path` real de `HKCU\\Environment` no se ha tocado"
    );
}

/// El criterio 7, de punta a punta: instalar y desinstalar sobre una clave de
/// prueba deja el tipo y las entradas `%VAR%` exactamente como estaban.
#[test]
fn windows_user_path_type_and_vars_survive() {
    let key = test_key("tipo");
    let entry = entry();

    create_key_for_test(&key, INITIAL_VALUE, REG_EXPAND_SZ).expect("se crea la clave de prueba");

    let clean = || -> String {
        read_path(&key)
            .expect("se lee")
            .expect("el valor existe")
            .value
    };

    // Punto de partida: el tipo es `REG_EXPAND_SZ` y las dos entradas con
    // variable están tal cual.
    assert_eq!(
        read_path(&key).unwrap().unwrap().kind,
        REG_EXPAND_SZ,
        "el valor de partida es `REG_EXPAND_SZ`"
    );
    assert_eq!(clean(), INITIAL_VALUE);

    // Instalar: la entrada se añade y se diffuse el cambio.
    let installation = integrate(&key, &entry).expect("la integración se aplica");
    assert!(installation.changed, "la entrada no estaba: se escribe");
    assert!(installation.broadcast, "se difunde `WM_SETTINGCHANGE`");

    let after_install = read_path(&key).unwrap().expect("el valor sigue ahí");
    assert_eq!(
        after_install.kind, REG_EXPAND_SZ,
        "el tipo se conserva después de instalar: se lee sin expandir y se escribe con el mismo tipo"
    );
    assert_eq!(
        after_install.value,
        format!("{INITIAL_VALUE};{}", entry.display()),
        "la entrada se añade al final sin tocar lo anterior"
    );
    assert!(
        after_install.value.contains("%SystemRoot%\\system32"),
        "la entrada `%SystemRoot%` sigue sin expandir"
    );
    assert!(
        after_install
            .value
            .contains("%USERPROFILE%\\AppData\\Local\\Programs"),
        "la entrada `%USERPROFILE%` sigue sin expandir"
    );

    // Instalar otra vez es un no-op: es el criterio 2 sobre el registro.
    let repeated = integrate(&key, &entry).expect("la segunda integración se aplica");
    assert!(
        !repeated.changed,
        "la entrada ya estaba: no se reescribe el valor"
    );
    assert!(!repeated.broadcast, "y no se difunde un cambio que no hubo");
    assert_eq!(
        read_path(&key).unwrap().unwrap().value,
        after_install.value,
        "el valor no se duplica"
    );

    // Desinstalar: la entrada del programa desaparece y lo demás queda byte a
    // byte igual, con el mismo tipo.
    let uninstall = revert(&key, &entry).expect("la reversión se aplica");
    assert!(uninstall.changed, "había entrada que quitar");
    assert!(uninstall.broadcast, "se vuelve a difundir el cambio");

    let after_uninstall = read_path(&key).unwrap().expect("el valor sigue ahí");
    assert_eq!(
        after_uninstall.kind, REG_EXPAND_SZ,
        "el tipo se conserva también al revertir"
    );
    assert_eq!(
        after_uninstall.value, INITIAL_VALUE,
        "el valor vuelve a ser byte a byte el de partida"
    );
    assert!(
        after_uninstall.value.contains("%SystemRoot%\\system32")
            && after_uninstall
                .value
                .contains("%USERPROFILE%\\AppData\\Local\\Programs"),
        "las entradas `%VAR%` siguen intactas"
    );

    // Desinstalar otra vez tampoco hace nada.
    let repeated = revert(&key, &entry).expect("la segunda reversión se aplica");
    assert!(!repeated.changed, "ya no había nada que quitar");

    delete_key(&key).expect("se borra la clave de prueba");
}

/// Un `Path` que el usuario escribió como `REG_SZ` sigue siendo `REG_SZ`: la
/// regla manda conservar el tipo, no normalizarlo a `REG_EXPAND_SZ`.
#[test]
fn windows_user_path_keeps_reg_sz_type() {
    let key = test_key("regsz");
    let entry = entry();
    create_key_for_test(&key, r"C:\Windows;C:\herramientas", REG_SZ).expect("se crea la clave");

    assert_eq!(read_path(&key).unwrap().unwrap().kind, REG_SZ);
    integrate(&key, &entry).expect("se integra");
    let after = read_path(&key).unwrap().expect("el valor sigue ahí");
    assert_eq!(
        after.kind, REG_SZ,
        "un `REG_SZ` del usuario no se convierte en `REG_EXPAND_SZ`"
    );
    revert(&key, &entry).expect("se revierte");
    assert_eq!(read_path(&key).unwrap().unwrap().kind, REG_SZ);
    delete_key(&key).expect("se borra la clave de prueba");
}

/// Un `Path` que no existe se crea con `REG_EXPAND_SZ`, que es lo que fija la regla, y
/// una clave que no existe se trata igual: no es un error de la integración.
#[test]
fn windows_user_path_absent_value_is_created_expandable() {
    let entry = entry();
    let key = test_key("ausente");
    // Clave **sin** valor: es el caso real de "el valor no existía".
    avi_lifecycle::path_windows::create_key(&key).expect("se crea la clave de prueba");
    assert!(
        read_path(&key).unwrap().is_none(),
        "una clave sin valor se lee como ausente"
    );

    let plan = plan_integrate(read_path(&key).unwrap().as_ref(), &entry);
    assert!(plan.changed);
    assert_eq!(
        plan.kind, REG_EXPAND_SZ,
        "el valor que no existía se crea como `REG_EXPAND_SZ`, que es lo que permite \
         que el propio `Path` del usuario siga usando `%VAR%`"
    );
    assert_eq!(plan.value, entry.display().to_string());

    let plan = plan_revert(None, &entry);
    assert!(!plan.changed, "revertir sin valor no inventa nada");

    // Un valor presente pero vacío **no** es un valor ausente: se distingue, porque
    // "vacío" puede querer decir que el usuario lo vació y eso hay que conservarlo.
    let empty = test_key("vacio");
    create_key_for_test(&empty, "", REG_EXPAND_SZ).expect("se crea la clave con valor vacío");
    let loaded = read_path(&empty)
        .unwrap()
        .expect("el valor existe, aunque esté vacío");
    assert_eq!(loaded.value, "");
    assert_eq!(loaded.kind, REG_EXPAND_SZ);
    let plan = plan_integrate(Some(&loaded), &entry);
    assert!(plan.changed);
    assert_eq!(
        plan.value,
        entry.display().to_string(),
        "y la integración no antepone un `;` a un valor vacío"
    );
    assert_eq!(plan.kind, REG_EXPAND_SZ, "conservando el tipo leído");
    delete_key(&empty).expect("se borra la clave de prueba");

    // Y una clave que no existe, directamente: leerla da `None`, no un error.
    let missing = test_key("inexistente");
    assert!(
        read_path(&missing).unwrap().is_none(),
        "una clave ausente no es un error de lectura"
    );
    delete_key(&key).expect("se borra la clave de prueba");
}

/// La comparación canónica se apoya en `avi-store`, así que la misma ruta escrita
/// con `%VAR%` y expandida se reconoce como una sola entrada, que es lo que hace
/// que la reversión sea exacta en vez de dejar un duplicado.
#[test]
fn windows_user_path_verbatim_revert_keeps_others_untouched() {
    let entry = entry();
    // `%USERPROFILE%` está definida en cualquier sesión de Windows abierta, así que la
    // forma con variable de la misma ruta se puede construir sin suponer nada: se
    // quita el prefijo del perfil de la ruta real y se lo vuelve a poner como variable.
    let profile = std::env::var("USERPROFILE").expect("sesión de Windows con USERPROFILE");
    // El separador se conserva: `%USERPROFILE%` no lo trae, y sin él pegaría el perfil
    // con `AppData`.
    let relative = entry.display().to_string().replacen(&profile, "", 1);
    assert!(
        relative.starts_with('\\'),
        "la ruta es hija del perfil y conserva el separador: {entry:?}"
    );
    let with_value = format!(r"%SystemRoot%\system32;%USERPROFILE%{relative}");

    let raw = avi_lifecycle::path_windows::RawPath {
        value: with_value.clone(),
        kind: REG_EXPAND_SZ,
    };
    let plan = plan_revert(Some(&raw), &entry);
    assert!(
        plan.changed,
        "la entrada escrita con `%USERPROFILE%` es la del programa: {with_value}"
    );
    assert_eq!(plan.value, r"%SystemRoot%\system32");
    assert_eq!(plan.kind, REG_EXPAND_SZ);

    // Y al revés: integrar cuando el `Path` ya la tiene en forma de variable no
    // duplica la entrada.
    let plan = plan_integrate(Some(&raw), &entry);
    assert!(!plan.changed, "no se duplica la entrada ya presente");
}
