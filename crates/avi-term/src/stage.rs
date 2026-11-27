// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! Catálogo único de etapas de trabajo: el texto con el que una etapa se anima
//! mientras corre, su nombre de columna al terminar y si es transitoria.

/// Una etapa de trabajo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stage {
    /// Identificador de la etapa. Dos etapas con el mismo `label` son la misma
    /// etapa para el indicador: no reinician su reloj.
    pub label: &'static str,
    /// Texto mientras la etapa corre.
    pub active: &'static str,
    /// Nombre de la etapa en su columna al terminar (`Grabación`, `Síntesis`…).
    pub name: &'static str,
    /// Etapa transitoria: no deja línea al terminar ni se numera, salvo que el
    /// recorrido declarado del comando la incluya.
    pub transient: bool,
}

/// Etapa transitoria.
const fn transient(label: &'static str, active: &'static str, name: &'static str) -> Stage {
    Stage {
        label,
        active,
        name,
        transient: true,
    }
}

/// Etapa permanente.
const fn permanent(label: &'static str, active: &'static str, name: &'static str) -> Stage {
    Stage {
        label,
        active,
        name,
        transient: false,
    }
}

pub const VALIDATING: Stage = transient("validating", "Validando…", "Validación");
pub const STARTED: Stage = transient("started", "Enviando al daemon…", "Envío");
pub const DENOISE_QUEUE: Stage = transient(
    "denoise_queue",
    "Esperando turno para limpiar el ruido…",
    "Espera de limpieza",
);
pub const SAVING: Stage = transient("saving", "Guardando…", "Guardado");
pub const CHECKING: Stage = transient("checking", "Buscando actualizaciones…", "Comprobación");
pub const READING: Stage = permanent("reading", "Leyendo el audio…", "Lectura");
pub const CAPTURING: Stage = permanent("capturing", "Grabando…", "Grabación");
pub const DENOISING: Stage = permanent("denoising", "Limpiando el ruido…", "Limpieza");
pub const TRANSCRIBING: Stage = permanent("transcribing", "Transcribiendo…", "Transcripción");
pub const TRANSLATING: Stage = permanent("translating", "Traduciendo…", "Traducción");
pub const SYNTHESIZING: Stage = permanent("synthesizing", "Sintetizando…", "Síntesis");
/// Textos activos de `SYNTHESIZING` mientras la síntesis espera: no son etapas
/// propias, así que la espera comparte el reloj de la síntesis y nunca deja una
/// línea terminada.
pub const SYNTHESIS_QUEUED_TEXT: &str = "Esperando turno para sintetizar…";
pub const SYNTHESIS_WARMING_TEXT: &str = "Calentando el motor…";
pub const SYNTHESIS_LOADING_VOICE_TEXT: &str = "Cargando la voz…";
pub const PLAYING: Stage = permanent("playing", "Reproduciendo…", "Reproducción");
pub const CLONING: Stage = permanent(
    "cloning",
    "Clonando la voz… (puede tardar varios minutos)",
    "Clonación",
);
pub const WARMING: Stage = permanent("warming", "Calentando el motor…", "Calentamiento");
pub const REMOVING: Stage = permanent("removing", "Borrando…", "Borrado");
/// Transitoria: los recorridos de `daemon` la declaran; `self update` la renombra con `done_as`.
pub const STOPPING: Stage = transient("stopping", "Deteniendo el daemon…", "Parada del daemon");
pub const STARTING: Stage = permanent("starting", "Iniciando el daemon…", "Arranque");
pub const DOWNLOADING: Stage = permanent("downloading", "Descargando…", "Descarga");
pub const VERIFYING: Stage = permanent("verifying", "Verificando la descarga…", "Verificación");
pub const REPLACING: Stage = permanent("replacing", "Reemplazando el programa…", "Reemplazo");
pub const COPYING: Stage = permanent("copying", "Copiando el programa…", "Copia");
pub const PATH_ADD: Stage = permanent("path", "Integrando en el PATH…", "Integración en el PATH");
pub const PATH_REVERT: Stage = permanent("path", "Revirtiendo el PATH…", "Reversión del PATH");
pub const RECEIPT: Stage = permanent("receipt", "Escribiendo el recibo…", "Recibo");
pub const STATE: Stage = permanent("state", "Borrando el estado…", "Estado");
pub const PROGRAM: Stage = permanent("program", "Borrando el programa…", "Programa");
pub const LOCKS: Stage = permanent("locks", "Borrando los archivos de bloqueo…", "Bloqueos");
pub const CLEANING: Stage = permanent("cleaning", "Borrando los datos…", "Limpieza de datos");
pub const REPAIRING: Stage = permanent("repairing", "Reparando los restos…", "Reparación");

#[cfg(test)]
mod tests {
    use super::*;

    /// Todas las etapas del catálogo.
    fn all() -> [Stage; 29] {
        [
            VALIDATING,
            STARTED,
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
        ]
    }

    /// Ningún texto de etapa nombra al daemon salvo las etapas que hablan de
    /// él por definición: el catálogo describe la operación, no la ruta.
    #[test]
    fn only_daemon_stages_mention_the_daemon() {
        for stage in all() {
            let mentions = stage.active.contains("daemon") || stage.name.contains("daemon");
            let allowed = matches!(stage.label, "started" | "stopping" | "starting");
            assert!(!mentions || allowed, "{} nombra al daemon", stage.label);
        }
        for text in [
            SYNTHESIS_QUEUED_TEXT,
            SYNTHESIS_WARMING_TEXT,
            SYNTHESIS_LOADING_VOICE_TEXT,
        ] {
            assert!(!text.contains("daemon"), "{text} nombra al daemon");
        }
    }

    /// Las dos etapas del PATH comparten identificador.
    #[test]
    fn path_stages_share_a_label() {
        assert_eq!(PATH_ADD.label, PATH_REVERT.label);
    }

    /// Solo las seis etapas transitorias del catálogo lo son, y todas tienen
    /// un nombre de columna.
    #[test]
    fn catalog_transient_flags_and_names() {
        let transients: Vec<&str> = all()
            .iter()
            .filter(|stage| stage.transient)
            .map(|stage| stage.label)
            .collect();
        assert_eq!(
            transients,
            [
                "validating",
                "started",
                "denoise_queue",
                "saving",
                "checking",
                "stopping"
            ]
        );
        assert!(all().iter().all(|stage| !stage.name.is_empty()));
    }
}
