"""
Orquestación del flujo de síntesis.

`SynthesisOrchestrator` es dueño del flujo `synthesize` y del ciclo de vida de
`engine._active_progress_cb`, sacando la orquestación stateful del God object
`ChatterboxEngine` (que queda como façade / composition root). Replica el cuerpo
de `ChatterboxEngine.synthesize`, leyendo el estado del engine y delegando a los
colaboradores `conditionals_prep` y `audio_writer`.
"""

import logging
import os

from .timing import StageTimer, SynthesisMetrics, SynthesisResult, log

logger = logging.getLogger(__name__)


class SynthesisOrchestrator:
    """Orquesta una síntesis: conditionals → generate → encode → (save)."""

    def __init__(self, engine, conditionals_prep, audio_writer):
        """Guarda las referencias a sus colaboradores.

        Lee de `engine` en tiempo de ejecución (no copia): `_tts`,
        `compute_backend`, `EXAGGERATION`, `_conds_cache_key`,
        `load_precomputed_conditionals`, `_emit_progress`, `_active_progress_cb`.
        """
        self.engine = engine
        self.conditionals_prep = conditionals_prep
        self.audio_writer = audio_writer

    def synthesize(
        self,
        text: str,
        timbre_reference,
        speech_reference,
        progress_callback,
        exaggeration=None,
        cfg_weight=None,
        temperature=None,
    ) -> SynthesisResult:
        """Ejecuta la síntesis completa y retorna el audio + las métricas.

        Fija `engine._active_progress_cb` por la duración de esta síntesis y lo
        limpia en `finally` (lo leen el shim de tokens y los wrappers timed_t3/
        timed_s3gen). El llamador serializa la síntesis, así que un único slot
        basta y no cruza callbacks entre invocaciones.

        `exaggeration`/`cfg_weight`/`temperature`: overrides opcionales; si son
        None se usa el default de SYNTHESIS_DEFAULTS para el idioma del engine.
        """
        engine = self.engine
        engine._active_progress_cb = progress_callback
        try:
            return self._synthesize_impl(
                text,
                timbre_reference,
                speech_reference,
                exaggeration=exaggeration,
                cfg_weight=cfg_weight,
                temperature=temperature,
            )
        finally:
            engine._active_progress_cb = None

    def _synthesize_impl(
        self,
        text,
        timbre_reference,
        speech_reference,
        exaggeration=None,
        cfg_weight=None,
        temperature=None,
    ) -> SynthesisResult:
        engine = self.engine

        # Stage 1: Carga de conditionals
        engine._emit_progress(stage="conditionals")
        with StageTimer("1-Speech", "Etapa 1/4: Cargando conditionals"):
            voice_dir = None
            if speech_reference:
                voice_dir = os.path.dirname(speech_reference)
                conditionals_path = os.path.join(voice_dir, "conditionals.pt")
                if os.path.exists(conditionals_path):
                    conds_key = (voice_dir, os.path.getmtime(conditionals_path))
                    if (
                        getattr(engine, "_conds_cache_key", None) == conds_key
                        and getattr(engine._tts, "conds", None) is not None
                    ):
                        # Misma voz consecutiva: los conds ya están en memoria
                        log("   -> Conditionals precomputados (en memoria, sin lectura de disco)")
                    elif engine.load_precomputed_conditionals(voice_dir):
                        engine._conds_cache_key = conds_key
                        log("   -> Conditionals precomputados cargados")
                    else:
                        # conditionals.pt ilegible: degradar al cómputo on-the-fly
                        # en vez de sintetizar en silencio con los conds previos.
                        log("   -> conditionals.pt inválido, recomputando on-the-fly...")
                        engine._conds_cache_key = None
                        self._compute_conditionals(timbre_reference, speech_reference)
                else:
                    log("   -> Calculando conditionals on-the-fly...")
                    engine._conds_cache_key = None
                    self._compute_conditionals(timbre_reference, speech_reference)
            else:
                engine._conds_cache_key = None
                self._compute_conditionals(timbre_reference, speech_reference)

        # Stage 2: Generación TTS. Se ramifica por idioma: es-mx-latam exige
        # language_id="es" (ChatterboxMultilingualTTS.generate), en no acepta
        # ese parámetro (ChatterboxTTS.generate no lo declara). Los tres
        # parámetros de síntesis se resuelven contra SYNTHESIS_DEFAULTS de la
        # ruta activa, salvo override explícito (no None).
        language = getattr(engine, "_language", "es-mx-latam")
        defaults = engine.SYNTHESIS_DEFAULTS.get(
            language, engine.SYNTHESIS_DEFAULTS["es-mx-latam"]
        )
        resolved_exaggeration = exaggeration if exaggeration is not None else defaults["exaggeration"]
        resolved_cfg_weight = cfg_weight if cfg_weight is not None else defaults["cfg_weight"]
        resolved_temperature = temperature if temperature is not None else defaults["temperature"]

        engine._emit_progress(stage="tts")
        with StageTimer("2-Speech", "Etapa 2/4: Generando audio (TTS)"):
            generate_kwargs = dict(
                exaggeration=resolved_exaggeration,
                cfg_weight=resolved_cfg_weight,
                temperature=resolved_temperature,
            )
            if language != "en":
                generate_kwargs["language_id"] = "es"
            wav = engine._tts.generate(text, **generate_kwargs)

        # Stage 3: Conversión a WAV
        engine._emit_progress(stage="encoding")
        with StageTimer("3-Speech", "Etapa 3/4: Convirtiendo a WAV"):
            sample_rate = getattr(engine._tts, 'sr', 24000)
            wav_bytes = self.audio_writer.write(wav, sample_rate)

        # Métricas tipadas publicadas por el engine (SynthesisMetrics), pobladas
        # por los wrappers timed_t3/timed_s3gen en engine._apply_synthesis_optimizations.
        # Un engine de prueba minimal (sin esa instrumentación) no las tiene: se
        # degrada a métricas vacías en vez de fallar.
        metrics = getattr(engine, "_synthesis_metrics", None) or SynthesisMetrics()
        return SynthesisResult(audio_bytes=wav_bytes, metrics=metrics)

    def _compute_conditionals(self, timbre_reference, speech_reference) -> None:
        """Computa los conditionals on-the-fly y los asigna a `engine._tts.conds`.

        Espejo del antiguo `_prepare_conditionals_multi` del engine.
        """
        engine = self.engine
        engine._tts.conds = self.conditionals_prep.compute(
            engine._tts, engine.compute_backend, timbre_reference, speech_reference
        )
