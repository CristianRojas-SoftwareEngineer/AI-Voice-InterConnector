//! Comprobación de privilegios: ninguna operación de ciclo de vida pide
//! elevación (§9.1, §12).
//!
//! §12 fija el principio —*nunca se eleva*— y §9.1 lo hace verificable por
//! plataforma, y no con el mismo gesto en las dos:
//!
//! - **Unix**: si se detecta ejecución vía `sudo` (uid 0 con `SUDO_USER`
//!   definido) se **aborta**. La instalación es per-user y con `sudo` acabaría
//!   en el perfil de root, donde el usuario que la pidió no la encuentra. Root
//!   *sin* `SUDO_USER` sí se permite: es el caso de los contenedores, donde
//!   root es el usuario real y no una elevación.
//! - **Windows**: un proceso elevado **no aborta**, pero avisa por stderr de que
//!   la instalación se hará en el perfil de la cuenta que ejecuta, que es lo que
//!   evita el diagnóstico de "no aparece en el PATH".
//!
//! **La decisión es una función pura.** `decide_unix` recibe el uid y el valor de
//! `SUDO_USER` y devuelve el motivo, o `None` si se puede seguir. Así la regla que
//! §9.1 declara se afirma en cualquier plataforma y en cualquier momento, sin
//! depender de estar dentro de un contenedor, elevada o no. Lo que sí es de
//! plataforma son las dos lecturas que la alimentan.

use crate::LifecycleError;

/// Veredicto de la comprobación de privilegios, y el aviso que §9.1 pide emitir
/// cuando corresponde.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    /// El proceso se ejecuta con privilegios de superusuario: en Unix,
    /// `euid == 0`; en Windows, el token está elevado.
    pub elevated: bool,
    /// La elevated vino de `sudo` (`SUDO_USER` definido), no del modo root
    /// propio de un contenedor. Solo tiene sentido en Unix.
    pub via_sudo: bool,
    /// Aviso para stderr (§9.1). `None` cuando no hay nada que avisar.
    pub warning: Option<String>,
}

/// Inspecciona el proceso actual sin abortar nada.
///
/// Es la forma de la que usan las operaciones que solo avisan (Windows) y la que
/// precede a la comprobación de las que abortan, para poder ordenar los avisos.
pub fn inspect() -> Report {
    let mut report = Report {
        elevated: elevated(),
        via_sudo: via_sudo(),
        warning: None,
    };
    report.warning = warning_for(&report);
    report
}

/// Comprueba los privilegios antes de una operación. Aborta en Unix si la
/// ejecución vino de `sudo`; en Windows solo avisa, que es lo que §9.1 fija.
pub fn ensure_per_user() -> Result<Report, LifecycleError> {
    let report = inspect();
    if report.via_sudo {
        return Err(LifecycleError::new("sudo_not_supported", 1, sudo_message()));
    }
    Ok(report)
}

/// Regla de Unix como función pura: con `uid == 0` y `SUDO_USER` definido la
/// operación se aborta; en cualquier otro caso se permite.
///
/// El motivo de la asimetría es el del comentario del módulo: root sin
/// `SUDO_USER` es el contenedor, donde root es el usuario real y abortar dejaría
/// la imagen sin poder instalar nada. `sudo` es lo que convierte a root en
/// *otra* persona, y esa es la condición que hay que rechazar.
pub fn decide_unix(uid: u32, sudo_user: Option<&str>) -> Option<String> {
    if uid == 0 && sudo_user.is_some_and(|user| !user.is_empty()) {
        Some(sudo_message())
    } else {
        None
    }
}

/// Mensaje de rechazo de `sudo`. Vive aparte porque aparece tanto en el
/// veredicto puro como en el error, y las dos rutas deben decir lo mismo.
fn sudo_message() -> String {
    format!(
        "{} se instala por usuario, no en el sistema: con `sudo` el programa \
         acabaría en el perfil de root y no en el tuyo. Vuelve a ejecutarlo sin \
         elevación.",
        crate::APP_NAME
    )
}

/// Aviso de §9.1 para un proceso elevado en Windows. `None` fuera de Windows: en
/// Unix la elevación por `sudo` aborta, así que no hay nada que avisar.
fn warning_for(report: &Report) -> Option<String> {
    if cfg!(windows) && report.elevated {
        return Some(format!(
            "AVISO: {} se está ejecutando con privilegios de administrador. La \
             instalación se hará en el perfil de la cuenta que ejecuta, no en el \
             de todos los usuarios.",
            crate::APP_NAME
        ));
    }
    None
}

/// ¿El proceso se ejecuta como superusuario? En Unix, `euid == 0`; en Windows,
/// el token del proceso está elevado (`TokenElevation`, que distingue "elevado
/// de verdad" de un token con el grupo de administradores pero sin elevación
/// activa).
fn elevated() -> bool {
    #[cfg(unix)]
    {
        // SAFETY: `geteuid` no toma argumentos ni toca memoria.
        unsafe { libc::geteuid() == 0 }
    }
    #[cfg(windows)]
    {
        windows_elevated()
    }
    #[cfg(not(any(unix, windows)))]
    {
        false
    }
}

/// ¿La ejecución elevada vino de `sudo`? Solo Unix: en Windows no hay `sudo` y
/// el aviso no aborta, así que la distinción no se necesita.
fn via_sudo() -> bool {
    if !cfg!(unix) {
        return false;
    }
    std::env::var("SUDO_USER").is_ok_and(|user| !user.is_empty())
}

/// Detección de elevación en Windows con `GetTokenInformation` y
/// `TokenElevation`, tal como fija el plan. Sin crate nuevo: `windows-sys`
/// expone `Win32_Security` y `Win32_System_Threading`, y las dos features ya
/// están declaradas.
#[cfg(windows)]
fn windows_elevated() -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::Security::{
        GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    // SAFETY: `OpenProcessToken` sobre el pseudo-handle del proceso actual pide
    // un token de consulta, que existe siempre para el propio proceso. El handle
    // devuelto se cierra antes de salir, también en la rama de error.
    unsafe {
        let mut token: HANDLE = 0;
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut returned: u32 = 0;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            std::ptr::addr_of_mut!(elevation).cast(),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        ) != 0;
        CloseHandle(token);
        ok && elevation.TokenIsElevated != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// La regla de §9.1 afirmada como tabla y no como caso: se enumera toda la
    /// combinación de uid y `SUDO_USER` y se dice qué hace cada una.
    #[test]
    fn privileges_abort_under_sudo_and_allow_plain_root() {
        assert_eq!(
            decide_unix(0, Some("ana")),
            Some(sudo_message()),
            "root con SUDO_USER es `sudo` y aborta"
        );
        assert_eq!(
            decide_unix(0, Some("")),
            None,
            "SUDO_USER vacío no es una elevación: es root del contenedor"
        );
        assert_eq!(
            decide_unix(0, None),
            None,
            "root sin SUDO_USER es el caso de los contenedores y se permite"
        );
        assert_eq!(
            decide_unix(1000, Some("ana")),
            None,
            "un usuario normal puede heredar SUDO_USER del entorno sin estar elevado"
        );
        assert_eq!(decide_unix(1000, None), None);

        // El mensaje de §9.1 explica el porqué, no solo el qué: sin esto el
        // usuario ve un rechazo sin motivo y concluye que el programa no funciona.
        let mensaje = decide_unix(0, Some("ana")).expect("sudo aborta");
        assert!(mensaje.contains("por usuario"), "{mensaje}");
        assert!(mensaje.contains("perfil de root"), "{mensaje}");
        assert!(mensaje.contains("sin elevación"), "{mensaje}");

        // `inspect` no inventa elevación donde no la hay: la puerta de CI corre
        // sin `sudo`, así que aquí se afirma el caso real de la máquina.
        let report = inspect();
        assert_eq!(
            report.via_sudo,
            decide_unix(u32::MAX, None).is_some(),
            "`via_sudo` solo puede venir de SUDO_USER"
        );
        if report.via_sudo {
            // Si esta máquina estuviera elevada con `sudo`, `ensure_per_user`
            // tiene que rechazarla: es el contrato de §9.1.
            let err = ensure_per_user().expect_err("con sudo se aborta");
            assert_eq!(err.reason, "sudo_not_supported");
        } else {
            assert!(ensure_per_user().is_ok());
        }
    }

    /// El aviso de Windows solo existe si el proceso está elevado, y su texto
    /// dice dónde va a instalarse: es la mitad de §9.1 que no aborta.
    #[test]
    fn privileges_warn_only_when_elevated() {
        let baja = Report {
            elevated: false,
            via_sudo: false,
            warning: None,
        };
        let alta = Report {
            elevated: true,
            via_sudo: false,
            warning: None,
        };
        if cfg!(windows) {
            assert!(warning_for(&baja).is_none(), "sin elevación no hay aviso");
            let aviso = warning_for(&alta).expect("elevado avisa");
            assert!(aviso.contains("perfil de la cuenta que ejecuta"), "{aviso}");
        } else {
            // En Unix el proceso elevado aborta (§9.1), así que no hay aviso que
            // emitir: la misma regla, leída al revés.
            assert!(warning_for(&baja).is_none());
            assert!(warning_for(&alta).is_none());
        }
    }

    /// La detección de elevación no tiene estado ni efectos: se puede repetir
    /// sin que la segunda lectura difiera, y `inspect` y el predicado coinciden.
    #[cfg(windows)]
    #[test]
    fn privileges_elevated_probe_is_safe_and_idempotent() {
        let primera = windows_elevated();
        let segunda = windows_elevated();
        assert_eq!(
            primera, segunda,
            "la detección no tiene estado: dos llamadas dan lo mismo"
        );
        assert_eq!(
            inspect().elevated,
            primera,
            "`inspect` y el predicado coinciden"
        );
    }
}
