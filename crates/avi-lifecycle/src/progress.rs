// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! Medición de la transferencia de cada modelo en la provisión.
//!
//! La provisión pinta una etapa `downloading` por modelo con
//! `avi_term::progress::PhaseProgress`; este módulo solo aporta lo que esa
//! etapa necesita: el observador [`ModelTransfer`], que guarda los bytes
//! descargados y el total que informa `avi-store`, y el texto de la línea viva
//! [`download_live_text`], una función pura para poder fijarlo en pruebas.
//!
//! Cuando `AVI_SETUP_WATCHDOG` está definido, [`ModelTransfer`] envía además al
//! padre un latido interno de bytes por stderr cada cinco segundos, aunque se
//! haya silenciado el progreso visible: es lo que distingue una descarga larga
//! de un proceso colgado. Con `AVI_HANDOVER_HEARTBEAT_FILE` (el proceso es el
//! hijo del traspaso de `self update`), sobrescribe además cada cinco segundos
//! ese fichero con `<hecho> <total>`, y [`beat_stage`] deja una línea al abrir
//! cada etapa: el padre vigila el fichero y solo mata al hijo si deja de cambiar.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use avi_store::TransferObserver;
use avi_term::fmt::bytes as human_bytes;

/// Lo descargado en toda la provisión y cuánto tardó.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProvisionStats {
    pub bytes: u64,
    pub elapsed: Duration,
}

/// Cadencia del latido interno para que el padre vigile descargas largas.
const WATCHDOG_HEARTBEAT: Duration = Duration::from_secs(5);

/// Observador de la transferencia de un modelo: recibe el progreso de
/// `avi-store` y lo deja disponible para la línea viva de la etapa.
pub struct ModelTransfer {
    started: Instant,
    completed: AtomicU64,
    total: AtomicU64,
    /// Último latido entregado, en milisegundos desde `started`, para no emitir
    /// uno por chunk.
    last_beat: AtomicU64,
    /// Hay un watchdog (`AVI_SETUP_WATCHDOG`) que lee los latidos por stderr.
    watchdog: bool,
    /// Fichero de latidos del padre del traspaso de `self update`
    /// (`AVI_HANDOVER_HEARTBEAT_FILE`), si este proceso es el hijo.
    handover_file: Option<PathBuf>,
}

/// Contador de las líneas de latido por apertura de etapa: cada una difiere de
/// la anterior, que es lo que el padre del traspaso interpreta como señal de vida.
static STAGE_BEATS: AtomicU64 = AtomicU64::new(0);

/// Fichero de latidos del traspaso, si este proceso es el hijo de `self update`.
fn handover_heartbeat_file() -> Option<PathBuf> {
    std::env::var_os(crate::update::HANDOVER_HEARTBEAT_FILE_ENV).map(PathBuf::from)
}

/// Sobrescribe el fichero de latidos del padre con `line`. Mejor esfuerzo: un
/// latido perdido solo retrasa la señal, y el siguiente la repone.
fn write_handover_beat(file: &Path, line: &str) {
    #[expect(
        clippy::let_underscore_must_use,
        reason = "un latido perdido solo retrasa la señal de vida; el siguiente la repone"
    )]
    let _ = std::fs::write(file, line);
}

/// Deja una señal de vida en el fichero de latidos del padre al abrir una etapa,
/// para que los pasos sin descarga también la den. Sin traspaso no hace nada.
pub(crate) fn beat_stage() {
    if let Some(file) = handover_heartbeat_file() {
        let n = STAGE_BEATS.fetch_add(1, Ordering::Relaxed) + 1;
        write_handover_beat(&file, &format!("etapa {n}"));
    }
}

impl ModelTransfer {
    /// Crea el observador de un modelo que empieza a descargarse ahora.
    pub fn new() -> Self {
        Self {
            started: Instant::now(),
            completed: AtomicU64::new(0),
            total: AtomicU64::new(0),
            last_beat: AtomicU64::new(0),
            watchdog: std::env::var_os("AVI_SETUP_WATCHDOG").is_some(),
            handover_file: handover_heartbeat_file(),
        }
    }

    /// Bytes descargados y total informado hasta ahora (`0` si aún se desconoce).
    pub fn snapshot(&self) -> (u64, u64) {
        (
            self.completed.load(Ordering::Relaxed),
            self.total.load(Ordering::Relaxed),
        )
    }
}

impl Default for ModelTransfer {
    fn default() -> Self {
        Self::new()
    }
}

impl TransferObserver for ModelTransfer {
    fn heartbeat(&self, completed: u64, total: u64) {
        if !self.watchdog && self.handover_file.is_none() {
            return;
        }
        let now = self.started.elapsed().as_millis() as u64;
        let previous = self.last_beat.load(Ordering::Relaxed);
        if now.saturating_sub(previous) >= WATCHDOG_HEARTBEAT.as_millis() as u64
            && self
                .last_beat
                .compare_exchange(previous, now, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
        {
            if self.watchdog {
                eprintln!("AVI_SETUP_HEARTBEAT {completed} {total}");
            }
            if let Some(file) = &self.handover_file {
                write_handover_beat(file, &format!("{completed} {total}"));
            }
        }
    }

    fn update(&self, completed: u64, total: u64) {
        self.completed.store(completed, Ordering::Relaxed);
        self.total.store(total, Ordering::Relaxed);
    }
}

/// Velocidad media de `bytes` en `elapsed`, en bytes por segundo. Se calcula
/// sobre al menos un milisegundo, para que una descarga instantánea no divida
/// por cero.
pub fn bytes_per_second(bytes: u64, elapsed: Duration) -> u64 {
    (bytes as f64 / elapsed.as_secs_f64().max(0.001)) as u64
}

/// Texto de la línea viva de la descarga de un modelo:
/// `<nombre> · <pct> % · <hecho>/<total> · <velocidad>/s`; sin total conocido,
/// `<nombre> · <hecho> · <velocidad>/s`. La velocidad es la media del modelo.
pub fn download_live_text(name: &str, completed: u64, total: u64, elapsed: Duration) -> String {
    let speed = human_bytes(bytes_per_second(completed, elapsed));
    if total == 0 {
        return format!("{name} · {} · {speed}/s", human_bytes(completed));
    }
    let percent = (completed.min(total) as u128 * 100 / total as u128) as u64;
    format!(
        "{name} · {percent} % · {}/{} · {speed}/s",
        human_bytes(completed),
        human_bytes(total)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watchdog_heartbeat_survives_disabled_visible_progress() {
        const CHILD: &str = "AVI_TEST_WATCHDOG_PROGRESS_CHILD";
        if std::env::var_os(CHILD).is_some() {
            let mut transfer = ModelTransfer::new();
            transfer.started = Instant::now() - Duration::from_secs(6);
            transfer.heartbeat(1, 10);
            transfer.heartbeat(2, 10);
            return;
        }
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "progress::tests::watchdog_heartbeat_survives_disabled_visible_progress",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env("AVI_SETUP_WATCHDOG", "1")
            .env("AVI_NO_PROGRESS", "1")
            .output()
            .expect("se ejecuta el hijo de la prueba");
        assert!(output.status.success(), "{:?}", output.status);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("AVI_SETUP_HEARTBEAT 1 10"), "{stderr}");
        assert_eq!(
            stderr.matches("AVI_SETUP_HEARTBEAT ").count(),
            1,
            "{stderr}"
        );
    }

    #[test]
    fn update_is_visible_through_snapshot() {
        let transfer = ModelTransfer::new();
        assert_eq!(transfer.snapshot(), (0, 0));
        transfer.update(5, 20);
        assert_eq!(transfer.snapshot(), (5, 20));
    }

    #[test]
    fn download_live_text_shows_percent_sizes_and_speed() {
        assert_eq!(
            download_live_text(
                "qwen3-tts-0.6b",
                1_000_000_000,
                2_500_000_000,
                Duration::from_secs(20)
            ),
            "qwen3-tts-0.6b · 40 % · 1,0 GB/2,5 GB · 50,0 MB/s"
        );
    }

    #[test]
    fn download_live_text_without_total_has_no_percent() {
        assert_eq!(
            download_live_text("m", 1024, 0, Duration::from_secs(1)),
            "m · 1,0 KB · 1,0 KB/s",
            "un total desconocido no inventa denominador"
        );
    }
}
