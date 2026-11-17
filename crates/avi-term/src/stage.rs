// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! Catálogo único de etapas de trabajo: el texto con el que una etapa se anima
//! mientras corre y el texto con el que queda fija al terminar.

/// Una etapa de trabajo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stage {
    /// Identificador de la etapa. Dos etapas con el mismo `label` son la misma
    /// etapa para el indicador: no reinician su reloj.
    pub label: &'static str,
    /// Texto mientras la etapa corre.
    pub active: &'static str,
    /// Texto de la línea terminada. `None` significa etapa transitoria: no deja
    /// línea al terminar.
    pub done: Option<&'static str>,
}

/// Etapa transitoria.
const fn transient(label: &'static str, active: &'static str) -> Stage {
    Stage {
        label,
        active,
        done: None,
    }
}

/// Etapa permanente.
const fn permanent(label: &'static str, active: &'static str, done: &'static str) -> Stage {
    Stage {
        label,
        active,
        done: Some(done),
    }
}

pub const VALIDATING: Stage = transient("validating", "Validando…");
pub const STARTED: Stage = transient("started", "Enviando al daemon…");
pub const QUEUED: Stage = transient("queued", "Esperando turno para sintetizar…");
pub const DENOISE_QUEUE: Stage =
    transient("denoise_queue", "Esperando turno para limpiar el ruido…");
pub const SAVING: Stage = transient("saving", "Guardando…");
pub const CHECKING: Stage = transient("checking", "Buscando actualizaciones…");
pub const READING: Stage = permanent("reading", "Leyendo el audio…", "Audio leído");
pub const CAPTURING: Stage = permanent("capturing", "Grabando…", "Audio grabado");
pub const DENOISING: Stage = permanent("denoising", "Limpiando el ruido…", "Ruido limpiado");
pub const TRANSCRIBING: Stage = permanent("transcribing", "Transcribiendo…", "Transcrito");
pub const TRANSLATING: Stage = permanent("translating", "Traduciendo…", "Traducido");
/// El llamador cambia el texto terminado con `done_as("Primer audio")` si
/// después se reproduce.
pub const SYNTHESIZING: Stage = permanent("synthesizing", "Sintetizando…", "Voz sintetizada");
pub const PLAYING: Stage = permanent("playing", "Reproduciendo…", "Reproducido");
pub const CLONING: Stage = permanent(
    "cloning",
    "Clonando la voz… (puede tardar varios minutos)",
    "Voz clonada",
);
pub const WARMING: Stage = permanent("warming", "Calentando el motor…", "Motor calentado");
pub const WARMUP: Stage = permanent("warmup", "Precargando la voz…", "Voz precargada");
pub const REMOVING: Stage = permanent("removing", "Borrando…", "Borrado");
/// Transitoria: el llamador la hace permanente con `done_as("Daemon detenido")`
/// solo si se detuvo un daemon.
pub const STOPPING: Stage = transient("stopping", "Deteniendo el daemon…");
pub const STARTING: Stage = permanent("starting", "Iniciando el daemon…", "Daemon iniciado");
pub const DOWNLOADING: Stage = permanent("downloading", "Descargando…", "Descargado");
pub const VERIFYING: Stage = permanent(
    "verifying",
    "Verificando la descarga…",
    "Descarga verificada",
);
pub const REPLACING: Stage = permanent(
    "replacing",
    "Reemplazando el programa…",
    "Programa reemplazado",
);
pub const COPYING: Stage = permanent("copying", "Copiando el programa…", "Programa copiado");
pub const PATH_ADD: Stage = permanent("path", "Integrando en el PATH…", "PATH integrado");
pub const PATH_REVERT: Stage = permanent("path", "Revirtiendo el PATH…", "PATH revertido");
pub const RECEIPT: Stage = permanent("receipt", "Escribiendo el recibo…", "Recibo escrito");
pub const STATE: Stage = permanent("state", "Borrando el estado…", "Estado borrado");
pub const PROGRAM: Stage = permanent("program", "Borrando el programa…", "Programa borrado");
pub const LOCKS: Stage = permanent(
    "locks",
    "Borrando los archivos de bloqueo…",
    "Bloqueos borrados",
);
pub const CLEANING: Stage = permanent("cleaning", "Borrando los datos…", "Datos borrados");
pub const REPAIRING: Stage = permanent("repairing", "Reparando los restos…", "Restos reparados");

#[cfg(test)]
mod tests {
    use super::*;

    /// Ningún texto de etapa nombra al daemon salvo las etapas que hablan de
    /// él por definición: el catálogo describe la operación, no la ruta.
    #[test]
    fn only_daemon_stages_mention_the_daemon() {
        let all = [
            VALIDATING,
            STARTED,
            QUEUED,
            DENOISE_QUEUE,
            SAVING,
            CHECKING,
            READING,
            CAPTURING,
            DENOISING,
            TRANSCRIBING,
            TRANSLATING,
            SYNTHESIZING,
            PLAYING,
            CLONING,
            WARMING,
            WARMUP,
            REMOVING,
            STOPPING,
            STARTING,
            DOWNLOADING,
            VERIFYING,
            REPLACING,
            COPYING,
            PATH_ADD,
            PATH_REVERT,
            RECEIPT,
            STATE,
            PROGRAM,
            LOCKS,
            CLEANING,
            REPAIRING,
        ];
        for stage in all {
            let mentions =
                stage.active.contains("daemon") || stage.done.is_some_and(|d| d.contains("daemon"));
            let allowed = matches!(stage.label, "started" | "stopping" | "starting");
            assert!(!mentions || allowed, "{} nombra al daemon", stage.label);
        }
    }

    /// Las dos etapas del PATH comparten identificador.
    #[test]
    fn path_stages_share_a_label() {
        assert_eq!(PATH_ADD.label, PATH_REVERT.label);
    }
}
