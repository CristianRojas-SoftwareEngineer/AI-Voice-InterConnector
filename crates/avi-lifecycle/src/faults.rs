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
//!
//! El estado de armado es **por hilo**, y el módulo `state` explica por qué un
//! cerrojo de proceso no habría servido.

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

/// Estado de los puntos de fallo de `faults`.
///
/// El estado es **por hilo**, no por proceso, y no por un detalle de
/// implementación: cada `#[test]` corre en su propio hilo dentro del mismo
/// binario, y el arnés los ejecuta en paralelo. Con un estado por proceso, una
/// prueba que arma un punto contamina a una prueba vecina que no arma nada, y el
/// síntoma es un `fallo inyectado en before_park` en una prueba que jamás
/// pidió ese fallo.
///
/// Un cerrojo de proceso **no** arregla esto, y conviene que quede escrito por
/// qué, porque es la solución que se le ocurre a cualquiera que lea el síntoma
/// después. El cerrojo solo serializa entre las pruebas que arman: la que no
/// arma nunca lo toma, así que su `trip` sigue viendo el punto que armó la
/// vecina y aborta su flujo con un fallo que nadie pidió. Medido con el cerrojo
/// en su sitio, la suite seguía fallando tres de tres.
///
/// Lo que sí aísla es que el estado no se comparta, y es coherente con el uso
/// que le da el motor: `trip` se llama desde el hilo que ejecuta el flujo bajo
/// prueba, y ese mismo hilo es el que arma el punto. No hay ningún caso en el
/// que un hilo tenga que ver el punto que armó otro.
#[cfg(feature = "faults")]
mod state {
    use super::FaultPoint;
    use std::cell::RefCell;

    thread_local! {
        static ARMED: RefCell<Vec<FaultPoint>> = const { RefCell::new(Vec::new()) };
    }

    /// Permanece armado el punto mientras la guardia está viva, y lo desarma sola
    /// al soltarse. La guardia pertenece al hilo que la creó: al soltarla en otro
    /// hilo no puede alcanzar el estado de este.
    pub struct Armed {
        point: FaultPoint,
    }

    impl Drop for Armed {
        fn drop(&mut self) {
            ARMED.with(|armed| {
                armed.borrow_mut().retain(|p| *p != self.point);
            });
        }
    }

    /// Arma `point` en el hilo que llama hasta que se suelte la guardia.
    pub fn armed(point: FaultPoint) -> Armed {
        ARMED.with(|armed| armed.borrow_mut().push(point));
        Armed { point }
    }

    /// Dice si `point` está armado en el hilo que llama.
    pub fn is_armed(point: FaultPoint) -> bool {
        ARMED.with(|armed| armed.borrow().contains(&point))
    }
}

/// Arma un punto de fallo en este hilo mientras la guardia está viva.
/// Sin el feature `faults` la guardia es inerte: es lo que garantiza que el
/// binario distribuido no pueda provocar un fallo a propósito.
#[cfg(feature = "faults")]
pub use state::{armed, is_armed, Armed};

/// Guardia inerte: existe para que los puntos de armado compilen igual sin el
/// feature, y no tiene nada que desarmar. Lleva `Drop` vacío para que las dos
/// variantes tengan la misma forma —una guardia que suelta algo al caer— y las
/// pruebas puedan llamar a `drop(window)` sin que Clippy lo marque por
/// `drop_non_drop` cuando el feature está apagado.
#[cfg(not(feature = "faults"))]
pub struct Armed;

#[cfg(not(feature = "faults"))]
impl Drop for Armed {
    fn drop(&mut self) {}
}

#[cfg(not(feature = "faults"))]
pub fn armed(_point: FaultPoint) -> Armed {
    Armed
}

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

#[cfg(all(test, feature = "faults"))]
mod tests {
    use super::*;
    use std::thread;

    /// El punto armado es invisible para los demás hilos.
    ///
    /// Esta es la evidencia de que el defecto está cerrado. La comprobación
    /// contraria no valdría —que el fallo ya no se reproduzca—, porque antes de
    /// arreglarlo tampoco se reproducía en tres de cada cuatro corridas.
    ///
    /// Usa `OnJournalWrite` porque ninguna otra prueba de este binario lo arma:
    /// así lo que lee el observador es exactamente la guardia de esta prueba y
    /// nada más. Y antes de concluir comprueba que la guardia seguía armada en su
    /// propio hilo, porque con un estado compartido otra prueba concurrent podría
    /// haber borrado el punto —`Drop` borra todas las apariciones de él— y entonces
    /// la lectura del observador no probaría nada: la prueba pasaría sin que el
    /// estado fuera por hilo. Ese era el fallo de la versión anterior de esta
    /// prueba, y por eso la comprobación se queda.
    #[test]
    fn armed_point_is_invisible_to_other_threads() {
        let guard = armed(FaultPoint::OnJournalWrite);
        assert!(
            is_armed(FaultPoint::OnJournalWrite),
            "el punto armado se ve en su propio hilo"
        );

        let observer = thread::spawn(|| is_armed(FaultPoint::OnJournalWrite));
        let seen = observer.join().expect("el hilo observador no debe fallar");

        assert!(
            is_armed(FaultPoint::OnJournalWrite),
            "la guardia se desarmó durante la observación, así que la lectura del observador no prueba nada"
        );
        assert!(!seen, "el punto armado es visible desde otro hilo");

        drop(guard);
        assert!(
            !is_armed(FaultPoint::OnJournalWrite),
            "al soltar la guardia se desarma"
        );
    }

    /// Al soltarse la guardia desaparecen solo los puntos que ella armó.
    #[test]
    fn dropping_one_guard_leaves_the_others_armed() {
        let before_park = armed(FaultPoint::BeforePark);
        let before_commit = armed(FaultPoint::BeforeCommit);

        drop(before_park);
        assert!(
            !is_armed(FaultPoint::BeforePark),
            "el punto soltado ya no está armado"
        );
        assert!(
            is_armed(FaultPoint::BeforeCommit),
            "el otro punto sigue armado"
        );

        drop(before_commit);
        assert!(
            !is_armed(FaultPoint::BeforeCommit),
            "al soltar la última guardia no queda ninguno"
        );
    }
}
