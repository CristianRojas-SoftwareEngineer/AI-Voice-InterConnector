//! Puerta de concurrencia del preprocesamiento DPDFNet en el daemon.
//!
//! `ORT_CACHE` ya serializa la inferencia por proceso, pero sin admisión
//! explícita cada petición podría quedar bloqueada en un hilo de
//! `spawn_blocking` esperando el mutex sin límite ni respuesta. Esta puerta
//! conserva **una inferencia activa y a lo sumo una solicitud esperando**:
//!
//! - la primera petición adquiere el turno;
//! - la siguiente espera como máximo 30 s;
//! - cualquier otra, o la que agote su espera, recibe `denoise_busy`.
//!
//! El turno se toma antes de convertir el audio a 48 kHz o de reservar
//! buffers grandes. Al soltar el permiso se despierta al único esperador.

use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;
use tokio::time::Instant;

/// Espera máxima de la solicitud que queda en cola.
pub(crate) const DENOISE_QUEUE_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Default)]
struct GateInner {
    active: bool,
    waiting: bool,
}

/// Puerta compartida por el estado del daemon.
pub(crate) struct DenoiseGate {
    inner: Mutex<GateInner>,
    notify: Notify,
}

impl DenoiseGate {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(GateInner::default()),
            notify: Notify::new(),
        })
    }

    /// Admisión atómica: libre, en espera o saturada. La decisión no reserva
    /// audio ni temporales; solo el identificador del trabajo.
    pub(crate) fn begin(self: &Arc<Self>) -> GateBegin {
        let mut inner = self.inner.lock().unwrap();
        if !inner.active {
            inner.active = true;
            return GateBegin::Acquired(DenoisePermit { gate: self.clone() });
        }
        if inner.waiting {
            return GateBegin::Busy;
        }
        inner.waiting = true;
        GateBegin::Waiting(DenoiseWaiter {
            gate: self.clone(),
            armed: true,
        })
    }
}

pub(crate) enum GateBegin {
    Acquired(DenoisePermit),
    Waiting(DenoiseWaiter),
    Busy,
}

/// Turno activo. Al soltarse libera la puerta y despierta al esperador.
pub(crate) struct DenoisePermit {
    gate: Arc<DenoiseGate>,
}

impl Drop for DenoisePermit {
    fn drop(&mut self) {
        let mut inner = self.gate.inner.lock().unwrap();
        inner.active = false;
        self.gate.notify.notify_one();
    }
}

/// Solicitud en espera. Si se cancela (desconexión del cliente) antes de
/// obtener el turno, `Drop` libera la plaza de espera para que otra petición
/// pueda ocuparla.
pub(crate) struct DenoiseWaiter {
    gate: Arc<DenoiseGate>,
    armed: bool,
}

impl DenoiseWaiter {
    /// Espera el turno hasta `timeout`. Al vencer devuelve `Err` y libera la
    /// plaza de espera; no inicia DSP.
    pub(crate) async fn wait_timeout(
        self,
        timeout: Duration,
    ) -> Result<DenoisePermit, WaitTimeout> {
        let deadline = Instant::now() + timeout;
        let gate = self.gate.clone();
        let gate_for_notify = gate.clone();
        let mut armed = self.armed;
        loop {
            // Registrar la notificación antes de comprobar el estado evita
            // perder un aviso entre la comprobación y el `await`.
            let notified = gate_for_notify.notify.notified();
            tokio::pin!(notified);

            {
                let mut inner = gate.inner.lock().unwrap();
                if !inner.active {
                    inner.active = true;
                    inner.waiting = false;
                    drop(inner);
                    // `gate_for_notify` y `notified` se descartan al retornar;
                    // solo `gate` pasa al permiso.
                    return Ok(DenoisePermit { gate });
                }
            }

            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                release_waiter(&gate, &mut armed);
                return Err(WaitTimeout);
            };
            if tokio::time::timeout(remaining, notified).await.is_err() {
                release_waiter(&gate, &mut armed);
                return Err(WaitTimeout);
            }
            // Se recibió una notificación: reintentar la comprobación de
            // estado en la siguiente iteración.
        }
    }
}

/// Libera la plaza de espera y despierta al siguiente solicitante, si queda
/// alguno. `armed` indica si el waiter aún está activo.
fn release_waiter(gate: &DenoiseGate, armed: &mut bool) {
    if !*armed {
        return;
    }
    let mut inner = gate.inner.lock().unwrap();
    if inner.waiting {
        inner.waiting = false;
        gate.notify.notify_one();
    }
    *armed = false;
}

impl Drop for DenoiseWaiter {
    fn drop(&mut self) {
        release_waiter(&self.gate, &mut self.armed);
    }
}

/// La espera agotó los 30 s sin obtener turno.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct WaitTimeout;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn first_request_acquires_and_second_waits() {
        let gate = DenoiseGate::new();
        let permit = match gate.begin() {
            GateBegin::Acquired(permit) => permit,
            _ => panic!("la primera solicitud debe adquirir el turno"),
        };
        match gate.begin() {
            GateBegin::Waiting(waiter) => drop(waiter),
            _ => panic!("la segunda solicitud debe quedar en espera"),
        }
        drop(permit);
    }

    #[tokio::test]
    async fn a_third_request_is_busy_while_one_waits() {
        let gate = DenoiseGate::new();
        let permit = match gate.begin() {
            GateBegin::Acquired(permit) => permit,
            _ => panic!("la primera solicitud debe adquirir el turno"),
        };
        let waiter = match gate.begin() {
            GateBegin::Waiting(waiter) => waiter,
            _ => panic!("la segunda solicitud debe quedar en espera"),
        };
        assert!(matches!(gate.begin(), GateBegin::Busy));
        drop(waiter);
        drop(permit);
    }

    #[tokio::test]
    async fn releasing_the_permit_wakes_the_waiter() {
        let gate = DenoiseGate::new();
        let permit = match gate.begin() {
            GateBegin::Acquired(permit) => permit,
            _ => panic!("la primera solicitud debe adquirir el turno"),
        };
        let waiter = match gate.begin() {
            GateBegin::Waiting(waiter) => waiter,
            _ => panic!("la segunda solicitud debe quedar en espera"),
        };
        let wait = tokio::spawn(waiter.wait_timeout(Duration::from_secs(5)));
        tokio::task::yield_now().await;
        drop(permit);
        assert!(wait.await.unwrap().is_ok());
    }

    #[tokio::test]
    async fn a_timed_out_waiter_frees_the_queue_slot() {
        let gate = DenoiseGate::new();
        let permit = match gate.begin() {
            GateBegin::Acquired(permit) => permit,
            _ => panic!("la primera solicitud debe adquirir el turno"),
        };
        let waiter = match gate.begin() {
            GateBegin::Waiting(waiter) => waiter,
            _ => panic!("la segunda solicitud debe quedar en espera"),
        };
        assert!(matches!(
            waiter.wait_timeout(Duration::from_millis(10)).await,
            Err(WaitTimeout)
        ));
        match gate.begin() {
            GateBegin::Waiting(waiter) => drop(waiter),
            _ => panic!("la plaza de espera debe quedar libre tras el vencimiento"),
        }
        drop(permit);
    }

    #[tokio::test]
    async fn dropping_a_waiter_frees_the_queue_slot() {
        let gate = DenoiseGate::new();
        let permit = match gate.begin() {
            GateBegin::Acquired(permit) => permit,
            _ => panic!("la primera solicitud debe adquirir el turno"),
        };
        let waiter = match gate.begin() {
            GateBegin::Waiting(waiter) => waiter,
            _ => panic!("la segunda solicitud debe quedar en espera"),
        };
        drop(waiter);
        match gate.begin() {
            GateBegin::Waiting(waiter) => drop(waiter),
            _ => panic!("la plaza de espera debe quedar libre al cancelar"),
        }
        drop(permit);
    }
}
