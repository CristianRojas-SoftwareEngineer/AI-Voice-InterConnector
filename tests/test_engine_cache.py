"""Tests de la selección determinista de snapshot en la caché de HuggingFace.

Cubre _resolve_cached_snapshot (refs/main con fallback a mtime), la resolución
de hub_cache_path (precedencia HF_HUB_CACHE/HF_HOME/default) e is_model_cached
sobre una caché sintética en tmp_path.
"""

import contextlib
import importlib
import os
import time
from pathlib import Path

from ai_voice_interconnector.model_cache import (
    BASE_MODEL_REVISION,
    MODEL_REVISIONS,
    _resolve_cached_snapshot,
    hub_cache_path,
    is_model_cached,
)

ES_MX_FOLDER = "models--ResembleAI--Chatterbox-Multilingual-es-mx-latam"

# Revisión fijada del language pack: is_model_cached solo considera
# válido el snapshot de esta revisión, así que las cachés sintéticas de estos
# tests deben crearse bajo ese nombre de snapshot.
PINNED_REV = MODEL_REVISIONS["es-mx-latam"]

# Revisión fijada del repo base (fuente de ve.safetensors): la resolución del
# snapshot base en is_ve_cached también la honra (cierre del hueco residual).
BASE_PINNED_REV = BASE_MODEL_REVISION

# Contenido safetensors sintético con header válido: header_length=100 (u64 LE)
# seguido de 100 bytes de relleno. Pasa _safetensors_header_ok (0 < 100 < 108).
VALID_SAFETENSORS = b"\x64\x00\x00\x00\x00\x00\x00\x00" + b"\x00" * 100


def _make_snapshot(model_dir, name, mtime=None):
    snap = model_dir / "snapshots" / name
    snap.mkdir(parents=True)
    if mtime is not None:
        os.utime(snap, (mtime, mtime))
    return snap


def _set_ref_main(model_dir, revision):
    refs = model_dir / "refs"
    refs.mkdir(exist_ok=True)
    (refs / "main").write_text(revision + "\n", encoding="utf-8")


class TestResolveCachedSnapshot:
    def test_without_directory_returns_none(self, tmp_path):
        assert _resolve_cached_snapshot(tmp_path / "inexistente") is None

    def test_empty_snapshots_returns_none(self, tmp_path):
        (tmp_path / "snapshots").mkdir()
        assert _resolve_cached_snapshot(tmp_path) is None

    def test_refs_main_wins_over_mtime(self, tmp_path):
        now = time.time()
        _make_snapshot(tmp_path, "aaa", mtime=now)  # el más reciente
        target = _make_snapshot(tmp_path, "bbb", mtime=now - 1000)
        _set_ref_main(tmp_path, "bbb")
        assert _resolve_cached_snapshot(tmp_path) == target

    def test_without_refs_picks_most_recent_mtime(self, tmp_path):
        now = time.time()
        _make_snapshot(tmp_path, "vieja", mtime=now - 1000)
        newest = _make_snapshot(tmp_path, "nueva", mtime=now)
        assert _resolve_cached_snapshot(tmp_path) == newest

    def test_ref_to_absent_snapshot_falls_back_to_mtime(self, tmp_path):
        only = _make_snapshot(tmp_path, "aaa")
        _set_ref_main(tmp_path, "borrada")
        assert _resolve_cached_snapshot(tmp_path) == only


class TestCorruptConditionals:
    def _engine_sin_modelo(self):
        """Instancia de ChatterboxEngine sin cargar el modelo real."""
        from ai_voice_interconnector.engine import ChatterboxEngine
        from ai_voice_interconnector.conditionals import ConditionalsPreparer
        from ai_voice_interconnector.audio_writer import AudioWriter
        from ai_voice_interconnector.synthesis import SynthesisOrchestrator

        eng = ChatterboxEngine.__new__(ChatterboxEngine)
        eng.compute_backend = "cpu"
        eng._conds_cache_key = None
        eng._active_progress_cb = None
        # Tras la extracción de ConditionalsPreparer, el engine delega la
        # carga de conditionals a este colaborador; lo inyectamos igual que __init__.
        eng._conditionals_prep = ConditionalsPreparer()
        # El flujo de síntesis vive en el orquestador; el engine.synthesize es
        # un façade que delega en él. Lo cableamos igual que __init__.
        eng._audio_writer = AudioWriter()
        eng._orchestrator = SynthesisOrchestrator(
            eng, eng._conditionals_prep, eng._audio_writer
        )
        return eng

    def test_load_returns_false_with_corrupt_file(self, tmp_path):
        eng = self._engine_sin_modelo()
        eng._tts = object()
        (tmp_path / "conditionals.pt").write_bytes(b"no es un checkpoint")
        assert eng.load_precomputed_conditionals(str(tmp_path)) is False

    def test_synthesize_recomputes_with_corrupt_conditionals(self, tmp_path, monkeypatch):
        eng = self._engine_sin_modelo()
        voice = tmp_path / "voz"
        voice.mkdir()
        (voice / "conditionals.pt").write_bytes(b"basura")
        speech = voice / "speech-reference.wav"
        speech.write_bytes(b"RIFF")

        recomputos = []
        eng._conditionals_prep.compute = lambda *a, **kw: recomputos.append((a, kw))

        class FakeTTS:
            conds = None
            sr = 24000

            def generate(self, text, **kwargs):
                return [0.0]

        eng._tts = FakeTTS()
        monkeypatch.setattr(
            eng._orchestrator.audio_writer, "write",
            lambda audio_data, sample_rate: b"RIFF",
        )

        assert eng.synthesize("hola", speech_reference=str(speech)).audio_bytes == b"RIFF"
        assert recomputos, "synthesize debe recomputar los conditionals cuando el .pt es corrupto"


class TestUnifiedParameters:
    def _engine_stub(self, tmp_path):
        from ai_voice_interconnector.engine import ChatterboxEngine
        from ai_voice_interconnector.conditionals import ConditionalsPreparer
        from ai_voice_interconnector.audio_writer import AudioWriter
        from ai_voice_interconnector.synthesis import SynthesisOrchestrator

        eng = ChatterboxEngine.__new__(ChatterboxEngine)
        eng.compute_backend = "cpu"
        eng._conds_cache_key = None
        eng._active_progress_cb = None
        eng._conditionals_prep = ConditionalsPreparer()

        class FakeTTS:
            conds = None
            sr = 24000

            def generate(self, text, **kwargs):
                self.last_generate_kwargs = kwargs
                return [0.0]

        eng._tts = FakeTTS()
        eng._audio_writer = AudioWriter()
        eng._orchestrator = SynthesisOrchestrator(
            eng, eng._conditionals_prep, eng._audio_writer
        )
        return eng

    def test_get_instance_includes_models_dir_in_key(self, monkeypatch):
        from ai_voice_interconnector.engine import ChatterboxEngine

        monkeypatch.setattr(ChatterboxEngine, "_cache", {})
        monkeypatch.setattr(
            ChatterboxEngine, "__init__", lambda self, model, compute_backend, models_dir=None: None
        )
        a = ChatterboxEngine.get_instance(models_dir="/ruta/a")
        b = ChatterboxEngine.get_instance(models_dir="/ruta/b")
        assert a is not b
        assert ChatterboxEngine.get_instance(models_dir="/ruta/a") is a

    def test_direct_mode_uses_unified_exaggeration(self, tmp_path, monkeypatch):
        from ai_voice_interconnector.engine import ChatterboxEngine

        eng = self._engine_stub(tmp_path)
        monkeypatch.setattr(
            eng._orchestrator.audio_writer, "write",
            lambda audio_data, sample_rate: b"RIFF",
        )
        speech = tmp_path / "speech-reference.wav"
        speech.write_bytes(b"RIFF")
        eng._conditionals_prep.compute = lambda *a, **kw: None

        eng.synthesize("hola", speech_reference=str(speech))

        assert eng._tts.last_generate_kwargs["exaggeration"] == ChatterboxEngine.EXAGGERATION

    def test_conditionals_memoization_by_mtime(self, tmp_path, monkeypatch):
        import os
        from ai_voice_interconnector.engine import ChatterboxEngine

        eng = self._engine_stub(tmp_path)
        monkeypatch.setattr(
            eng._orchestrator.audio_writer, "write",
            lambda audio_data, sample_rate: b"RIFF",
        )

        voice = tmp_path / "voz"
        voice.mkdir()
        conds = voice / "conditionals.pt"
        conds.write_bytes(b"valido")
        speech = voice / "speech-reference.wav"
        speech.write_bytes(b"RIFF")

        loads = []

        def fake_load(voice_dir):
            loads.append(voice_dir)
            eng._tts.conds = object()
            return True

        eng.load_precomputed_conditionals = fake_load

        eng.synthesize("hola", speech_reference=str(speech))
        eng.synthesize("hola otra vez", speech_reference=str(speech))
        assert len(loads) == 1, "la segunda síntesis de la misma voz no debe releer disco"

        # Un conditionals.pt regenerado (mtime nuevo) invalida la memoización
        mtime = os.path.getmtime(conds) + 10
        os.utime(conds, (mtime, mtime))
        eng.synthesize("hola de nuevo", speech_reference=str(speech))
        assert len(loads) == 2


@contextlib.contextmanager
def _hf_env(**env):
    """Fija las variables de entorno de HF y recarga constants; restaura al salir.

    huggingface_hub.constants computa HF_HUB_CACHE en tiempo de import, así que
    cambiar el entorno exige recargar el módulo para que el cambio surta efecto
    (y recargarlo de nuevo al salir para no contaminar otros tests).
    """
    from huggingface_hub import constants

    keys = ("HF_HOME", "HF_HUB_CACHE", "HUGGINGFACE_HUB_CACHE", "XDG_CACHE_HOME")
    saved = {k: os.environ.get(k) for k in keys}
    try:
        for k in keys:
            os.environ.pop(k, None)
        for k, v in env.items():
            os.environ[k] = v
        importlib.reload(constants)
        yield
    finally:
        for k, v in saved.items():
            if v is None:
                os.environ.pop(k, None)
            else:
                os.environ[k] = v
        importlib.reload(constants)


class TestDownloadModelHonorsPin:
    """La resolución de carga del engine honra la revisión fijada, igual
    que la detección de caché: un snapshot de otra revisión no se usa aunque
    refs/main lo prefiera (antes la carga caía al fallback refs/main→mtime)."""

    def _fake_hub(self, tmp_path, monkeypatch):
        hub = tmp_path / "hub"
        hub.mkdir()
        from huggingface_hub import constants

        monkeypatch.setattr(constants, "HF_HUB_CACHE", str(hub))
        return hub

    def _engine_sin_modelo(self):
        from ai_voice_interconnector.engine import ChatterboxEngine

        return ChatterboxEngine.__new__(ChatterboxEngine)

    def test_download_model_ignores_other_revision_snapshot(self, tmp_path, monkeypatch):
        hub = self._fake_hub(tmp_path, monkeypatch)
        model_dir = hub / ES_MX_FOLDER

        # Snapshot de la revisión fijada (el que la carga DEBE usar).
        pinned = _make_snapshot(model_dir, PINNED_REV, mtime=time.time() - 1000)
        (pinned / "t3_es_mx_latam.safetensors").write_bytes(VALID_SAFETENSORS)

        # Snapshot de OTRA revisión, más reciente y apuntado por refs/main:
        # sin honrar el pin, la carga lo preferiría (asimetría).
        other = _make_snapshot(model_dir, "otra_revision", mtime=time.time())
        (other / "t3_es_mx_latam.safetensors").write_bytes(VALID_SAFETENSORS)
        _set_ref_main(model_dir, "otra_revision")

        eng = self._engine_sin_modelo()
        resolved = eng._download_model("ResembleAI/Chatterbox-Multilingual-es-mx-latam")
        assert resolved == pinned


class TestHubCachePath:
    def test_hf_hub_cache_takes_precedence(self, tmp_path):
        custom = tmp_path / "hub-custom"
        with _hf_env(HF_HUB_CACHE=str(custom), HF_HOME=str(tmp_path / "hf-home")):
            assert hub_cache_path() == custom

    def test_hf_home_defines_root(self, tmp_path):
        home = tmp_path / "hf-home"
        with _hf_env(HF_HOME=str(home)):
            assert hub_cache_path() == home / "hub"

    def test_fallback_to_default(self):
        with _hf_env():
            assert hub_cache_path() == Path("~/.cache/huggingface/hub").expanduser()


class TestIsModelCached:
    def _fake_hub(self, tmp_path, monkeypatch):
        """Redirige la caché de HuggingFace a una caché sintética."""
        hub = tmp_path / "hub"
        hub.mkdir()
        from huggingface_hub import constants
        monkeypatch.setattr(constants, "HF_HUB_CACHE", str(hub))
        return hub

    def test_without_cache_returns_false(self, tmp_path, monkeypatch):
        self._fake_hub(tmp_path, monkeypatch)
        assert is_model_cached("es-mx-latam") is False

    def test_snapshot_with_checkpoint_and_ve_returns_true(self, tmp_path, monkeypatch):
        hub = self._fake_hub(tmp_path, monkeypatch)
        model_dir = hub / ES_MX_FOLDER
        snap = _make_snapshot(model_dir, PINNED_REV)
        _set_ref_main(model_dir, PINNED_REV)
        # Antes bastaba con `b"\x00"` (pasaba el .exists()): ahora se
        # valida la integridad del header de los tres checkpoints.
        (snap / "t3_es_mx_latam.safetensors").write_bytes(VALID_SAFETENSORS)
        (snap / "s3gen_v3.safetensors").write_bytes(VALID_SAFETENSORS)
        (snap / "ve.safetensors").write_bytes(VALID_SAFETENSORS)
        assert is_model_cached("es-mx-latam") is True

    def test_t3_present_without_ve_returns_false(self, tmp_path, monkeypatch):
        """El Voice Encoder es obligatorio: sin ve.safetensors resoluble,
        la primera síntesis dispararía una descarga (fuga de la promesa offline)."""
        hub = self._fake_hub(tmp_path, monkeypatch)
        model_dir = hub / ES_MX_FOLDER
        snap = _make_snapshot(model_dir, PINNED_REV)
        _set_ref_main(model_dir, PINNED_REV)
        (snap / "t3_es_mx_latam.safetensors").write_bytes(VALID_SAFETENSORS)
        (snap / "s3gen_v3.safetensors").write_bytes(VALID_SAFETENSORS)
        assert is_model_cached("es-mx-latam") is False

    def test_ve_in_base_model_also_counts(self, tmp_path, monkeypatch):
        """ve.safetensors puede residir en la caché del modelo base, siempre
        bajo el snapshot de la revisión base fijada (el camino base honra el pin)."""
        hub = self._fake_hub(tmp_path, monkeypatch)
        model_dir = hub / ES_MX_FOLDER
        snap = _make_snapshot(model_dir, PINNED_REV)
        _set_ref_main(model_dir, PINNED_REV)
        (snap / "t3_es_mx_latam.safetensors").write_bytes(VALID_SAFETENSORS)
        (snap / "s3gen_v3.safetensors").write_bytes(VALID_SAFETENSORS)

        base_dir = hub / "models--ResembleAI--chatterbox"
        base_snap = _make_snapshot(base_dir, BASE_PINNED_REV)
        _set_ref_main(base_dir, BASE_PINNED_REV)
        (base_snap / "ve.safetensors").write_bytes(VALID_SAFETENSORS)
        assert is_model_cached("es-mx-latam") is True

    def test_ve_in_base_of_other_revision_returns_false(self, tmp_path, monkeypatch):
        """ve.safetensors bajo un snapshot del repo base de otra revisión no cuenta
        como caché válida — si un release bumpea BASE_MODEL_REVISION, 'setup' debe
        re-descargar el VE."""
        hub = self._fake_hub(tmp_path, monkeypatch)
        model_dir = hub / ES_MX_FOLDER
        snap = _make_snapshot(model_dir, PINNED_REV)
        _set_ref_main(model_dir, PINNED_REV)
        (snap / "t3_es_mx_latam.safetensors").write_bytes(VALID_SAFETENSORS)
        (snap / "s3gen_v3.safetensors").write_bytes(VALID_SAFETENSORS)

        base_dir = hub / "models--ResembleAI--chatterbox"
        base_snap = _make_snapshot(base_dir, "otra_revision_base")
        _set_ref_main(base_dir, "otra_revision_base")
        (base_snap / "ve.safetensors").write_bytes(VALID_SAFETENSORS)
        assert is_model_cached("es-mx-latam") is False

    def test_snapshot_of_other_revision_returns_false(self, tmp_path, monkeypatch):
        """Un snapshot completo pero de una revisión distinta a la fijada
        no cuenta como caché válida, aunque refs/main apunte a él."""
        hub = self._fake_hub(tmp_path, monkeypatch)
        model_dir = hub / ES_MX_FOLDER
        snap = _make_snapshot(model_dir, "otra_revision")
        _set_ref_main(model_dir, "otra_revision")
        (snap / "t3_es_mx_latam.safetensors").write_bytes(VALID_SAFETENSORS)
        (snap / "s3gen_v3.safetensors").write_bytes(VALID_SAFETENSORS)
        (snap / "ve.safetensors").write_bytes(VALID_SAFETENSORS)
        assert is_model_cached("es-mx-latam") is False

    def test_snapshot_of_pinned_revision_returns_true(self, tmp_path, monkeypatch):
        """El snapshot de la revisión fijada valida incluso sin refs/main
        (hf_hub_download con commit hash no crea refs)."""
        hub = self._fake_hub(tmp_path, monkeypatch)
        model_dir = hub / ES_MX_FOLDER
        snap = _make_snapshot(model_dir, PINNED_REV)  # sin _set_ref_main
        (snap / "t3_es_mx_latam.safetensors").write_bytes(VALID_SAFETENSORS)
        (snap / "s3gen_v3.safetensors").write_bytes(VALID_SAFETENSORS)
        (snap / "ve.safetensors").write_bytes(VALID_SAFETENSORS)
        assert is_model_cached("es-mx-latam") is True

    def test_safetensors_header_truncated_returns_false(self, tmp_path, monkeypatch):
        """Un t3_es_mx_latam.safetensors truncado (header-length inválido)
        debe tratarse como caché corrupta: 'doctor' lo marcará FAIL y remitirá
        a 'setup' para una re-descarga limpia."""
        from ai_voice_interconnector.model_cache import _safetensors_header_ok

        hub = self._fake_hub(tmp_path, monkeypatch)
        model_dir = hub / ES_MX_FOLDER
        snap = _make_snapshot(model_dir, PINNED_REV)
        _set_ref_main(model_dir, PINNED_REV)

        # Header-length 0 (los 8 primeros bytes a cero): archivo vacío/truncado.
        (snap / "t3_es_mx_latam.safetensors").write_bytes(b"\x00" * 8)
        (snap / "s3gen_v3.safetensors").write_bytes(VALID_SAFETENSORS)
        (snap / "ve.safetensors").write_bytes(VALID_SAFETENSORS)
        assert is_model_cached("es-mx-latam") is False
        # El helper en sí también lo rechaza: el header-length de 0 nunca es
        # válido (un header JSON de tamaño 0 no codifica metadatos del tensor).
        assert _safetensors_header_ok(snap / "t3_es_mx_latam.safetensors") is False

    def test_safetensors_header_valid_returns_true(self, tmp_path, monkeypatch):
        """Un .safetensors con header-length plausible (en el rango (0, size))
        pasa la validación ligera; se mantiene el resto del flujo de is_ve_cached."""
        from ai_voice_interconnector.model_cache import _safetensors_header_ok

        hub = self._fake_hub(tmp_path, monkeypatch)
        model_dir = hub / ES_MX_FOLDER
        snap = _make_snapshot(model_dir, PINNED_REV)
        _set_ref_main(model_dir, PINNED_REV)
        (snap / "t3_es_mx_latam.safetensors").write_bytes(VALID_SAFETENSORS)
        (snap / "s3gen_v3.safetensors").write_bytes(VALID_SAFETENSORS)
        (snap / "ve.safetensors").write_bytes(VALID_SAFETENSORS)
        assert _safetensors_header_ok(snap / "t3_es_mx_latam.safetensors") is True
        assert is_model_cached("es-mx-latam") is True

    def test_s3gen_truncated_returns_false(self, tmp_path, monkeypatch):
        """Un s3gen_v3.safetensors truncado se reporta como no cacheado,
        igual que el T3 (el engine carga los tres checkpoints)."""
        hub = self._fake_hub(tmp_path, monkeypatch)
        model_dir = hub / ES_MX_FOLDER
        snap = _make_snapshot(model_dir, PINNED_REV)
        _set_ref_main(model_dir, PINNED_REV)
        (snap / "t3_es_mx_latam.safetensors").write_bytes(VALID_SAFETENSORS)
        (snap / "s3gen_v3.safetensors").write_bytes(b"\x00" * 8)  # header-length 0
        (snap / "ve.safetensors").write_bytes(VALID_SAFETENSORS)
        assert is_model_cached("es-mx-latam") is False

    def test_s3gen_missing_returns_false(self, tmp_path, monkeypatch):
        """Sin s3gen_v3.safetensors el modelo no está completo."""
        hub = self._fake_hub(tmp_path, monkeypatch)
        model_dir = hub / ES_MX_FOLDER
        snap = _make_snapshot(model_dir, PINNED_REV)
        _set_ref_main(model_dir, PINNED_REV)
        (snap / "t3_es_mx_latam.safetensors").write_bytes(VALID_SAFETENSORS)
        (snap / "ve.safetensors").write_bytes(VALID_SAFETENSORS)
        assert is_model_cached("es-mx-latam") is False

    def test_ve_truncated_returns_false(self, tmp_path, monkeypatch):
        """Un ve.safetensors truncado ya no pasa por mera existencia."""
        hub = self._fake_hub(tmp_path, monkeypatch)
        model_dir = hub / ES_MX_FOLDER
        snap = _make_snapshot(model_dir, PINNED_REV)
        _set_ref_main(model_dir, PINNED_REV)
        (snap / "t3_es_mx_latam.safetensors").write_bytes(VALID_SAFETENSORS)
        (snap / "s3gen_v3.safetensors").write_bytes(VALID_SAFETENSORS)
        (snap / "ve.safetensors").write_bytes(b"\x00" * 8)  # header-length 0
        assert is_model_cached("es-mx-latam") is False

    def test_safetensors_header_larger_than_file_returns_false(self, tmp_path):
        """Un header-length que excede el tamaño del archivo es signo claro de
        truncamiento: el helper debe rechazarlo sin necesidad de parsear JSON."""
        from ai_voice_interconnector.model_cache import _safetensors_header_ok

        p = tmp_path / "fake.safetensors"
        p.write_bytes(b"\xff\xff\xff\xff\xff\xff\xff\x7f" + b"x" * 4)  # ~9.2 EB
        assert _safetensors_header_ok(p) is False


class TestIsModelCachedEn:
    """Espejo de TestIsModelCached para el modelo inglés base ('en'), promovido
    a entrada de primer nivel en el rediseño cross-lingual. Mismo repo que la
    fuente de ve.safetensors (BASE_MODEL_REPO), así que la carpeta de caché
    sintética es la misma que usa TestDownloadModelHonorsPin/is_ve_cached."""

    EN_FOLDER = "models--ResembleAI--chatterbox"

    def _fake_hub(self, tmp_path, monkeypatch):
        hub = tmp_path / "hub"
        hub.mkdir()
        from huggingface_hub import constants
        monkeypatch.setattr(constants, "HF_HUB_CACHE", str(hub))
        return hub

    def test_without_cache_returns_false(self, tmp_path, monkeypatch):
        self._fake_hub(tmp_path, monkeypatch)
        assert is_model_cached("en") is False

    def test_full_snapshot_returns_true(self, tmp_path, monkeypatch):
        hub = self._fake_hub(tmp_path, monkeypatch)
        model_dir = hub / self.EN_FOLDER
        snap = _make_snapshot(model_dir, BASE_PINNED_REV)
        _set_ref_main(model_dir, BASE_PINNED_REV)
        (snap / "t3_cfg.safetensors").write_bytes(VALID_SAFETENSORS)
        (snap / "s3gen.safetensors").write_bytes(VALID_SAFETENSORS)
        (snap / "ve.safetensors").write_bytes(VALID_SAFETENSORS)
        (snap / "tokenizer.json").write_text("{}", encoding="utf-8")
        (snap / "conds.pt").write_bytes(b"conds")
        assert is_model_cached("en") is True

    def test_truncated_t3_returns_false(self, tmp_path, monkeypatch):
        hub = self._fake_hub(tmp_path, monkeypatch)
        model_dir = hub / self.EN_FOLDER
        snap = _make_snapshot(model_dir, BASE_PINNED_REV)
        _set_ref_main(model_dir, BASE_PINNED_REV)
        (snap / "t3_cfg.safetensors").write_bytes(b"\x00" * 8)  # header-length 0
        (snap / "s3gen.safetensors").write_bytes(VALID_SAFETENSORS)
        (snap / "ve.safetensors").write_bytes(VALID_SAFETENSORS)
        (snap / "tokenizer.json").write_text("{}", encoding="utf-8")
        (snap / "conds.pt").write_bytes(b"conds")
        assert is_model_cached("en") is False

    def test_missing_tokenizer_returns_false(self, tmp_path, monkeypatch):
        """tokenizer.json no es safetensors: se valida solo por existencia,
        pero su ausencia igualmente reporta 'no cacheado'."""
        hub = self._fake_hub(tmp_path, monkeypatch)
        model_dir = hub / self.EN_FOLDER
        snap = _make_snapshot(model_dir, BASE_PINNED_REV)
        _set_ref_main(model_dir, BASE_PINNED_REV)
        (snap / "t3_cfg.safetensors").write_bytes(VALID_SAFETENSORS)
        (snap / "s3gen.safetensors").write_bytes(VALID_SAFETENSORS)
        (snap / "ve.safetensors").write_bytes(VALID_SAFETENSORS)
        (snap / "conds.pt").write_bytes(b"conds")
        assert is_model_cached("en") is False

    def test_missing_conds_returns_false(self, tmp_path, monkeypatch):
        hub = self._fake_hub(tmp_path, monkeypatch)
        model_dir = hub / self.EN_FOLDER
        snap = _make_snapshot(model_dir, BASE_PINNED_REV)
        _set_ref_main(model_dir, BASE_PINNED_REV)
        (snap / "t3_cfg.safetensors").write_bytes(VALID_SAFETENSORS)
        (snap / "s3gen.safetensors").write_bytes(VALID_SAFETENSORS)
        (snap / "ve.safetensors").write_bytes(VALID_SAFETENSORS)
        (snap / "tokenizer.json").write_text("{}", encoding="utf-8")
        assert is_model_cached("en") is False

    def test_missing_ve_returns_false(self, tmp_path, monkeypatch):
        """El VE vive en el mismo repo/snapshot que 'en': is_ve_cached(cached)
        lo resuelve directamente sin necesitar un repo base aparte."""
        hub = self._fake_hub(tmp_path, monkeypatch)
        model_dir = hub / self.EN_FOLDER
        snap = _make_snapshot(model_dir, BASE_PINNED_REV)
        _set_ref_main(model_dir, BASE_PINNED_REV)
        (snap / "t3_cfg.safetensors").write_bytes(VALID_SAFETENSORS)
        (snap / "s3gen.safetensors").write_bytes(VALID_SAFETENSORS)
        (snap / "tokenizer.json").write_text("{}", encoding="utf-8")
        (snap / "conds.pt").write_bytes(b"conds")
        assert is_model_cached("en") is False

    def test_snapshot_of_other_revision_returns_false(self, tmp_path, monkeypatch):
        hub = self._fake_hub(tmp_path, monkeypatch)
        model_dir = hub / self.EN_FOLDER
        snap = _make_snapshot(model_dir, "otra_revision")
        _set_ref_main(model_dir, "otra_revision")
        (snap / "t3_cfg.safetensors").write_bytes(VALID_SAFETENSORS)
        (snap / "s3gen.safetensors").write_bytes(VALID_SAFETENSORS)
        (snap / "ve.safetensors").write_bytes(VALID_SAFETENSORS)
        (snap / "tokenizer.json").write_text("{}", encoding="utf-8")
        (snap / "conds.pt").write_bytes(b"conds")
        assert is_model_cached("en") is False


class TestEsMxLatamRegressionAfterEn:
    """Regresión: promover 'en' a entrada de primer nivel no altera el
    comportamiento de detección de es-mx-latam (ramas independientes)."""

    def _fake_hub(self, tmp_path, monkeypatch):
        hub = tmp_path / "hub"
        hub.mkdir()
        from huggingface_hub import constants
        monkeypatch.setattr(constants, "HF_HUB_CACHE", str(hub))
        return hub

    def test_es_mx_latam_still_validated_independently_of_en(self, tmp_path, monkeypatch):
        hub = self._fake_hub(tmp_path, monkeypatch)
        model_dir = hub / ES_MX_FOLDER
        snap = _make_snapshot(model_dir, PINNED_REV)
        _set_ref_main(model_dir, PINNED_REV)
        (snap / "t3_es_mx_latam.safetensors").write_bytes(VALID_SAFETENSORS)
        (snap / "s3gen_v3.safetensors").write_bytes(VALID_SAFETENSORS)
        (snap / "ve.safetensors").write_bytes(VALID_SAFETENSORS)

        # 'en' no está cacheado: no debe afectar el resultado de es-mx-latam.
        assert is_model_cached("es-mx-latam") is True
        assert is_model_cached("en") is False


class TestModelFor:
    """model_for(): traduce la taxonomía de idioma compartida al alias de
    modelo que consumen MODELS/MODEL_REVISIONS/is_model_cached."""

    def test_es_latam_maps_to_es_mx_latam(self):
        from ai_voice_interconnector.model_cache import model_for
        assert model_for("es-latam") == "es-mx-latam"

    def test_en_maps_to_en(self):
        from ai_voice_interconnector.model_cache import model_for
        assert model_for("en") == "en"

    def test_unknown_value_passes_through(self):
        from ai_voice_interconnector.model_cache import model_for
        assert model_for("all") == "all"
