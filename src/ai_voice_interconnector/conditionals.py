"""
Cómputo, precomputación y carga de conditionals de Chatterbox.

Colaborador extraído de `engine.py`
para aislar la preparación de conditionals del estado interno del engine
(`self._tts`, `self.compute_backend`) y hacerla testeable por inyección,
sin cargar el modelo real. Las funciones reciben `tts`/`compute_backend` como
argumentos y solo asignan `tts.conds` desde el llamador (el engine).
"""

import os

from .timing import log

# Adv. emocional del T3Cond: parámetro propio de la arquitectura T3, sin
# relación con EXAGGERATION (que ajusta tts.generate()). Se deja fijo en el
# valor neutro del modelo base; espejo de ChatterboxEngine.EMOTION_ADV.
EMOTION_ADV = 0.5


class ConditionalsPreparer:
    """Prepara los conditionals del modelo con fuentes de audio separadas.

    Inyectable en `ChatterboxEngine`: en producción se instancia por defecto,
    y en tests se puede sustituir por un doble.
    """

    def compute(self, tts, compute_backend, timbre_reference_path, speech_reference_path):
        """
        Computa los conditionals con fuentes de audio separadas.

        - timbre_reference_path: audio para el Voice Encoder (usa el audio COMPLETO para el embedding de timbre)
        - speech_reference_path: audio para el conditioning del T3 (6s) + decoder S3Gen (10s)

        Si timbre_reference_path es None, usa speech_reference_path para todo (el
        conditioning del T3 reutiliza ese mismo buffer de habla).
        """
        import librosa
        from chatterbox.mtl_tts import Conditionals, T3Cond

        # --- Carga el audio UNA vez a 24kHz ---
        ref_24k_speech, _ = librosa.load(speech_reference_path, sr=24000)

        # --- Voice Encoder: usa el audio completo para el timbre ---
        if timbre_reference_path:
            ref_24k_voice, _ = librosa.load(timbre_reference_path, sr=24000)
            ref_16k_voice = librosa.resample(ref_24k_voice, orig_sr=24000, target_sr=16000)
            # El conditioning del T3 solo consume ENC_COND_LEN muestras a 16k:
            # se recorta a 24k antes de resamplear en vez de bajar el audio completo.
            head_24k = ref_24k_speech[: tts.ENC_COND_LEN * 24000 // 16000]
            ref_16k_speech = librosa.resample(head_24k, orig_sr=24000, target_sr=16000)
        else:
            # Sin timbre_reference, el timbre exige el audio completo a 16k; el
            # conditioning del T3 reutiliza ese mismo buffer.
            ref_16k_speech = librosa.resample(ref_24k_speech, orig_sr=24000, target_sr=16000)
            ref_16k_voice = ref_16k_speech

        import torch

        ve_embed = torch.from_numpy(
            tts.ve.embeds_from_wavs([ref_16k_voice], sample_rate=16000)
        )
        ve_embed = ve_embed.mean(axis=0, keepdim=True).to(compute_backend)

        # --- Audio de habla: conditioning del T3 (6s) + decoder S3Gen (10s) ---
        # Referencia del decoder S3Gen (primeros 10s)
        ref_24k_speech = ref_24k_speech[:tts.DEC_COND_LEN]
        s3gen_ref_dict = tts.s3gen.embed_ref(ref_24k_speech, 24000, device=compute_backend)

        # Tokens de conditioning de habla del T3 (primeros 6s)
        t3_cond_prompt_tokens = None
        if plen := tts.t3.hp.speech_cond_prompt_len:
            s3_tokzr = tts.s3gen.tokenizer
            t3_cond_prompt_tokens, _ = s3_tokzr.forward(
                [ref_16k_speech[:tts.ENC_COND_LEN]], max_len=plen
            )
            t3_cond_prompt_tokens = torch.atleast_2d(t3_cond_prompt_tokens).to(compute_backend)

        t3_cond = T3Cond(
            speaker_emb=ve_embed,
            cond_prompt_speech_tokens=t3_cond_prompt_tokens,
            emotion_adv=EMOTION_ADV * torch.ones(1, 1, 1),
        ).to(device=compute_backend)

        return Conditionals(t3_cond, s3gen_ref_dict)

    def load_precomputed(self, voice_dir: str, compute_backend: str):
        """
        Carga los conditionals precomputados del directorio de la voz si existen.

        Devuelve los conds cargados, o None si no existe `conditionals.pt` o si
        la carga falla. NO muta `tts`: el llamador asigna `tts.conds`.
        """
        conds_path = os.path.join(voice_dir, "conditionals.pt")
        if not os.path.exists(conds_path):
            return None

        try:
            import torch
            from chatterbox.mtl_tts import Conditionals

            conds = Conditionals.load(conds_path, map_location=torch.device(compute_backend))
            return conds.to(compute_backend)
        except Exception as e:
            log(f"   -> No se pudieron cargar los conditionals precomputados ({conds_path}): {e}")
            return None

    def precompute_and_save(
        self,
        voice_dir: str,
        timbre_reference: str,
        speech_reference: str,
        tts,
        compute_backend: str,
    ):
        """
        Precomputa los conditionals desde los archivos de audio y los guarda a disco.

        Reutiliza el cómputo único de `compute` y crea un archivo
        `conditionals.pt` en el directorio de la voz.
        """
        conds = self.compute(tts, compute_backend, timbre_reference, speech_reference)
        conds.save(os.path.join(voice_dir, "conditionals.pt"))
