//! Plan de fallos: puntos de inyección nombrados que una prueba puede usar para
//! provocar un fallo en un instante exacto del flujo.
//!
//! Sin este módulo, probar la reversión de un reemplazo transaccional obliga a
//! simular el fallo, y una simulación no ejercita la rama de reversión real.
//! §13 lo autoriza, y solo en los builds de prueba: el binario distribuido se
//! compila sin el feature `faults`, con lo que el estado de armado no existe y
//! `trip` se reduce a `Ok(())`.
//!
//! Uso: la prueba arma el punto, ejecuta el flujo y comprueba el desenlace —
//! `rolled_back` con el programa restaurado, `daemon_stop_failed` sin haber
//! modificado nada, el diario limpio. La lista de puntos crece solo cuando una
//! prueba necesita uno que no está.

use anyhow::{bail, Result};

/// Puntos del flujo en los que se puede inyectar un fallo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FaultPoint {
    /// Antes de aparcar el contenido actual del directorio de programa (§9.3.6.1).
    BeforePark,
    /// Con el contenido aparcado y antes de colocar el bundle nuevo (§9.3.6.2).
    BeforePlace,
    /// Con el bundle colocado y antes de ajustar permisos (§9.3.6.3).
    BeforeFixPermissions,
    /// Antes de confirmar la transacción y borrar el aparcado (§9.3.6.4).
    BeforeCommit,
    /// Antes de integrar el PATH (§9.3.1).
    BeforePathIntegration,
    /// Antes de escribir el recibo (§9.3, paso 10).
    BeforeReceipt,
    /// Al parar el daemon (§9.3, paso 5).
    OnDaemonStop,
    /// Al escribir el diario de la transacción (§9.1, recuperación).
    OnJournalWrite,
    /// Antes del traspaso de `self update` al binario nuevo (§9.4, paso 9).
    BeforeHandover,
    /// Con el traspaso ya ejecutado y antes de la limpieza del staging (§9.4,
    /// paso 10): simula la interrupción que deja el staging para la
    /// recuperación de la siguiente operación.
    DuringHandover,
}

impl FaultPoint {
    /// Nombre estable del punto, para mensajes de prueba y aserciones.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BeforePark => "before_park",
            Self::BeforePlace => "before_place",
            Self::BeforeFixPermissions => "before_fix_permissions",
            Self::BeforeCommit => "before_commit",
            Self::BeforePathIntegration => "before_path_integration",
            Self::BeforeReceipt => "before_receipt",
            Self::OnDaemonStop => "on_daemon_stop",
            Self::OnJournalWrite => "on_journal_write",
            Self::BeforeHandover => "before_handover",
            Self::DuringHandover => "during_handover",
        }
    }
}

#[cfg(feature = "faults")]
mod state {
    use super::FaultPoint;
    use std::sync::Mutex;

    static ARMED: Mutex<Vec<FaultPoint>> = Mutex::new(Vec::new());

    pub fn arm(point: FaultPoint) {
        ARMED.lock().unwrap().push(point);
    }

    pub fn disarm_all() {
        ARMED.lock().unwrap().clear();
    }

    pub fn is_armed(point: FaultPoint) -> bool {
        ARMED.lock().unwrap().contains(&point)
    }
}

/// Arma un punto de fallo. Sin el feature `faults` no hace nada: es lo que
/// garantiza que el binario distribuido no pueda provocar un fallo a propósito.
#[cfg(feature = "faults")]
pub use state::{arm, disarm_all, is_armed};

#[cfg(not(feature = "faults"))]
pub fn arm(_point: FaultPoint) {}

#[cfg(not(feature = "faults"))]
pub fn disarm_all() {}

#[cfg(not(feature = "faults"))]
pub fn is_armed(_point: FaultPoint) -> bool {
    false
}

/// Falla aquí si el punto está armado. El motor la llama en los puntos del flujo
/// y propaga el error con `?`, de modo que la reversión se ejecuta por el mismo
/// camino que con un fallo real.
#[inline]
pub fn trip(point: FaultPoint) -> Result<()> {
    if is_armed(point) {
        bail!("fallo inyectado en {}", point.as_str());
    }
    Ok(())
}
