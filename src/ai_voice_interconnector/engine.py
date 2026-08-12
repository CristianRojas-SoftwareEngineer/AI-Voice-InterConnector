"""
Motor TTS de Chatterbox.
Wrapper de Chatterbox Multilingual TTS desde HuggingFace.
Soporta el language pack es-mx-latam vía un loader propio.

Arquitectura: T3 (autoregresivo, ~0.4B params) + S3Gen (vocoder de flow matching).
Parámetros de síntesis optimizados, propios del engine (aplican en modo directo
y en el daemon por igual):
  - max_new_tokens=500  (tope de output del T3; el default es 1000)
  - n_cfm_timesteps=4  (pasos de flow matching; el default es 10)
  - exaggeration=0.75   (expresividad emocional; el default es 0.5)
  - bypass del watermark PerthNet: el audio generado NO lleva marca de agua.
    Esto implica que no es distinguible técnicamente de una grabación real y
    traslada al usuario la responsabilidad del uso legítimo (ver «Uso ético y
    responsable» en README.md/USAGE.md).

Optimizaciones multiplataforma para Windows, Linux y Mac.
"""

import os

# Desactiva las barras tqdm internas de Chatterbox (T3 "Sampling", flow matching
# del S3Gen) ANTES de importar chatterbox: ai-voice-interconnector muestra su propio indicador
# de liveness (timing.Spinner) en el proceso cliente, y dos indicadores sobre el
# mismo stderr colisionarían. En el proceso daemon el tqdm ya era invisible (corre
# detached), así que no se pierde nada. setdefault respeta un valor externo previo.
os.environ.setdefault("TQDM_DISABLE", "1")

import logging
import platform
import threading
from pathlib import Path
from typing import Callable, Optional

import torch

from . import voices
from .model_cache import (
    BASE_MODEL_REVISION,
    MODEL_REVISIONS,
    MODEL_ALLOW_PATTERNS,
    MODELS as _MODEL_ALIASES,
    _resolve_cached_snapshot,
    cache_folder_for,
    hub_cache_path,
    is_model_cached,  # re-export para compatibilidad interna
)
from .timing import StageTimer, SynthesisMetrics, SynthesisResult, log
from .model_loader import ModelLoader
from .conditionals import ConditionalsPreparer
from .compute_backend import ComputeBackendResolver
from .audio_writer import AudioWriter
from .synthesis import SynthesisOrchestrator
from .exceptions import SynthesisCancelled

logger = logging.getLogger(__name__)

# =============================================================================
# Configuración de PyTorch según la plataforma para inferencia en CPU
# =============================================================================

def _configure_torch_for_platform():
    """Configura los ajustes de PyTorch según la plataforma actual."""
    system = platform.system()

    # Determina el número óptimo de hilos según los núcleos de CPU
    cpu_count = os.cpu_count() or 4
    # Usa hasta 8 hilos: más allá, el ancho de banda de memoria se vuelve el cuello
    # de botella para modelos transformer en CPU (más hilos = peor por contención).
    optimal_threads = min(8, cpu_count)

    torch.set_num_threads(optimal_threads)

    if system == "Windows":
        # Windows: Intel MKL suele estar disponible vía PyTorch
        os.environ["OMP_NUM_THREADS"] = str(optimal_threads)
        os.environ["MKL_NUM_THREADS"] = str(optimal_threads)
        # Habilita el fallback de MPS (inofensivo en Windows)
        os.environ["PYTORCH_ENABLE_MPS_FALLBACK"] = "1"

    elif system == "Linux":
        # Linux: OpenMP vía libgomp, potencialmente MKL
        os.environ["OMP_NUM_THREADS"] = str(optimal_threads)
        os.environ["MKL_NUM_THREADS"] = str(optimal_threads)
        # Intenta habilitar oneDNN para mejor rendimiento en CPU
        try:
            torch.backends.mkldnn.enabled = True
        except Exception:
            logger.debug("No se pudo habilitar oneDNN (mkldnn) en Linux", exc_info=True)

    elif system == "Darwin":
        # Mac: framework Accelerate (nativo)
        os.environ["OMP_NUM_THREADS"] = str(optimal_threads)
        # Fallback de MPS para Macs con Apple Silicon
        os.environ["PYTORCH_ENABLE_MPS_FALLBACK"] = "1"

    # Habilita MKLDNN si está disponible (las CPU Intel/AMD se benefician)
    try:
        torch.backends.mkldnn.enabled = True
        torch.backends.mkldnn.verbose(0)
    except Exception:
        logger.debug("MKL-DNN no disponible; se omite la optimización", exc_info=True)

    # Desactiva el flushing de denormales para cómputo más rápido en CPU modernas
    try:
        torch.set_flush_denormal(True)
    except Exception:
        logger.debug("torch.set_flush_denormal no disponible; se omite", exc_info=True)

    return optimal_threads


# Aplica la configuración de plataforma al importar el módulo
_THREAD_COUNT = _configure_torch_for_platform()
del _configure_torch_for_platform  # Mantiene limpio el namespace


from safetensors.torch import load_file

# Imports de Chatterbox
try:
    from chatterbox.tts import (
        ChatterboxTTS, T3, S3Gen, VoiceEncoder,
        Conditionals, EnTokenizer,
    )
    from chatterbox.mtl_tts import (
        ChatterboxMultilingualTTS, MTLTokenizer, T3Config, T3Cond,
    )
    from chatterbox.models.s3tokenizer import S3_SR
except ImportError:
    raise ImportError(
        "Chatterbox not installed. Run: pip install chatterbox-tts"
    )


class ChatterboxEngine:
    """
    Motor TTS que usa los modelos de Chatterbox.

    Modelo por defecto: language pack es-mx-latam.
    Usa la arquitectura ChatterboxMultilingualTTS con vocabulario de 2454 tokens
    para coincidir con el checkpoint es-mx-latam (text_emb: 2454×1024).

    Soporta caché de modelos para evitar recargarlos en cada instanciación.
    """

    # Configuraciones de modelo (mapa único en model_cache)
    MODELS = _MODEL_ALIASES

    # Parámetros de síntesis optimizados (compartidos por modo directo y daemon)
    MAX_NEW_TOKENS = 500     # tope de output del T3 (default del modelo: 1000)
    N_CFM_TIMESTEPS = 4      # pasos de flow matching (default: 10; 4 es ~2.5x más rápido)
    EXAGGERATION = 0.75      # expresividad emocional (default: 0.5)
    # Adv. emocional del T3Cond: parámetro propio de la arquitectura T3, sin
    # relación con EXAGGERATION (que ajusta tts.generate()). Se deja fijo en
    # el valor neutro del modelo base; no es un default de Chatterbox a heredar.
    EMOTION_ADV = 0.5

    # Defaults de síntesis por ruta (rediseño cross-lingual, §3.6 de
    # docs/proposals/cli-redesign.md): NO se comparten entre rutas, la terna
    # del inglés no está validada para español. Keyed por el alias de idioma
    # recibido en `model` (self._language), no por el repo id resuelto: es la
    # clave que ya tenemos en mano sin traducciones adicionales. La terna
    # es-mx-latam es exactamente el comportamiento efectivo actual (EXAGGERATION
    # cableado; cfg_weight/temperature heredados de fábrica) — cablearlos aquí
    # no cambia el resultado, solo lo hace controlable vía overrides.
    SYNTHESIS_DEFAULTS = {
        "es-mx-latam": {"exaggeration": 0.75, "cfg_weight": 0.5, "temperature": 0.8},
        "en": {"exaggeration": 0.65, "cfg_weight": 0.3, "temperature": 0.7},
    }

    # Caché a nivel de clase para los modelos cargados (evita recargar en cada synthesize)
    _cache: dict[str, "ChatterboxEngine"] = {}
    _cache_lock = threading.Lock()

    @classmethod
    def get_instance(cls, model: str = "es-mx-latam", compute_backend: str = "auto", models_dir: Optional[str] = None) -> "ChatterboxEngine":
        """
        Obtiene una instancia cacheada del motor o crea una nueva.

        El modelo queda cargado en memoria para las llamadas siguientes.

        Args:
            model: Modelo a usar. El CLI invoca con "es-mx-latam" o "en".
            compute_backend: Backend de cómputo para la inferencia
                ("auto", "cpu", "cuda", "mps"). Con "auto" (default), se
                detecta el mejor disponible.
            models_dir: Directorio donde cachear los modelos
        """
        resolved = ComputeBackendResolver.resolve(compute_backend)
        key = ComputeBackendResolver.cache_key(model, resolved, models_dir)
        with cls._cache_lock:
            if key not in cls._cache:
                cls._cache[key] = cls(model=model, compute_backend=resolved, models_dir=models_dir)
            return cls._cache[key]

    def __init__(
        self,
        model: str = "es-mx-latam",
        compute_backend: str = "auto",
        models_dir: Optional[str] = None,
        model_loader: Optional["ModelLoader"] = None,
        conditionals_prep: Optional["ConditionalsPreparer"] = None,
    ):
        """
        Inicializa el motor TTS de Chatterbox.

        Args:
            model: Modelo a usar. El CLI invoca con "es-mx-latam" o "en".
            compute_backend: Backend de cómputo ("auto", "cpu", "cuda", "mps").
                Con "auto" (default) se detecta el mejor disponible.
            models_dir: Directorio donde cachear los modelos. Default: ~/.cache/huggingface/hub
            model_loader: Colaborador de carga de modelo (inyectable en tests).
                Por defecto se instancia `ModelLoader`.
            conditionals_prep: Colaborador de preparación de conditionals
                (inyectable en tests). Por defecto se instancia `ConditionalsPreparer`.
        """
        self.compute_backend = ComputeBackendResolver.resolve(compute_backend)
        self.model_name = self.MODELS.get(model, model)
        # Alias original recibido (p. ej. "es-mx-latam" o "en"), distinto de
        # model_name (repo id resuelto, usado para descarga/caché): permite
        # elegir la ruta de generate() y los SYNTHESIS_DEFAULTS sin traducciones.
        self._language = model

        # Colaboradores inyectables: por defecto se crean aquí para no alterar el
        # comportamiento en producción; en tests se sustituyen por dobles.
        self._model_loader = model_loader or ModelLoader()
        self._conditionals_prep = conditionals_prep or ConditionalsPreparer()

        # Descarga el modelo a la caché local
        self._cache_dir = self._download_model(self.model_name, models_dir)

        # Carga el modelo desde los archivos locales usando el loader correcto
        self._tts = self._model_loader.load(self._cache_dir, self.model_name, self.compute_backend)

        # Memoización en memoria de los conditionals precomputados: clave
        # (voice_dir, mtime de conditionals.pt) de la última carga exitosa.
        self._conds_cache_key = None

        # Callback de progreso activo por síntesis (Fase 2): synthesize() lo fija al
        # entrar y lo limpia en finally. El shim de tokens del T3 y los wrappers
        # timed_t3/timed_s3gen lo consultan para emitir eventos de sub-etapa.
        # La síntesis está serializada (una a la vez) por el llamador —el daemon
        # con su _synthesis_lock, el modo directo por ser mono-hilo—, así que un
        # único slot es suficiente y no cruza callbacks entre invocaciones.
        self._active_progress_cb: Optional[Callable[[dict], None]] = None

        # Aplica los parámetros de síntesis optimizados y el timing por sub-etapa
        self._apply_synthesis_optimizations()

        # Colaboradores de síntesis: el orquestador es dueño del flujo
        # synthesize y del ciclo de vida de _active_progress_cb; el engine queda como
        # façade / composition root que posee el modelo y los colaboradores.
        self._audio_writer = AudioWriter()
        self._orchestrator = SynthesisOrchestrator(
            self, self._conditionals_prep, self._audio_writer
        )

    def _emit_progress(self, **fields) -> None:
        """Emite un evento de progreso al callback activo, si lo hay (best-effort).

        Un callback que lance no debe abortar la síntesis: se traga la excepción.
        """
        cb = getattr(self, "_active_progress_cb", None)
        if cb is None:
            return
        try:
            cb({"event": "progress", **fields})
        except SynthesisCancelled:
            # Señal cooperativa del daemon: la dejamos propagar para abortar la
            # síntesis. No es un error del callback, sino una petición de
            # cancelación que el worker espera.
            raise
        except Exception:
            logger.debug("El callback de progreso lanzó; se ignora (best-effort)", exc_info=True)

    def _apply_synthesis_optimizations(self):
        """
        Instrumenta el modelo con los parámetros de síntesis optimizados y el
        timing por sub-etapa (antes monkey-patches del daemon en daemon/run.py):

        - t3.inference con max_new_tokens=MAX_NEW_TOKENS y medición de tiempo
        - s3gen.inference con n_cfm_timesteps=N_CFM_TIMESTEPS y medición de tiempo
        - bypass del watermarker PerthNet (segunda red neuronal de post-procesado)

        El timing queda en self._synthesis_metrics (SynthesisMetrics con campos
        t3/s3gen en segundos), que el daemon incluye en el frame `result` del
        stream NDJSON de /synthesize (campos t3_time / s3gen_time). Las sub-etapas
        t3/s3gen (y el conteo de tokens del T3, vía el shim de tqdm) también se
        emiten al progress_callback activo desde estos wrappers.
        """
        import functools
        import time as time_mod

        self._synthesis_metrics = SynthesisMetrics()
        tts = self._tts

        _orig_t3 = tts.t3.inference

        @functools.wraps(_orig_t3)
        def timed_t3(*args, **kwargs):
            kwargs['max_new_tokens'] = self.MAX_NEW_TOKENS
            # Sub-etapa T3: el conteo de tokens ascendente lo emite el shim de
            # tqdm (ver _install_token_progress_shim); aquí solo marcamos la
            # transición de etapa para el caso en que el shim haya degradado.
            self._emit_progress(stage="t3")
            t0 = time_mod.time()
            result = _orig_t3(*args, **kwargs)
            self._synthesis_metrics.t3 = time_mod.time() - t0
            log(f"   [Etapa 2a] T3 autoregresivo: {self._synthesis_metrics.t3:.1f}s")
            # En el modelo multilingüe, el cuerpo de T3.inference reconstruye en
            # CADA llamada un AlignmentStreamAnalyzer que registra un forward_hook
            # por capa alineada sin removerlo nunca. Como el daemon cachea el
            # motor entre peticiones, esos hooks (y las clausuras que retienen los
            # tensores del analizador viejo) se acumularían sin cota. Los vaciamos
            # aquí para acotar su footprint a la síntesis en curso.
            self._clear_alignment_hooks()
            return result

        tts.t3.inference = timed_t3

        _orig_s3gen = tts.s3gen.inference

        @functools.wraps(_orig_s3gen)
        def timed_s3gen(*args, **kwargs):
            kwargs['n_cfm_timesteps'] = self.N_CFM_TIMESTEPS
            # Sub-etapa S3Gen (vocoder): transición tras completar el T3.
            self._emit_progress(stage="s3gen")
            t0 = time_mod.time()
            result = _orig_s3gen(*args, **kwargs)
            self._synthesis_metrics.s3gen = time_mod.time() - t0
            log(f"   [Etapa 2b] S3Gen vocoder:   {self._synthesis_metrics.s3gen:.1f}s")
            return result

        tts.s3gen.inference = timed_s3gen

        # El audio generado NO lleva la marca de agua de PerthNet: este reemplazo
        # neutraliza `apply_watermark`. En consecuencia, el audio producido no es
        # distinguible por medios técnicos de una grabación real, lo que traslada
        # al usuario la responsabilidad del uso legítimo (consentimiento para
        # clonar voces, no suplantación). Ver la sección «Uso ético y responsable»
        # en README.md/USAGE.md. No es una mera optimización de velocidad.
        def noop_watermark(wav, sample_rate=None, **kwargs):
            return wav

        tts.watermarker.apply_watermark = noop_watermark

        # Shim de progreso de tokens del T3 (Fase 2b), best-effort.
        self._install_token_progress_shim()

    def _clear_alignment_hooks(self):
        """Vacía los forward_hooks que el AlignmentStreamAnalyzer del T3 deja
        registrados en las capas alineadas tras cada inferencia multilingüe.

        Solo aplica cuando `t3.hp.is_multilingual` es True: la ruta english_only
        nunca construye el analizador y por tanto nunca registra estos hooks. Las
        capas objetivo se derivan de `LLAMA_ALIGNED_HEADS` (única fuente de verdad
        del paquete), de modo que un cambio futuro del paquete no deje el fix
        desincronizado en silencio.

        Es **best-effort**, como `_install_token_progress_shim`: si el layout del
        paquete difiere de lo esperado (símbolo ausente, atributos distintos tras
        una actualización), degrada en silencio —la síntesis nunca se rompe por
        la limpieza—.
        """
        try:
            t3 = self._tts.t3
            if not t3.hp.is_multilingual:
                return
            from chatterbox.models.t3.inference.alignment_stream_analyzer import (
                LLAMA_ALIGNED_HEADS,
            )

            layers = {layer_idx for layer_idx, _head in LLAMA_ALIGNED_HEADS}
            for idx in layers:
                t3.tfmr.layers[idx].self_attn._forward_hooks.clear()
        except Exception:
            # Layout inesperado: no removemos hooks, pero la síntesis continúa.
            logger.debug(
                "No se pudieron limpiar los forward_hooks del analizador de alineación del T3",
                exc_info=True,
            )

    def _install_token_progress_shim(self):
        """Envuelve el símbolo `tqdm` de `chatterbox.models.t3.t3` para reportar
        el conteo de tokens del bucle 'Sampling' del T3 al callback activo.

        Es un acoplamiento a un detalle interno de Chatterbox (el bucle
        `for i in tqdm(range(max_new_tokens), desc="Sampling", ...)`), del mismo
        tipo que los monkeypatches que este engine ya aplica sobre t3/s3gen. Se
        implementa **best-effort**: si el símbolo no está donde se espera (layout
        distinto tras una actualización del paquete), degrada en silencio a solo
        eventos de etapa —la síntesis nunca se rompe por esto—.
        """
        try:
            from chatterbox.models.t3 import t3 as _t3_mod
            real_tqdm = _t3_mod.tqdm
            # Idempotencia: si ya instalamos el shim (p. ej. un segundo engine en
            # otra cache key), no lo envolvemos de nuevo.
            if getattr(real_tqdm, "_is_ai_voice_interconnector_shim", False):
                return

            engine = self

            def progress_tqdm(iterable=None, *args, **kwargs):
                cb = getattr(engine, "_active_progress_cb", None)
                # Solo interceptamos el bucle de sampling del T3 con callback activo;
                # cualquier otro uso de tqdm delega en el real (tqdm ya está
                # deshabilitado vía TQDM_DISABLE, así que no dibuja nada).
                if cb is None or kwargs.get("desc") != "Sampling" or iterable is None:
                    return real_tqdm(iterable, *args, **kwargs)
                return engine._token_counting_iter(iterable, cb)

            progress_tqdm._is_ai_voice_interconnector_shim = True
            _t3_mod.tqdm = progress_tqdm
        except Exception:
            # Layout inesperado: degradar a solo eventos de etapa.
            logger.debug("No se pudo instalar el shim de tqdm del T3; solo eventos de etapa", exc_info=True)

    @staticmethod
    def _token_counting_iter(iterable, cb):
        """Itera reportando el conteo de tokens al callback, con throttle.

        Emite un evento como mucho cada ~10 tokens y a un máximo de ~10 eventos/s,
        para acotar el volumen del stream. Un callback que lance no interrumpe la
        generación.
        """
        import time as _t

        last_emit = 0.0
        count = 0
        for item in iterable:
            count += 1
            now = _t.time()
            if count % 10 == 0 and (now - last_emit) >= 0.1:
                last_emit = now
                try:
                    cb({"event": "progress", "stage": "t3", "tokens": count})
                except SynthesisCancelled:
                    # Señal cooperativa del daemon: la dejamos propagar para
                    # abortar la síntesis. Véase _emit_progress.
                    raise
                except Exception:
                    logger.debug("El callback de progreso de tokens lanzó; se ignora (best-effort)", exc_info=True)
            yield item

    def _download_model(self, model_name: str, models_dir: Optional[str] = None) -> Path:
        """Descarga el modelo desde HuggingFace o lo obtiene de la caché local."""
        from huggingface_hub import snapshot_download
        import os

        # Revisión fijada del repo: la resolución de carga y la descarga de
        # respaldo honran el mismo pin que 'setup' y la detección de caché, de modo
        # que un bump futuro de MODEL_REVISIONS no puede producir síntesis silenciosa
        # con el modelo viejo que 'refs/main' seguiría prefiriendo. Acepta tanto el
        # alias como el repo id (model_name es el repo id resuelto por self.MODELS).
        revision = MODEL_REVISIONS.get(model_name)
        if revision is None:
            for alias, repo in self.MODELS.items():
                if repo == model_name and alias in MODEL_REVISIONS:
                    revision = MODEL_REVISIONS[alias]
                    break

        # Intenta encontrar un modelo ya cacheado
        cached = _resolve_cached_snapshot(
            hub_cache_path() / cache_folder_for(model_name), revision=revision
        )

        if cached is not None:
            # Verifica que existan los archivos de es-mx-latam
            if model_name == "es-mx-latam" or "es-mx-latam" in model_name:
                if (cached / "t3_es_mx_latam.safetensors").exists():
                    log(f"Usando modelo en caché: {model_name} ({cached})")
                    return cached
            else:
                log(f"Usando modelo en caché: {model_name} ({cached})")
                return cached

        # Descarga desde HuggingFace
        log(f"Descargando {model_name} desde HuggingFace")

        # Resuelve allow_patterns: si model_name es un alias, lo usa directamente;
        # si es un repo_id, intenta resolverlo vía MODELS inverso.
        allow = MODEL_ALLOW_PATTERNS.get(model_name)
        if allow is None:
            for alias, repo in _MODEL_ALIASES.items():
                if repo == model_name:
                    allow = MODEL_ALLOW_PATTERNS.get(alias)
                    break

        cached_path = Path(
            snapshot_download(
                repo_id=model_name,
                repo_type="model",
                revision=revision,
                token=os.getenv("HF_TOKEN"),
                **({"allow_patterns": allow} if allow else {}),
            )
        )
        log(f"Modelo descargado en: {cached_path}")
        return cached_path

    def _load_model(self, cache_dir: Path, model_name: str, compute_backend: str):
        """Carga el modelo según la ruta de caché: es-mx-latam o el multilingüe base.

        Delegado a `ModelLoader` (instanciado por defecto en `__init__` e
        inyectable en tests). El cuerpo vivo está en `model_loader.py`.
        """
        return self._model_loader.load(cache_dir, model_name, compute_backend)

    def synthesize(
        self,
        text: str,
        timbre_reference: Optional[str] = None,
        speech_reference: Optional[str] = None,
        verbose: bool = True,
        progress_callback: Optional[Callable[[dict], None]] = None,
        exaggeration: Optional[float] = None,
        cfg_weight: Optional[float] = None,
        temperature: Optional[float] = None,
    ) -> SynthesisResult:
        """
        Genera audio a partir de texto.

        Args:
            text: Texto a sintetizar (español)
            timbre_reference: Archivo de audio para el Voice Encoder (audio completo para el embedding de timbre)
            speech_reference: Archivo de audio para el conditioning del T3 (6s) + decoder S3Gen (10s).
                         Si es None pero se da timbre_reference, se usa timbre_reference para ambos.
            verbose: Si es True, imprime info de timing por etapa (default True)
            progress_callback: Callback opcional que recibe dicts de progreso
                (`{"event":"progress","stage":...,"tokens":...}`) en cada
                transición de etapa (conditionals → t3 → s3gen → encoding →
                saving) y con el conteo de tokens del T3 en vivo. Es la fuente
                única de progreso, compartida por el modo directo (CLI) y el
                daemon (que la reenvía por el stream NDJSON). Best-effort: una
                excepción del callback no aborta la síntesis.
            exaggeration: Override opcional; si es None, se usa el default de
                SYNTHESIS_DEFAULTS para el idioma cargado en este engine.
            cfg_weight: Override opcional; mismo criterio que exaggeration.
            temperature: Override opcional; mismo criterio que exaggeration.

        Returns:
            `SynthesisResult` (audio_bytes + métricas t3/s3gen).
        """
        if not timbre_reference and not speech_reference:
            raise ValueError(
                "Se requiere al menos un archivo de audio de referencia. "
                "Usa `--voice` para una voz registrada; sin él, la CLI recurre "
                "a la voz de fábrica 'default'."
            )

        # Si solo hay timbre_reference, úsalo para ambos
        if timbre_reference and not speech_reference:
            speech_reference = timbre_reference

        # Façade delgado: el flujo de síntesis y el ciclo de vida de
        # _active_progress_cb viven en el orquestador, no en el engine.
        return self._orchestrator.synthesize(
            text,
            timbre_reference,
            speech_reference,
            progress_callback,
            exaggeration=exaggeration,
            cfg_weight=cfg_weight,
            temperature=temperature,
        )

    def precompute_voice(self, name: str) -> None:
        """
        Precomputa y guarda los conditionals de una voz ya registrada.

        Resuelve las rutas de audio de la voz desde el registro (usuario→fábrica)
        y escribe `conditionals.pt` en su directorio, para que las síntesis
        posteriores los carguen desde disco en lugar de recomputarlos. No copia
        audios: el registro de la voz es responsabilidad de `voices.clone_voice_files`.

        Args:
            name: Nombre de una voz ya registrada.

        Raises:
            FileNotFoundError: si la voz no está registrada (vía `voices.voice_paths`).
        """
        ref_path, speech_path = voices.voice_paths(name)
        voice_dir = os.path.dirname(ref_path)
        self._conditionals_prep.precompute_and_save(
            voice_dir, ref_path, speech_path, self._tts, self.compute_backend
        )
        log(f"Conditionals precomputados para la voz '{name}'")

    def load_precomputed_conditionals(self, voice_dir: str) -> bool:
        """
        Carga los conditionals precomputados del directorio de la voz si existen.

        Devuelve True si los cargó, False en caso contrario. Delegado a
        `ConditionalsPreparer` (inyectable en tests).
        """
        conds = self._conditionals_prep.load_precomputed(voice_dir, self.compute_backend)
        if conds is None:
            return False
        self._tts.conds = conds
        return True

    def list_voices(self) -> list[str]:
        """Lista todas las voces registradas."""
        return voices.list_voices()

    def remove_voice(self, name: str) -> bool:
        """Elimina una voz registrada."""
        return voices.remove_voice(name)

    def resolve_voice(self, name: str) -> tuple[str, str]:
        """
        Resuelve el nombre de la voz a sus rutas de audio de referencia y habla.

        Args:
            name: Nombre de la voz

        Returns:
            Tupla de (reference_path, speech_path)
        """
        return voices.voice_paths(name)
