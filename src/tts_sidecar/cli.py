"""
Interfaz CLI de TTS Sidecar.
Consumible desde cualquier lenguaje de programación vía subprocess.

Contrato de salida (estable entre SO y lenguajes):
  - Los datos van a stdout; los diagnósticos y errores van a stderr.
  - Los comandos de lectura (voice list, devices, version, daemon status, doctor)
    aceptan --json para salida legible por máquina.
  - Códigos de salida (contrato público congelado; un orquestador distingue la
    causa sin parsear texto):
      0   éxito
      1   error genérico (incluye chequeos fallidos de doctor)
      2   entrada inválida (texto vacío, nombre ilegal, colisión, uso incorrecto)
      3   recurso no encontrado (voz o archivo de audio)
      4   modelo no provisionado (ejecutar 'setup')
      5   daemon inalcanzable o no gestionable
      6   conflicto de estado (recurso ocupado, colisión de voz, puerto daemón en uso)
      7   operación no aplicable al contexto actual (sólo lectura, plataforma no soportada)
      8   precondición de entorno incumplida (credenciales, red, permisos, disco)
      130 interrupción por el usuario (Ctrl+C)
  - stdout/stderr se fuerzan a UTF-8 para una codificación consistente en toda
    plataforma.
"""

import sys

# La capa única de bootstrap (UTF-8, warnings, env vars, mock de pkg_resources)
# ya NO corre como efecto colateral de importar este módulo: la invoca main()
# como su primera acción. Así importar `tts_sidecar.cli` deja de imponer que
# todo import posterior ocurra tras un apply() implícito. Los imports
# de módulo de abajo son livianos (stdlib + timing) y no arrastran chatterbox;
# las dependencias pesadas se importan de forma perezosa dentro de cada comando.
from . import bootstrap

import argparse
import json
import os
import platform
from pathlib import Path

from .exit_codes import (  # noqa: F401
    EXIT_OK,
    EXIT_ERROR,
    EXIT_INVALID_INPUT,
    EXIT_NOT_FOUND,
    EXIT_MODEL_MISSING,
    EXIT_DAEMON_UNREACHABLE,
    EXIT_STATE_CONFLICT,
    EXIT_NOT_APPLICABLE,
    EXIT_PRECONDITION_FAILED,
    EXIT_INTERRUPTED,
    CliError,
)
from .timing import timed_command, StageTimer, Spinner, log, format_progress_event

# Los códigos de salida y CliError viven en exit_codes.py y se
# re-exportan aquí para que sea la única superficie que necesita
# importar tts_sidecar.exit_codes desde la CLI.

# Versión del esquema de la salida --json del CLI (contrato legible por máquina).
# Se emite como "schema_version" en TODOS los payloads JSON. Es un campo aditivo:
# los consumidores lo usan para detectar cambios de forma; añadir claves nuevas no
# incrementa la versión, solo lo haría un cambio incompatible de las existentes.
SCHEMA_VERSION = "3"


def emit_json(payload: dict) -> None:
    """Único punto de `cli.py` que serializa un payload --json a stdout.

    Inyecta `schema_version` (sin sobrescribirla si el caller ya la trae) y
    garantiza exactamente un objeto JSON por invocación. Migrar todos los
    emisores existentes a este helper (en vez de cada uno construyendo su
    propio `print(json.dumps({...}))`) es lo que impide que un comando nuevo
    olvide `schema_version` o emita más de un objeto: la garantía queda en un
    solo lugar, no repetida por convención en cada comando.
    """
    payload.setdefault("schema_version", SCHEMA_VERSION)
    print(json.dumps(payload))


def _translate_cli_error(e: CliError) -> None:
    """Traduce una CliError a la salida estándar de la CLI y sale con su code.

    Imprime `e.message` a stderr. Si `--json` está en `sys.argv`, emite
    el payload de error como JSON a stdout y luego llama a `sys.exit(e.code)`.
    """
    print(e.message, file=sys.stderr)
    if "--json" in sys.argv:
        emit_json({"error": {"code": e.code, "reason": e.reason, "message": e.message}})
    sys.exit(e.code)


# Umbral mínimo de espacio libre en disco para descargar el modelo en 'setup'.
# El language pack + Voice Encoder ocupan ~4 GB medidos en caché; 6 GB deja
# margen para los .incomplete de la descarga y la caché temporal de HuggingFace.
MIN_FREE_DISK_BYTES = 6 * 1024 * 1024 * 1024  # 6 GB

# RAM recomendada para una síntesis fluida (chequeo advisory de 'doctor').
# Por debajo de este umbral la inferencia en CPU funciona pero puede paginar en
# textos largos; es un WARN, no un FAIL (no altera el exit code).
RECOMMENDED_RAM_BYTES = 8 * 1024 * 1024 * 1024  # 8 GB

# Imports perezosos (lazy): solo se cargan cuando se ejecutan los comandos.
# Esto permite que --help funcione sin las dependencias instaladas.


def _resolve_voice_paths(args):
    """Resuelve las rutas de audio de una voz a partir de su nombre SIN cargar el modelo."""
    from . import voices

    voice_name = getattr(args, 'voice', None) or "default"
    timbre_reference, speech_reference = voices.voice_paths(voice_name)

    # Resuelve a rutas absolutas contra el CWD del cliente antes de que crucen la
    # frontera de proceso hacia el daemon, que tiene otro directorio de trabajo.
    if timbre_reference:
        timbre_reference = str(Path(timbre_reference).resolve())
    if speech_reference:
        speech_reference = str(Path(speech_reference).resolve())

    return timbre_reference, speech_reference


def _play_audio(audio_bytes):
    """Reproduce los bytes de audio."""
    log("[Reproducción] Reproduciendo audio...")
    from .audio import AudioPlayer
    try:
        player = AudioPlayer()
    except OSError as e:
        # PortAudio (dependencia nativa de sounddevice) falta en el sistema:
        # precondición de entorno incumplida, no un error genérico. El mensaje
        # accionable ya viene armado desde AudioPlayer._init_linux.
        raise CliError(EXIT_PRECONDITION_FAILED, "audio_library_missing", f"[FAIL] {e}")
    player.play(audio_bytes)
    log("[Reproducción] Reproducción finalizada")


def _warn_compute_backend_ignored(args):
    """Avisa si --compute-backend se ignora porque la síntesis va vía daemon.

    El daemon fija su compute backend una sola vez al arrancar (ver
    docs/DAEMON-MODE.md); un --compute-backend explícito distinto de "auto" en
    una invocación individual no tiene efecto en esa ruta, y antes se ignoraba
    en silencio.
    """
    backend = getattr(args, "compute_backend", "auto")
    if backend and backend != "auto":
        print(
            f"Advertencia: --compute-backend {backend} se ignora; el daemon usa "
            "el backend fijado a su arranque.",
            file=sys.stderr,
        )


def _synthesize_via_daemon(args, voice):
    """Sintetiza vía daemon y retorna el `SynthesisResult` (audio + métricas).

    Asume el daemon disponible: cualquier fallo de comunicación o síntesis
    propaga la excepción al llamador (sin fallback silencioso a modo directo).
    """
    import time

    from .daemon import DaemonIPCClient

    synth_start = time.time()
    log("[Servidor] Enviando solicitud de síntesis...")
    client = DaemonIPCClient()
    # El daemon transmite su progreso real por el stream NDJSON; on_progress
    # actualiza la etiqueta del spinner en vivo (etapa y tokens del T3). En
    # no-TTY el spinner es un no-op y la salida es idéntica a antes.
    with Spinner("Sintetizando vía daemon…") as sp:
        result = client.synthesize(
            text=args.text,
            voice=voice,
            language=getattr(args, "language", "es-latam"),
            exaggeration=getattr(args, "exaggeration", None),
            cfg_weight=getattr(args, "cfg_weight", None),
            temperature=getattr(args, "temperature", None),
            on_progress=lambda ev: sp.update(format_progress_event(ev)),
        )
    elapsed = time.time() - synth_start
    log(f"[Servidor] Síntesis completada ({elapsed:.1f}s)")

    return result


def _require_model_cached(model: str = "es-mx-latam", language: str = "es-latam"):
    """Verifica que el modelo esté en caché y, si no lo está, aborta remitiendo a 'setup'."""
    from .model_cache import is_model_cached
    if not is_model_cached(model):
        raise CliError(
            EXIT_MODEL_MISSING,
            "model_missing",
            f"Error: el modelo '{model}' no está descargado.\n"
            f"Ejecuta 'tts-sidecar setup --language {language}' para descargarlo antes de continuar.",
        )


def _require_models_cached_for_daemon(language: str) -> None:
    """Exige en caché el/los modelo(s) que el daemon precargará (§3.9).

    `language` "all" precarga ambos (es-latam y en); un valor concreto solo
    exige el suyo. Reutiliza `_require_model_cached` (mismo mensaje remitiendo
    a 'setup --language <x>') para cada idioma a precargar.
    """
    from .model_cache import model_for
    languages_to_preload = ["es-latam", "en"] if language == "all" else [language]
    for lang in languages_to_preload:
        _require_model_cached(model_for(lang), lang)


def _emit_say_json(voice_name: str) -> None:
    """Payload --json de `speech say`: solo la voz efectiva (§2.10).

    No repite el `text` (el llamador acaba de mandarlo) ni emite tiempos ni
    la vía de despacho: lo único que el llamador puede no saber es qué voz se
    usó, porque si no pasó --voice la eligió el sistema.
    """
    emit_json({"voice": voice_name})


def _validate_synthesis_text(text: str) -> None:
    """Valida el texto de síntesis (reglas 3 y 4 de §2.6), común a say y synthesize.

    Texto vacío o solo espacios y texto que excede `MAX_TEXT_LENGTH` salen con 2,
    validado en el cliente antes de cualquier despacho (el tope del daemon es
    defensa en profundidad). El aviso de truncado por texto largo (>2000) es no
    bloqueante y va por stderr.
    """
    if not text or not text.strip():
        raise CliError(EXIT_INVALID_INPUT, "usage_error",
                       "Error: --text no puede estar vacío.")

    from .daemon.protocol import MAX_TEXT_LENGTH
    if len(text) > MAX_TEXT_LENGTH:
        raise CliError(
            EXIT_INVALID_INPUT,
            "usage_error",
            f"Error: el texto tiene {len(text)} caracteres; el máximo permitido es "
            f"{MAX_TEXT_LENGTH}. Fragmenta el texto en varias llamadas.",
        )

    if len(text) > 2000:
        print(
            f"Advertencia: el texto tiene {len(text)} caracteres; los textos "
            "muy largos pueden truncarse al sintetizar (límite interno de tokens). "
            "Considera fragmentarlo en oraciones o párrafos para mejores resultados.",
            file=sys.stderr,
        )


def _validate_synthesis_params(args) -> None:
    """Valida los overrides de síntesis (§3.5 del rediseño cross-lingual).

    `cfg_weight=0.0` es un crash conocido en el inglés base (asume batch=2 en
    t3.py, y solo lo duplica si cfg_weight > 0.0); valores negativos o de
    temperatura no positiva tampoco tienen sentido físico. Exit 2 si algo no
    calza, igual que el resto de validaciones client-side del texto.
    """
    cfg_weight = getattr(args, "cfg_weight", None)
    if cfg_weight is not None and cfg_weight <= 0.0:
        raise CliError(
            EXIT_INVALID_INPUT, "usage_error",
            "Error: --cfg-weight debe ser mayor que 0 (0.0 provoca un crash "
            "conocido en el inglés base).",
        )

    exaggeration = getattr(args, "exaggeration", None)
    if exaggeration is not None and exaggeration < 0.0:
        raise CliError(EXIT_INVALID_INPUT, "usage_error",
                       "Error: --exaggeration no puede ser negativo.")

    temperature = getattr(args, "temperature", None)
    if temperature is not None and temperature <= 0.0:
        raise CliError(EXIT_INVALID_INPUT, "usage_error",
                       "Error: --temperature debe ser mayor que 0.")


def _validate_identifier(value: str, kind: str) -> str:
    """Valida y normaliza un identificador (voz o etiqueta); exit 2 si es ilegal.

    Delega en el único validador parametrizado (`voices._validate_path_segment`),
    de modo que voz y etiqueta comparten regex y defensa anti-escape; `kind` fija
    el sustantivo del mensaje para que el error apunte al flag correcto (§2.8).
    """
    from . import voices
    try:
        return voices._validate_path_segment(value, kind=kind)
    except ValueError as e:
        raise CliError(EXIT_INVALID_INPUT, "invalid_identifier", f"Error: {e}")


def _require_voice_exists(voice_name: str) -> None:
    """Exige que la voz exista (usuario o fábrica); exit 3 si no (§2.6).

    Se aplica en las cinco sub-acciones de `speech` para que «voz mal escrita»
    nunca se disfrace de «sin resultados».
    """
    from . import voices
    if voices._resolve_voice_dir(voice_name) is None:
        raise CliError(
            EXIT_NOT_FOUND, "voice_not_found",
            f"Error: la voz '{voice_name}' no existe. Clónala con 'tts-sidecar "
            "voice clone' o usa la voz 'default'.",
        )


def _dispatch_synthesis(args, voice_name: str):
    """Despacho de tres modos (§2.5). Devuelve `(result, daemon_flag)`.

    Compartido por `speech synthesize` y `speech say`: ambas sintetizan igual y
    solo difieren en el destino de `result` (disco o parlantes).
      --daemon:    exige el daemon; exit 5 si no está activo (no degrada).
      --no-daemon: fuerza modo directo sin sondear.
      sin flags:   autodetección: daemon si responde, directo si no.
    """
    from .daemon import is_daemon_running

    if getattr(args, "daemon", False):
        if not is_daemon_running():
            raise CliError(
                EXIT_DAEMON_UNREACHABLE, "daemon_unreachable",
                "Error: se exigió el daemon (--daemon) pero no está activo. "
                "Inícialo con 'tts-sidecar daemon start' o reintenta sin --daemon.",
            )
        _warn_compute_backend_ignored(args)
        return _synthesize_via_daemon(args, voice_name), True

    if not getattr(args, "no_daemon", False) and is_daemon_running():
        _warn_compute_backend_ignored(args)
        return _synthesize_via_daemon(args, voice_name), True

    # Modo directo: los imports pesados solo se cargan cuando no se usa el daemon.
    log("[Servidor] No disponible o desactivado; usando modo directo")
    from .engine import ChatterboxEngine

    # Resuelve las rutas de audio de la voz SIN cargar el modelo: solo la rama
    # directa las necesita (el daemon resuelve el nombre por su cuenta).
    timbre_reference, speech_reference = _resolve_voice_paths(args)

    # Spinner de liveness durante los dos tramos largos y opacos: la carga del
    # modelo (primer speech synthesize) y la síntesis. Las líneas [Stage N/4] del engine se
    # intercalan de forma coordinada (timing._active_spinner).
    from .model_cache import model_for
    model = model_for(getattr(args, "language", "es-latam"))

    with Spinner("Cargando modelo…") as _sp:
        engine = ChatterboxEngine.get_instance(model=model, compute_backend=args.compute_backend)
        _sp.update("Sintetizando voz…")
        result = engine.synthesize(
            text=args.text,
            timbre_reference=timbre_reference,
            speech_reference=speech_reference,
            progress_callback=lambda ev: _sp.update(format_progress_event(ev)),
            exaggeration=getattr(args, "exaggeration", None),
            cfg_weight=getattr(args, "cfg_weight", None),
            temperature=getattr(args, "temperature", None),
        )
    return result, False


@timed_command
def cmd_speech_say(args):
    """Sintetiza texto y reproduce el audio (no persiste)."""
    try:
        _validate_synthesis_text(args.text)
        _validate_synthesis_params(args)

        # Nombre de voz efectivo: el de --voice, o "default" (voz de fábrica).
        voice_name = _validate_identifier(getattr(args, "voice", None) or "default", "voz")

        # Exige el modelo en caché antes de sintetizar (descargas → 'setup').
        language = getattr(args, "language", "es-latam")
        from .model_cache import model_for
        _require_model_cached(model_for(language), language)
        _require_voice_exists(voice_name)

        result, _daemon_flag = _dispatch_synthesis(args, voice_name)
        _play_audio(result.audio_bytes)

        if getattr(args, "json", False):
            _emit_say_json(voice_name)

    except FileNotFoundError as e:
        print(f"Error: {e}", file=sys.stderr)
        # Remitir a setup solo cuando el faltante es el modelo: un audio o una
        # voz ausentes no se resuelven descargando el modelo.
        from .model_cache import is_model_cached, model_for
        language = getattr(args, "language", "es-latam")
        if not is_model_cached(model_for(language)):
            raise CliError(EXIT_MODEL_MISSING, "model_missing",
                            f"Ejecuta 'tts-sidecar setup --language {language}' primero.")
        raise CliError(EXIT_NOT_FOUND, "not_found",
                        "Voz no encontrada.")
    except Exception as e:
        # Un fallo del daemon (--daemon o sondeo automático) es inalcanzabilidad,
        # no un error genérico de síntesis: se distingue con su propio código.
        from .daemon import DaemonIPCError
        if isinstance(e, DaemonIPCError):
            raise CliError(EXIT_DAEMON_UNREACHABLE, "daemon_unreachable",
                            f"Error: no se pudo sintetizar vía daemon: {e}")
        raise CliError(EXIT_ERROR, "generic", f"Error: {e}")


@timed_command
def cmd_speech_synthesize(args):
    """Sintetiza texto y lo guarda en el almacén de habla sintética (§2.4)."""
    from . import synthetic_speech

    try:
        # Regla 2 (§2.6): --json y --play son incompatibles (el bucle usa
        # stdin/stdout y contaminaría el payload).
        if getattr(args, "json", False) and getattr(args, "play", False):
            raise CliError(
                EXIT_INVALID_INPUT, "usage_error",
                "Error: --play y --json son incompatibles: el bucle interactivo "
                "usa la entrada y la salida estándar y contaminaría el payload.",
            )

        # Regla 5 (§2.6): --play exige terminal en la entrada estándar. Se
        # rechaza antes de sintetizar.
        if getattr(args, "play", False) and not sys.stdin.isatty():
            raise CliError(
                EXIT_INVALID_INPUT, "usage_error",
                "Error: --play requiere una terminal interactiva en la entrada "
                "estándar; no la hay en esta invocación.",
            )

        _validate_synthesis_text(args.text)
        _validate_synthesis_params(args)
        voice_name = _validate_identifier(getattr(args, "voice", None) or "default", "voz")
        label = _validate_identifier(args.label, "etiqueta")

        language = getattr(args, "language", "es-latam")
        from .model_cache import model_for
        _require_model_cached(model_for(language), language)
        _require_voice_exists(voice_name)

        force = getattr(args, "force", False)
        # Fast-fail de colisión antes de gastar GPU (§2.4): etiqueta tomada sin
        # --force sale 6 sin sintetizar.
        if not force and synthetic_speech.exists(voice_name, label):
            raise CliError(
                EXIT_STATE_CONFLICT, "label_exists",
                f"Error: la etiqueta '{label}' ya existe para la voz '{voice_name}'. "
                "Usa --force para sobrescribirla o elige otra etiqueta.",
            )

        result, daemon_flag = _dispatch_synthesis(args, voice_name)

        if getattr(args, "play", False):
            # Bucle interactivo: reproduce y pregunta antes de guardar. La
            # persistencia (o su ausencia) la decide el bucle.
            _synthesize_play_loop(args, voice_name, label, result, force)
            return

        synthetic_speech.save(voice_name, label, result.audio_bytes, args.text)

        if getattr(args, "json", False):
            emit_json({
                "voice": voice_name,
                "label": label,
                "t3_time": result.metrics.t3,
                "s3gen_time": result.metrics.s3gen,
                "daemon": daemon_flag,
            })
        else:
            print(f"Locución '{label}' guardada (voz '{voice_name}').")

    except FileNotFoundError as e:
        print(f"Error: {e}", file=sys.stderr)
        from .model_cache import is_model_cached, model_for
        language = getattr(args, "language", "es-latam")
        if not is_model_cached(model_for(language)):
            raise CliError(EXIT_MODEL_MISSING, "model_missing",
                            f"Ejecuta 'tts-sidecar setup --language {language}' primero.")
        raise CliError(EXIT_NOT_FOUND, "not_found", "Voz no encontrada.")
    except Exception as e:
        from .daemon import DaemonIPCError
        if isinstance(e, DaemonIPCError):
            raise CliError(EXIT_DAEMON_UNREACHABLE, "daemon_unreachable",
                            f"Error: no se pudo sintetizar vía daemon: {e}")
        raise CliError(EXIT_ERROR, "generic", f"Error: {e}")


def _synthesize_play_loop(args, voice_name: str, label: str, result, force: bool) -> None:
    """Bucle interactivo de `speech synthesize --play` (§2.4).

    Reproduce la toma y ofrece cuatro opciones por un menú numerado en stderr:
      1) reproducir otra vez (mismos bytes en memoria, cero síntesis),
      2) aceptar y guardar (persiste la toma que sonó, exit 0),
      3) rechazar y regenerar (re-sintetiza y vuelve a preguntar),
      4) rechazar y descartar (exit 0, sin escribir).
    Ctrl-D (EOF) mapea a «descartar y salir». La regla 2 garantiza que aquí
    nunca coexiste --json, así que la pregunta y los avisos van por stderr sin
    contaminar ningún payload.
    """
    from . import synthetic_speech

    _play_audio(result.audio_bytes)
    while True:
        print(
            "\n¿Qué quieres hacer con esta toma?\n"
            "  1) Reproducir otra vez\n"
            "  2) Aceptar y guardar\n"
            "  3) Rechazar y regenerar\n"
            "  4) Rechazar y descartar",
            file=sys.stderr,
        )
        try:
            choice = input("Opción [1-4]: ").strip()
        except EOFError:
            # Ctrl-D: descartar y salir (exit 0, sin persistir).
            print("\nDescartado: no se guardó nada.", file=sys.stderr)
            return

        if choice == "1":
            _play_audio(result.audio_bytes)
        elif choice == "2":
            # Revalida la colisión al escribir: la ventana del bucle pudo tomar
            # la etiqueta. Sin --force, colisión → exit 6 (§2.4).
            if not force and synthetic_speech.exists(voice_name, label):
                raise CliError(
                    EXIT_STATE_CONFLICT, "label_exists",
                    f"Error: la etiqueta '{label}' quedó ocupada para la voz "
                    f"'{voice_name}' mientras decidías. Usa --force o elige otra.",
                )
            synthetic_speech.save(voice_name, label, result.audio_bytes, args.text)
            print(f"Guardado: locución '{label}' (voz '{voice_name}').", file=sys.stderr)
            return
        elif choice == "3":
            result, _daemon_flag = _dispatch_synthesis(args, voice_name)
            _play_audio(result.audio_bytes)
        elif choice == "4":
            print("Descartado: no se guardó nada.", file=sys.stderr)
            return
        else:
            print("Opción no válida; escribe 1, 2, 3 o 4.", file=sys.stderr)


@timed_command
def cmd_speech_play(args):
    """Reproduce una locución guardada (no toca el modelo ni el daemon)."""
    from . import synthetic_speech

    voice_name = _validate_identifier(getattr(args, "voice", None) or "default", "voz")
    label = _validate_identifier(args.label, "etiqueta")
    _require_voice_exists(voice_name)

    # La existencia de la locución la decide el WAV (§2.8): ausente → 3.
    if not synthetic_speech.exists(voice_name, label):
        raise CliError(
            EXIT_NOT_FOUND, "label_not_found",
            f"Error: la locución '{label}' no existe para la voz '{voice_name}'.",
        )

    with open(synthetic_speech.wav_path(voice_name, label), "rb") as f:
        audio_bytes = f.read()
    _play_audio(audio_bytes)

    if getattr(args, "json", False):
        emit_json({"voice": voice_name, "label": label})


def cmd_speech_list(args):
    """Lista las locuciones guardadas (opcionalmente filtradas por voz)."""
    from . import synthetic_speech

    voice_filter = getattr(args, "voice", None)
    if voice_filter is not None:
        voice_filter = _validate_identifier(voice_filter, "voz")
        _require_voice_exists(voice_filter)

    entries = synthetic_speech.list_entries(voice=voice_filter)

    if getattr(args, "json", False):
        # Texto completo en el payload legible por máquina (§2.10).
        emit_json({"synthetic_speech": entries})
        return

    if not entries:
        print("No hay locuciones guardadas.")
        return

    # Texto truncado en la salida humana (§2.8).
    for e in entries:
        text = e["text"]
        if text is None:
            shown = "(sin metadatos)"
        elif len(text) > 60:
            shown = text[:57] + "..."
        else:
            shown = text
        print(f"[{e['voice']}] {e['label']}: {shown}")


@timed_command
def cmd_speech_remove(args):
    """Borra una locución guardada (WAV + sidecar)."""
    from . import synthetic_speech

    voice_name = _validate_identifier(getattr(args, "voice", None) or "default", "voz")
    label = _validate_identifier(args.label, "etiqueta")
    _require_voice_exists(voice_name)

    # `remove` borra ambos archivos si están y devuelve si algo existía; una
    # locución ausente (ni WAV ni sidecar) sale 3 (§2.8).
    if not synthetic_speech.remove(voice_name, label):
        raise CliError(
            EXIT_NOT_FOUND, "label_not_found",
            f"Error: la locución '{label}' no existe para la voz '{voice_name}'.",
        )

    if getattr(args, "json", False):
        emit_json({"voice": voice_name, "label": label})
    else:
        print(f"Locución '{label}' (voz '{voice_name}') eliminada.")


@timed_command
def cmd_voice_clone(args):
    """Clona una voz a partir de dos archivos de audio (timbre + habla).

    Valida y copia los audios, y luego precomputa sus conditionals a disco
    (`conditionals.pt`) para que toda síntesis posterior los cargue en vez de
    recomputarlos. Exige el modelo en caché (`setup` previo), porque el
    precómputo ejecuta el modelo. Si hay un daemon activo, el precómputo lo hace
    con el modelo caliente (sin carga en frío); si no, carga el modelo en modo
    directo. Un fallo del precómputo no aborta el clonado: la voz queda
    registrada y el primer `speech say --voice <nombre>` recae en el cómputo
    on-the-fly.
    """
    from . import voices
    try:
        # Exige el modelo en caché antes de tocar el filesystem: el precómputo lo
        # necesita, así que un modelo ausente aborta con exit 2 sin dejar una voz
        # a medias. Las descargas son responsabilidad exclusiva de 'setup'.
        _require_model_cached()

        ref_path, speech_path = voices.clone_voice_files(
            name=args.name,
            timbre_reference=args.timbre_reference,
            speech_reference=args.speech_reference,
            force=getattr(args, "force", False),
        )

        # Precómputo daemon-first: con daemon activo se aprovecha el modelo
        # caliente; si no, se carga en frío en modo directo. El fallo se degrada
        # a aviso (register + lazy fallback), no aborta el clonado.
        precomputed = _precompute_cloned_voice(args)

        if getattr(args, "json", False):
            emit_json({
                "name": args.name,
                "reference": str(ref_path),
                "speech": str(speech_path),
                "precomputed": precomputed,
            })
            return

        print(f"Voz '{args.name}' clonada:")
        print(f"  timbre (reference): {ref_path}")
        print(f"  habla (conditioning): {speech_path}")
        if precomputed:
            print("  conditionals: precomputados")
        else:
            print("  conditionals: se computarán en la primera síntesis")

    except voices.VoiceExistsError as e:
        raise CliError(EXIT_STATE_CONFLICT, "voice_exists",
                        f"Error al clonar la voz: {e}")
    except ValueError as e:
        raise CliError(EXIT_INVALID_INPUT, "usage_error",
                        f"Error al clonar la voz: {e}")
    except Exception as e:
        raise CliError(EXIT_ERROR, "generic", f"Error al clonar la voz: {e}")


def _precompute_cloned_voice(args) -> bool:
    """Precomputa los conditionals de la voz recién clonada; True si tuvo éxito.

    Honra los tres modos de despacho (§2.5), en simetría con las dos superficies
    que sintetizan:
      --daemon:    exige el daemon; exit 5 si no está activo (no degrada).
      --no-daemon: fuerza la ruta directa (carga el modelo en frío).
      sin flags:   autodetección: modelo caliente del daemon si está, directo si no.
    Salvo con --daemon caído (exit 5), cualquier fallo del precómputo se captura,
    se avisa por stderr y se devuelve False para que el clonado siga siendo
    exitoso (la voz queda registrada y el primer `speech synthesize` computa los conditionals).
    """
    from .daemon import is_daemon_running, DaemonIPCClient

    # --daemon exige el daemon: su ausencia sale 5 (CliError escapa del clonado).
    if getattr(args, "daemon", False):
        if not is_daemon_running():
            raise CliError(
                EXIT_DAEMON_UNREACHABLE, "daemon_unreachable",
                "Error: se exigió el daemon (--daemon) para precomputar la voz "
                "pero no está activo. Inícialo con 'tts-sidecar daemon start'.",
            )
        return DaemonIPCClient().precompute_voice(args.name)

    try:
        if not getattr(args, "no_daemon", False) and is_daemon_running():
            return DaemonIPCClient().precompute_voice(args.name)
        from .engine import ChatterboxEngine
        with Spinner("Cargando modelo…") as _sp:
            engine = ChatterboxEngine.get_instance(
                compute_backend=getattr(args, "compute_backend", "auto")
            )
            _sp.update("Precomputando conditionals…")
            engine.precompute_voice(args.name)
        return True
    except Exception as e:
        print(
            f"Advertencia: no se pudieron precomputar los conditionals de "
            f"'{args.name}': {e}. Se computarán en la primera síntesis.",
            file=sys.stderr,
        )
        return False


@timed_command
def cmd_voice_remove(args):
    """Elimina una voz registrada."""
    from . import voices

    try:
        if voices.remove_voice(args.name):
            if getattr(args, "json", False):
                emit_json({"name": args.name, "removed": True})
                return
            print(f"Voz '{args.name}' eliminada.")
        elif voices._resolve_voice_dir(args.name) is not None:
            # Existe pero no como voz de usuario: es una voz de fábrica
            print(
                f"Voz '{args.name}' es una voz de fábrica (solo lectura) y no puede eliminarse.",
                file=sys.stderr,
            )
            raise CliError(EXIT_INVALID_INPUT, "factory_voice",
                           f"Voz '{args.name}' es una voz de fábrica y no puede eliminarse.")
        else:
            raise CliError(EXIT_NOT_FOUND, "voice_not_found",
                           f"Voz '{args.name}' no encontrada.")

    except (PermissionError, OSError) as e:
        # En Windows, shutil.rmtree falla con PermissionError si
        # timbre-reference.wav/speech-reference.wav están abiertos por otro proceso (p. ej. el
        # daemon o un reproductor). Sin esta rama, el except genérico de abajo
        # reportaba el mismo mensaje que un nombre de voz inválido.
        print(
            f"Error al eliminar la voz '{args.name}': uno de sus archivos parece "
            "estar en uso. En Windows los bloqueos típicos son el daemon "
            "('tts-sidecar daemon stop' lo cierra), un reproductor de audio con el "
            ".wav abierto, el panel de vista previa del Explorador de Windows sobre "
            "la carpeta de la voz, o un antivirus escaneando el archivo. Cierra el "
            f"proceso que lo retiene y vuelve a intentarlo. Detalle: {e}",
            file=sys.stderr,
        )
        raise CliError(EXIT_STATE_CONFLICT, "voice_remove_io_error",
                        f"Error al eliminar la voz '{args.name}': archivo en uso.")
    except ValueError as e:
        raise CliError(EXIT_INVALID_INPUT, "invalid_voice_name",
                        f"Error al eliminar la voz: {e}")
    except Exception as e:
        raise CliError(EXIT_ERROR, "voice_remove_error",
                        f"Error al eliminar la voz: {e}")


def cmd_voice_list(args):
    """Lista todas las voces registradas."""
    from . import voices

    try:
        voice_list = voices.list_voices()

        if getattr(args, "json", False):
            emit_json({"voices": voice_list})
            return

        if voice_list:
            print("Voces registradas:")
            for voice in voice_list:
                print(f"  - {voice}")
        else:
            print("No hay voces registradas. Ejecuta:")
            print("  tts-sidecar voice clone --name mi_voz --timbre-reference timbre-reference.wav --speech-reference speech-reference.wav")

    except FileNotFoundError as e:
        raise CliError(EXIT_NOT_FOUND, "not_found",
                        f"Error: {e}\nRevisa el directorio de voces de usuario: {voices.voices_root()}")
    except Exception as e:
        raise CliError(EXIT_ERROR, "generic", f"Error al listar las voces: {e}")


def cmd_devices(args):
    """Lista los dispositivos de salida de audio."""
    from .audio import get_audio_devices

    try:
        devices = get_audio_devices()
    except Exception as e:
        raise CliError(EXIT_ERROR, "generic", f"Error al enumerar los dispositivos de audio: {e}")

    if getattr(args, "json", False):
        emit_json({"devices": devices})
        return

    print("Dispositivos de salida de audio:")
    for dev in devices:
        print(f"  [{dev['id']}] {dev['name']} (latencia: {dev['latency']*1000:.1f}ms)")


def cmd_version(args):
    """Muestra la versión de tts-sidecar."""
    from . import __version__

    if getattr(args, "json", False):
        emit_json({"name": "tts-sidecar", "version": __version__})
    else:
        print(f"tts-sidecar {__version__}")


def _environment_checks() -> list[tuple[str, str, str]]:
    """Chequeos de entorno compartidos por doctor y setup.

    Devuelve una lista de (status, name, detail) con status en PASS/FAIL:
    Chatterbox importable + librería de audio de la plataforma.
    """
    checks = []

    # Chequea Chatterbox
    try:
        import chatterbox
        checks.append(("PASS", "Chatterbox TTS", chatterbox.__version__))
    except ImportError:
        checks.append(("FAIL", "Chatterbox TTS", "NO INSTALADO (pip install chatterbox-tts)"))

    # Chequea la librería de audio. En las tres plataformas se reutiliza la
    # enumeración real de audio.py (no solo la disponibilidad del import), de
    # modo que el chequeo refleje el estado efectivo del subsistema: un host
    # sin audio real (sesiones RDP/headless, sin backend ALSA/CoreAudio)
    # importa la librería sin problema pero falla al enumerar en runtime.
    try:
        from .audio import get_audio_devices_with_status
        devices, degraded = get_audio_devices_with_status()
        system = platform.system()
        lib_name = {"Windows": "pycaw", "Linux": "sounddevice", "Darwin": "sounddevice"}.get(system, "audio")
        if degraded:
            if system == "Linux":
                detail = (
                    f"{lib_name} no pudo enumerar ningún dispositivo real. Puede ser "
                    "un host sin audio (headless/SSH), o que falte la librería nativa "
                    "PortAudio: instala 'libportaudio2' (Debian/Ubuntu, "
                    "'sudo apt install libportaudio2') o 'portaudio' (Fedora, "
                    "'sudo dnf install portaudio')."
                )
            else:
                detail = (
                    f"{lib_name} importado pero no se pudo enumerar ningún dispositivo real "
                    "(host sin audio o sin subsistema de sonido)"
                )
            checks.append(("FAIL", "Audio library", detail))
        else:
            checks.append(("PASS", "Audio library", f"{lib_name} ({system}) — {len(devices)} dispositivo(s)"))
    except ImportError:
        checks.append(("FAIL", "Audio library", "NO INSTALADO"))
    except Exception as e:
        checks.append(("FAIL", "Audio library", str(e)))

    return checks


def _check_avx2() -> tuple[str, str, str]:
    """Chequeo best-effort de AVX2 para doctor.

    Solo aplica a x86-64: en ARM (Apple Silicon, aarch64) PyTorch usa NEON y el
    chequeo se reporta como no aplicable. La detección evita dependencias
    nuevas: /proc/cpuinfo en Linux y sysctl en macOS Intel. Windows no expone
    los flags de CPU por una vía estándar de stdlib, así que allí se degrada a
    una nota informativa que remite al requisito documentado en USAGE.md.
    """
    import subprocess  # Import explícito de la función: lo usa la rama darwin (sysctl).

    machine = platform.machine().lower()
    if machine not in ("x86_64", "amd64"):
        return ("SKIP", "CPU AVX2", f"no aplica en {platform.machine()} (solo x86-64)")

    try:
        if sys.platform.startswith("linux"):
            flags = Path("/proc/cpuinfo").read_text(encoding="utf-8", errors="replace")
            if "avx2" in flags:
                return ("PASS", "CPU AVX2", "soportado")
            return (
                "WARN", "CPU AVX2",
                "no detectado en /proc/cpuinfo: PyTorch puede fallar al cargar "
                "en esta CPU (requisito documentado en USAGE.md)",
            )
        if sys.platform == "darwin":
            out = subprocess.run(
                ["sysctl", "-n", "machdep.cpu.leaf7_features"],
                capture_output=True, text=True, timeout=5,
            )
            if out.returncode == 0 and "AVX2" in out.stdout.upper():
                return ("PASS", "CPU AVX2", "soportado")
            return (
                "WARN", "CPU AVX2",
                "no detectado (machdep.cpu.leaf7_features): PyTorch puede fallar "
                "al cargar en esta CPU (requisito documentado en USAGE.md)",
            )
    except Exception as e:
        return ("SKIP", "CPU AVX2", f"no se pudo determinar ({e})")

    return (
        "SKIP", "CPU AVX2",
        "no verificable automáticamente en Windows; se requiere CPU x86-64 con "
        "AVX2 (~2015 en adelante, ver USAGE.md)",
    )


def _check_onedrive() -> tuple[str, str, str]:
    """Chequeo informativo (WARN) de data_root() bajo OneDrive en Windows.

    En perfiles corporativos, LOCALAPPDATA puede caer bajo una jerarquía de
    OneDrive, exponiendo las voces de usuario a file locks y placeholders
    «a petición» (Files On-Demand) que causan fallos de lectura esporádicos e
    inatribuibles. El software no controla ese entorno, así que el chequeo solo
    da visibilidad en el punto de provisión (doctor): es puramente advisory,
    como el de AVX2 y la RAM — nunca altera el exit code ni la invariante de
    rutas (data_root sigue siendo LOCALAPPDATA).
    """
    if sys.platform != "win32":
        return ("SKIP", "OneDrive user-data-dir", "no aplica fuera de Windows")

    from . import paths

    data_root = paths.data_root()
    resolved = os.path.abspath(data_root).lower()

    # Raíz de sincronización de OneDrive desde las variables de entorno que el
    # cliente de OneDrive expone; si data_root() es prefijo de alguna, las voces
    # de usuario quedan bajo sincronización.
    onedrive_roots = [
        os.environ.get("OneDrive"),
        os.environ.get("OneDriveCommercial"),
    ]
    for root in onedrive_roots:
        if root and resolved.startswith(os.path.abspath(root).lower()):
            return (
                "WARN", "OneDrive user-data-dir",
                f"data_root() ({data_root}) está bajo la sincronización de OneDrive "
                f"({root}); excluye la carpeta de la sincronización o deshabilita "
                "Files On-Demand para ella para evitar file locks y placeholders "
                "a petición en las voces de usuario.",
            )

    # Respaldo por patrón de ruta: algún perfil corporativo monta OneDrive sin
    # exponer las variables de entorno (rutas de tipo 'OneDrive - Empresa').
    if "onedrive" in resolved:
        return (
            "WARN", "OneDrive user-data-dir",
            f"data_root() ({data_root}) contiene 'onedrive' en la ruta; puede "
            "estar bajo sincronización de OneDrive. Excluye la carpeta de la "
            "sincronización o deshabilita Files On-Demand para ella para evitar "
            "file locks y placeholders a petición en las voces de usuario.",
        )

    return ("PASS", "OneDrive user-data-dir", "no detectado")


def cmd_doctor(args):
    """Ejecuta los chequeos de diagnóstico."""
    from . import voices

    # lista de (status, name, detail) con status en PASS/FAIL/SKIP
    checks = _environment_checks()

    # Chequea el modelo: verifica por idioma qué hay en caché (sin cargar ni descargar)
    try:
        from .model_cache import is_model_cached
        for language, model in (("es-latam", "es-mx-latam"), ("en", "en")):
            if is_model_cached(model):
                checks.append(("PASS", f"Chatterbox model ({language})", f"{model} presente en la caché"))
            else:
                checks.append((
                    "FAIL", f"Chatterbox model ({language})",
                    f"{model} no está en caché (ejecuta: tts-sidecar setup --language {language})",
                ))
    except Exception as e:
        checks.append(("FAIL", "Chatterbox model", f"{e} (ejecuta: tts-sidecar setup)"))

    # Chequea el directorio de voces de usuario
    voices_path = voices.voices_root()
    count = len(voices.list_voices())
    if os.path.exists(voices_path) or count:
        checks.append(("PASS", "Voices directory", f"{count} voz(voces) disponible(s)"))
    else:
        checks.append(("SKIP", "Voices directory", "sin voces de usuario aún (opcional)"))

    # Chequea la RAM total (advisory): por debajo del umbral recomendado la
    # síntesis funciona pero puede paginar en textos largos. Es un WARN, no un
    # FAIL: no cuenta como chequeo fallido ni altera el exit code.
    try:
        import psutil
        total = psutil.virtual_memory().total
        total_gb = total / (1024 ** 3)
        if total < RECOMMENDED_RAM_BYTES:
            checks.append((
                "WARN", "RAM",
                f"{total_gb:.1f} GB detectados; se recomiendan 8 GB "
                "(4 GB mínimo). La síntesis puede paginar con textos largos.",
            ))
        else:
            checks.append(("PASS", "RAM", f"{total_gb:.1f} GB"))
    except Exception as e:
        # psutil no disponible o error de lectura: se omite sin penalizar.
        checks.append(("SKIP", "RAM", f"no se pudo determinar ({e})"))

    # Chequea AVX2 (advisory): PyTorch lo requiere en x86-64 y falla en
    # runtime sin diagnóstico en CPUs pre-2015. Detección best-effort por SO
    # (sin dependencia nueva): /proc/cpuinfo en Linux, sysctl en macOS Intel;
    # en Windows no hay vía estándar y el chequeo se degrada a una nota
    # informativa. Como la RAM, es un WARN: nunca altera el exit code.
    checks.append(_check_avx2())

    # Chequea si data_root() de Windows cae bajo la sincronización de OneDrive
    # (advisory): las voces de usuario quedarían expuestas a file locks y
    # placeholders a petición. Es puramente informativo (WARN), igual que AVX2
    # y RAM: no altera el exit code. Ver _check_onedrive.
    checks.append(_check_onedrive())

    # Solo FAIL cuenta como fallo: WARN/SKIP no penalizan el exit code.
    checks_failed = sum(1 for status, _, _ in checks if status == "FAIL")
    checks_passed = sum(1 for status, _, _ in checks if status == "PASS")

    if getattr(args, "json", False):
        emit_json({
            "python": sys.version,
            "platform": f"{platform.system()} {platform.release()}",
            "checks": [{"status": s, "name": n, "detail": d} for s, n, d in checks],
            "passed": checks_passed,
            "failed": checks_failed,
        })
        if checks_failed > 0:
            # Salida por veredicto: el reporte ya se emitió como único objeto
            # JSON; main() honra este retorno como sys.exit(1) sin adjuntar un
            # objeto 'error'. El exit 1 de doctor es un veredicto, no un fallo.
            return EXIT_ERROR
        return

    print("=== TTS Sidecar Doctor ===\n")
    print(f"Python: {sys.version}")
    print(f"Plataforma: {platform.system()} {platform.release()}")
    print()
    for status, name, detail in checks:
        print(f"[{status}] {name}: {detail}")
    print()
    print(f"Chequeos: {checks_passed} exitosos, {checks_failed} fallidos")

    if checks_failed > 0:
        # Veredicto: el reporte ya se imprimió; main() sale con 1 sin duplicar
        # la línea de resumen en stderr como haría CliError.
        return EXIT_ERROR


def _path_symlink() -> Path:
    """Ruta del symlink de PATH que setup gestiona en Unix (~/.local/bin/tts-sidecar).

    Es la misma ubicación en Linux (integración de $APPIMAGE) y en macOS
    (symlink del one-liner install-macos.sh), por lo que las ramas Linux y macOS
    del uninstall la comparten.
    """
    return Path.home() / ".local" / "bin" / "tts-sidecar"


def _integrate_linux_path():
    """Crea/actualiza el symlink ~/.local/bin/tts-sidecar → $APPIMAGE.

    Solo actúa en Linux cuando el proceso corre desde un AppImage (el runtime
    expone la ruta absoluta del archivo en la variable de entorno APPIMAGE).
    Fuera de ese contexto no toca el filesystem. ~/.local/bin es el directorio
    de usuario estándar XDG, presente en el PATH por defecto de las distros
    modernas, sin necesidad de sudo.

    `APPIMAGE` es un contrato oficial soportado, no solo el mecanismo interno
    del runtime AppImage: `install-linux.sh` la exporta explícitamente tras instalar
    el AppImage en `~/.local/opt/tts-sidecar/` y antes de invocar `setup`, así
    que cualquier valor externo de `APPIMAGE` que apunte a un archivo existente
    es una entrada válida.
    """
    appimage = os.environ.get("APPIMAGE")
    if sys.platform != "linux" or not appimage:
        return
    if not Path(appimage).is_file():
        print(
            f"\n[SKIP] PATH: APPIMAGE={appimage} no apunta a un archivo existente; no se modifica.",
            file=sys.stderr,
        )
        return

    link = _path_symlink()
    link.parent.mkdir(parents=True, exist_ok=True)
    if link.is_symlink():
        link.unlink()
    elif link.exists():
        # Un archivo regular homónimo no es nuestro: no se sobrescribe.
        print(f"\n[SKIP] PATH: {link} existe y no es un symlink; no se modifica.", file=sys.stderr)
        return
    link.symlink_to(appimage)
    print(f"\n[PASS] PATH: symlink creado {link} -> {appimage}", file=sys.stderr)
    print("El comando 'tts-sidecar' queda disponible por nombre en la terminal.", file=sys.stderr)
    print("Para revertirlo: tts-sidecar setup --remove-path", file=sys.stderr)

    if str(link.parent) not in os.environ.get("PATH", "").split(os.pathsep):
        print(f"[WARN] {link.parent} no está en el PATH de esta sesión.", file=sys.stderr)
        print('Añade esta línea a tu shell profile (~/.bashrc, ~/.zshrc, ...):', file=sys.stderr)
        print('    export PATH="$HOME/.local/bin:$PATH"', file=sys.stderr)
        print(
            "Después recarga el shell para que surta efecto: `exec $SHELL` "
            "(o abre una terminal nueva).",
            file=sys.stderr,
        )


def _remove_linux_path() -> bool:
    """Elimina el symlink de PATH creado por setup (rama --remove-path).

    Devuelve True si el symlink existía y se eliminó, False si no había nada
    que quitar (el caso de error — un archivo regular homónimo — aborta).
    """
    link = _path_symlink()
    if link.is_symlink():
        link.unlink()
        print(f"Symlink eliminado: {link}", file=sys.stderr)
        return True
    elif link.exists():
        raise CliError(EXIT_STATE_CONFLICT, "state_conflict",
                         f"Error: {link} existe pero no es un symlink; no se elimina.")
    else:
        print(f"No hay nada que quitar: {link} no existe.", file=sys.stderr)
        return False


def _linux_install_dir() -> Path:
    """Directorio de instalación del AppImage en Linux (~/.local/opt/tts-sidecar).

    Es propiedad exclusiva del proyecto: lo crea install-linux.sh y solo contiene los
    AppImages versionados. --uninstall lo borra por completo.
    """
    return Path.home() / ".local" / "opt" / "tts-sidecar"


def _uninstall_cleanup_data(args, json_mode):
    """Paso «datos independientes» del uninstall, compartido por las tres ramas.

    Encadena `cleanup --all` (modelo + voces de usuario) —siempre el primer
    borrado del orden unificado— y, tras un cleanup no cancelado, elimina el
    directorio raíz de datos (`data_root()`) si quedó vacío. `cleanup` solo borra
    `voices/` dentro de esa raíz, y `voices_root()` la re-crea al resolverse
    (`mkdir` incondicional en paths.py), así que sin este paso quedaría un
    directorio vacío, contra el residuo cero de la spec.

    Devuelve `(removed_paths, cancelled)`. Con `cancelled=True` (respuesta
    negativa o stdin cerrado, misma política que cmd_cleanup) el llamador aborta
    la desinstalación completa sin tocar el PATH ni el binario: es la
    cancelación atómica que habilita el orden unificado. El camino «no hay nada
    que limpiar» NO es una cancelación (rutas vacías, `cancelled=False`) y la
    desinstalación continúa. Bajo `--json` el stdout informativo del cleanup se
    redirige a stderr para no contaminar el único payload final.
    """
    print("\nEliminando los datos del proyecto (modelo y voces de usuario)...", file=sys.stderr)
    cleanup_args = argparse.Namespace(
        model=False,
        voices=False,
        all=True,
        dry_run=False,
        yes=getattr(args, "yes", False),
        json=False,
        cleanup_parser=getattr(args, "cleanup_parser", None),
    )
    if json_mode:
        import contextlib
        with contextlib.redirect_stdout(sys.stderr):
            result = cmd_cleanup(cleanup_args)
    else:
        result = cmd_cleanup(cleanup_args)

    if result.cancelled:
        return [], True

    removed = list(result.removed)

    from . import paths
    root = Path(paths.data_root())
    if root.exists() and not any(root.iterdir()):
        root.rmdir()
        print(f"Directorio de datos eliminado: {root}", file=sys.stderr)
        removed.append(root)

    return removed, False


def _uninstall(args):
    """Despacha `setup --uninstall` por SO sobre el contrato compartido.

    El guard de canal nativo (`is_frozen`) y el gate `--json`/`--yes` son comunes
    a las tres plataformas y viven solo aquí (no en las ramas). Luego despacha a
    la rama de la plataforma. Proceso no congelado (fuente o pip/uv) o plataforma
    fuera del dispatch → EXIT_INVALID_INPUT.
    """
    from . import paths

    # Guard de canal nativo: --uninstall solo aplica al AppImage / .app / Inno.
    if not paths.is_frozen():
        raise CliError(EXIT_NOT_APPLICABLE, "not_applicable",
                         "Error: 'setup --uninstall' solo aplica al canal nativo "
                         "(AppImage de Linux, .app de macOS o instalador de Windows).\n"
                         "  Si instalaste vía pip/uv, desinstala con: pip uninstall tts-sidecar")

    # Gate --json/--yes: la confirmación interactiva del cleanup contaminaría
    # stdout, reservado para el único payload JSON.
    if getattr(args, "json", False) and not getattr(args, "yes", False):
        raise CliError(EXIT_INVALID_INPUT, "usage_error",
                         "Error: setup --uninstall --json requiere --yes (la confirmación "
                         "interactiva del cleanup contaminaría stdout).")

    if sys.platform == "linux":
        _uninstall_linux(args)
    elif sys.platform == "darwin":
        _uninstall_macos(args)
    elif sys.platform == "win32":
        _uninstall_windows(args)
    else:
        raise CliError(EXIT_NOT_APPLICABLE, "not_applicable",
                         f"Error: 'setup --uninstall' no soporta la plataforma '{sys.platform}'.")


def _uninstall_linux(args):
    """Desinstala tts-sidecar en Linux en un paso (rama setup --uninstall).

    Sigue el orden unificado del contrato compartido: datos independientes
    (cleanup --all + data_root vacío) → integración de PATH (symlink) →
    componente ancla (directorio de instalación del AppImage). Con ese orden,
    cancelar el cleanup aborta la desinstalación sin borrar nada (salida 0). Con
    --yes se omite la confirmación del cleanup; con --json se emite un payload
    con las rutas eliminadas (incluidas las de datos y data_root si quedó vacío).

    Borrar el AppImage en ejecución es seguro en Linux: el unlink de un archivo
    abierto solo desliga el nombre; el inode sobrevive hasta que el proceso
    termina, así que la desinstalación se completa sin corromper el binario en
    curso.
    """
    import shutil

    json_mode = getattr(args, "json", False)
    removed_paths = []

    # 1. Datos independientes (helper compartido): cleanup --all + data_root vacío.
    data_removed, cancelled = _uninstall_cleanup_data(args, json_mode)
    if cancelled:
        print("\nDesinstalación cancelada: no se borró nada.", file=sys.stderr)
        return
    removed_paths.extend(data_removed)

    # 2. Integración de PATH: symlink ~/.local/bin/tts-sidecar.
    link = _path_symlink()
    if link.is_symlink():
        link.unlink()
        print(f"Symlink eliminado: {link}", file=sys.stderr)
        removed_paths.append(link)
    elif link.exists():
        print(f"[SKIP] {link} existe pero no es un symlink; no se elimina.", file=sys.stderr)
    else:
        print(f"[SKIP] Symlink de PATH: {link} no existe.", file=sys.stderr)

    # 3. Componente ancla: directorio de instalación del AppImage (borrado
    # quirúrgico acotado a esa ruta exacta), al final del orden unificado.
    install_dir = _linux_install_dir()
    expected = Path.home() / ".local" / "opt" / "tts-sidecar"
    if install_dir.exists():
        if install_dir != expected:
            raise RuntimeError(f"Ruta de instalación inesperada, no se borra: {install_dir}")
        shutil.rmtree(install_dir)
        print(f"Directorio de instalación eliminado: {install_dir}", file=sys.stderr)
        removed_paths.append(install_dir)
    else:
        print(f"[SKIP] Directorio de instalación: {install_dir} no existe.", file=sys.stderr)

    print("\n[PASS] Desinstalación completa.", file=sys.stderr)

    if json_mode:
        emit_json({
            "uninstall": True,
            "removed": [str(p) for p in removed_paths],
        })


def _uninstall_macos(args):
    """Desinstala tts-sidecar en macOS en un paso (rama setup --uninstall).

    Espeja install-macos.sh y los .command del .dmg, siguiendo el orden unificado
    (datos → PATH → binario). El `.app` se resuelve desde sys.executable, no se
    adivina: en modo congelado el binario corre en <app>/Contents/MacOS/, así que
    la raíz del bundle es parents[2]; el resolve() es obligatorio porque si el
    proceso se invocó vía el symlink ~/.local/bin/tts-sidecar, la ruta del
    ejecutable podría ser la del symlink y parents[2] apuntaría a $HOME. Un guard
    estructural (la ruta termina en .app) reemplaza al guard de ruta exacta de
    Linux, que aquí no aplica porque hay tres ubicaciones válidas (~/Applications
    del one-liner, /Applications del .dmg y del Cask).

    La instalación por Homebrew Cask se difiere a `brew uninstall --cask --zap`:
    el Cask mueve el bundle a la misma ruta que la vía .dmg, así que la ubicación
    no las distingue; la señal fiable es la metadata del Caskroom. Borrar el .app
    a mano dejaría esa metadata inconsistente, y una desinstalación parcial, un
    estado híbrido; por eso, si el Caskroom existe, se aborta sin tocar nada.

    Borrar el bundle en ejecución es seguro en macOS: el inode mapeado en memoria
    sobrevive hasta que el proceso termina (igual que el unlink de Linux). Los
    imports usados tras el rmtree (json) se resuelven antes: en el .app onedir no
    hay un squashfs que sobreviva al borrado, así que un import perezoso posterior
    se resolvería contra archivos ya inexistentes.
    """
    import shutil

    json_mode = getattr(args, "json", False)

    # 1. Localizar el .app desde sys.executable (se borra al final; se valida ya).
    app_bundle = Path(sys.executable).resolve().parents[2]
    if app_bundle.suffix != ".app":
        raise CliError(EXIT_NOT_APPLICABLE, "not_applicable",
                         f"Error: el ejecutable no reside en un bundle .app ({app_bundle}).\n"
                         "  'setup --uninstall' solo aplica a la instalación nativa de macOS.")

    # 2. Detección de Homebrew Cask por metadata del Caskroom (no por ruta del
    # .app). Si existe, la desinstalación no aplica: se difiere a brew --zap.
    brew_prefix = os.environ.get("HOMEBREW_PREFIX", "/opt/homebrew")
    caskroom_meta = Path(brew_prefix) / "Caskroom" / "tts-sidecar"
    if caskroom_meta.exists():
        raise CliError(EXIT_STATE_CONFLICT, "state_conflict",
                         "Error: tts-sidecar está instalado vía Homebrew Cask.\n"
                         "  Desinstálalo con: brew uninstall --cask --zap tts-sidecar\n"
                         "  (su 'zap' ya borra los datos; hacerlo a mano dejaría el Caskroom "
                         "inconsistente).")

    removed_paths = []

    # 3. Datos independientes (helper compartido): cleanup --all + data_root vacío.
    data_removed, cancelled = _uninstall_cleanup_data(args, json_mode)
    if cancelled:
        print("\nDesinstalación cancelada: no se borró nada.", file=sys.stderr)
        return
    removed_paths.extend(data_removed)

    # 4. Integración de PATH: symlink ~/.local/bin/tts-sidecar (misma ruta que Linux).
    link = _path_symlink()
    if link.is_symlink():
        link.unlink()
        print(f"Symlink eliminado: {link}", file=sys.stderr)
        removed_paths.append(link)
    elif link.exists():
        print(f"[SKIP] {link} existe pero no es un symlink; no se elimina.", file=sys.stderr)
    else:
        print(f"[SKIP] Symlink de PATH: {link} no existe.", file=sys.stderr)

    # 5. Componente ancla: el bundle .app, al final. Se re-aserta el guard de
    # sufijo inmediatamente antes del rmtree como defensa en profundidad.
    if app_bundle.suffix != ".app":
        raise RuntimeError(f"Ruta de bundle inesperada, no se borra: {app_bundle}")
    if app_bundle.exists():
        shutil.rmtree(app_bundle)
        print(f"Bundle .app eliminado: {app_bundle}", file=sys.stderr)
        removed_paths.append(app_bundle)
    else:
        print(f"[SKIP] Bundle .app: {app_bundle} no existe.", file=sys.stderr)

    print("\n[PASS] Desinstalación completa.", file=sys.stderr)

    if json_mode:
        emit_json({
            "uninstall": True,
            "removed": [str(p) for p in removed_paths],
        })


def _uninstall_windows(args):
    """Desinstala tts-sidecar en Windows en un paso (rama setup --uninstall).

    Windows origina el orden unificado: el SO mantiene un lock sobre el
    tts-sidecar.exe en ejecución, así que el propio proceso no puede borrar su
    binario ni esperar a un desinstalador que necesita borrarlo. Por eso el
    componente ancla se borra al final y de forma delegada:

    1. Se lee y valida `QuietUninstallString` del registro (HKCU, clave
       {AppId}_is1) primero, sin efectos: si falta (instalación no hecha por el
       instalador nativo), se aborta antes de borrar nada — análogo Windows de
       resolver el .app al inicio en macOS.
    2. `cleanup --all` corre en proceso (datos independientes).
    3. El desinstalador de Inno se lanza desacoplado con `subprocess.Popen` (sin
       espera), pasando el QuietUninstallString tal cual (ya incluye comillas y
       /SILENT): Inno se autocopia a %TEMP% y se relanza para poder borrar {app};
       revierte también el PATH de HKCU antes de borrar archivos.
    4. Se emite el payload y se retorna de inmediato para liberar el lock del
       .exe. La carrera es benigna: el CLI muere en milisegundos y el arranque
       de Inno (autocopia + relanzamiento) tarda más.

    Asimetría del payload: como el binario se borra *después* de la muerte del
    proceso, `removed` atestigua solo las rutas de datos (borradas en proceso);
    el directorio de instalación va en el campo aditivo `delegated`, nunca en
    `removed` (aún existe cuando se emite el payload; afirmarlo sería falso).
    """
    json_mode = getattr(args, "json", False)

    # 1. Leer y validar QuietUninstallString primero, sin efectos. winreg es
    # stdlib solo-Windows → import perezoso (no romper el import en Unix/tests).
    import winreg
    uninstall_key = (
        r"Software\Microsoft\Windows\CurrentVersion\Uninstall"
        r"\{E8A1B2C3-D4F5-6789-ABCD-EF0123456789}_is1"
    )
    try:
        with winreg.OpenKey(winreg.HKEY_CURRENT_USER, uninstall_key) as key:
            quiet_uninstall, _ = winreg.QueryValueEx(key, "QuietUninstallString")
    except OSError:
        quiet_uninstall = None
    if not quiet_uninstall:
        raise CliError(EXIT_NOT_APPLICABLE, "not_applicable",
                         "Error: no se encontró el registro del instalador nativo de Windows.\n"
                         "  'setup --uninstall' solo aplica a la instalación por el instalador "
                         "de Windows; desinstala desde Configuración → Aplicaciones.")

    removed_paths = []

    # 2. Datos independientes (helper compartido) en proceso, antes de delegar.
    data_removed, cancelled = _uninstall_cleanup_data(args, json_mode)
    if cancelled:
        print("\nDesinstalación cancelada: no se borró nada.", file=sys.stderr)
        return
    removed_paths.extend(data_removed)

    # 3. Lanzar el desinstalador de Inno desacoplado (sin espera).
    import subprocess
    install_dir = Path(sys.executable).resolve().parent
    subprocess.Popen(quiet_uninstall)

    # 4. Payload y retorno inmediato.
    print(
        "\n[PASS] Datos eliminados. El desinstalador de Windows completará el "
        "borrado del binario y la reversión del PATH.",
        file=sys.stderr,
    )

    if json_mode:
        emit_json({
            "uninstall": True,
            "removed": [str(p) for p in removed_paths],
            "delegated": [str(install_dir)],
        })


def _describe_provision_failure(e: Exception):
    """Terna (code, reason, message) accionable según la causa del fallo.

    Clasifica en cuatro familias observables sin depender del texto de la
    excepción: credenciales/acceso (HTTP 401/403, repos gated de HuggingFace),
    red (DNS, timeout, conexión), permisos y disco. Cada familia es una
    precondición de entorno, así que sale con EXIT_PRECONDITION_FAILED (8) y su
    reason; lo que no encaja en ninguna es un fallo genérico y se queda en
    EXIT_ERROR (1). El orden de los isinstance importa: RequestException hereda
    de OSError, así que las familias HTTP/red se descartan antes de diagnosticar
    disco.
    """
    import errno

    try:
        from huggingface_hub.errors import GatedRepoError, HfHubHTTPError
        if isinstance(e, GatedRepoError):
            return (EXIT_PRECONDITION_FAILED, "credentials", (
                "[FAIL] La provisión falló por acceso: el repo del modelo requiere "
                "autorización (gated). Acepta las condiciones en HuggingFace o define "
                f"un HF_TOKEN con acceso y reintenta 'tts-sidecar setup'. Detalle: {e}"
            ))
        if isinstance(e, HfHubHTTPError):
            status = getattr(getattr(e, "response", None), "status_code", None)
            if status in (401, 403):
                return (EXIT_PRECONDITION_FAILED, "credentials", (
                    f"[FAIL] La provisión falló por credenciales (HTTP {status}): el "
                    "token HF_TOKEN falta, expiró o no tiene acceso al repo del modelo. "
                    f"Revísalo y reintenta 'tts-sidecar setup'. Detalle: {e}"
                ))
    except ImportError:
        pass

    try:
        import requests
        if isinstance(e, requests.exceptions.RequestException):
            return (EXIT_PRECONDITION_FAILED, "network", (
                "[FAIL] La provisión falló por un problema de red: verifica tu conexión "
                "(o el proxy/firewall) y reintenta 'tts-sidecar setup'. "
                f"Detalle: {e}"
            ))
    except ImportError:
        pass

    if isinstance(e, PermissionError):
        return (EXIT_PRECONDITION_FAILED, "permissions", (
            "[FAIL] La provisión falló por permisos de escritura en la caché del "
            "modelo (~/.cache/huggingface o HF_HOME). Corrige los permisos y "
            f"reintenta 'tts-sidecar setup'. Detalle: {e}"
        ))
    if isinstance(e, OSError) and e.errno == errno.ENOSPC:
        return (EXIT_PRECONDITION_FAILED, "disk_full", (
            "[FAIL] La provisión falló por falta de espacio en disco. Libera espacio "
            f"y reintenta 'tts-sidecar setup'. Detalle: {e}"
        ))

    return (EXIT_ERROR, "provision_failed", f"[FAIL] La provisión falló: {e}")


def cmd_setup(args):
    """Provisiona el runtime: corre los chequeos de entorno y descarga el modelo si falta.

    En Linux, ejecutado desde un AppImage, también integra el comando en el PATH
    (symlink de $APPIMAGE en ~/.local/bin); --remove-path revierte ese symlink
    sin correr chequeos ni descargas.

    --uninstall desinstala en un paso en los tres SO (canal nativo): encadena
    'cleanup --all' (datos), revierte la integración de PATH y borra el binario,
    en ese orden. Solo aplica a la instalación nativa (AppImage / .app / Inno);
    desde fuente o pip/uv aborta remitiendo a 'pip uninstall'. En Unix el binario
    se borra en proceso —seguro: el unlink de un archivo/bundle abierto desliga
    el nombre pero el inode sobrevive hasta que el proceso termina—; en Windows
    se delega al desinstalador de Inno (el SO mantiene el lock del .exe).

    setup es provisión, no diagnóstico: el FAIL del chequeo de audio se degrada
    a WARN y la provisión continúa (la síntesis a disco con `speech synthesize
    --text T --label L` funciona sin subsistema de sonido, p. ej. en hosts
    headless/SSH). Cualquier
    otro FAIL sigue abortando. El rol diagnóstico lo cumple `doctor`, que
    conserva el FAIL de audio con salida 1.
    """
    if getattr(args, "uninstall", False):
        _uninstall(args)
        return

    if getattr(args, "remove_path", False):
        removed = _remove_linux_path()
        if getattr(args, "json", False):
            emit_json({"remove_path": True, "removed": removed})
        return

    print("=== TTS Sidecar Setup ===\n", file=sys.stderr)

    # 1. Integración de PATH (solo Linux desde AppImage; no-op en el resto).
    # Va antes de los chequeos para que un host degradado (p. ej. sin audio)
    # obtenga igualmente el comando en el PATH, en paridad con Windows y macOS;
    # el symlink es inocuo y reversible con --remove-path.
    _integrate_linux_path()

    # 2. Chequeos de entorno (implementación compartida con doctor). El FAIL
    # de audio es advisory aquí: sin sonido la reproducción no funcionará,
    # pero la síntesis a archivo sí, y bloquear la descarga del modelo dejaría
    # al host permanentemente sin provisión. Los demás FAIL abortan.
    for status, name, detail in _environment_checks():
        if status == "FAIL":
            if name == "Audio library":
                print(f"[WARN] {name}: {detail}", file=sys.stderr)
                print("[WARN] La reproducción de audio no estará disponible; "
                      "la síntesis a disco ('speech synthesize --text T --label L') "
                      "funciona igual.", file=sys.stderr)
                continue
            raise CliError(EXIT_PRECONDITION_FAILED, "environment_failed", f"[FAIL] {name}: {detail}")
        print(f"[{status}] {name}: {detail}", file=sys.stderr)

    # 3. Provisión del modelo (idempotente): descarga solo si no está ya en caché.
    # El modelo se descarga a la caché de HuggingFace (ver engine._download_model),
    # estable tanto desde fuente como en el ejecutable onedir. La ruta efectiva
    # respeta HF_HUB_CACHE/HF_HOME (misma resolución que el descargador).
    from .model_cache import hub_cache_path
    model_dir = str(hub_cache_path())

    # --force-update: borra los snapshots del modelo antes del gate para forzar
    # una re-descarga limpia. Borrado quirúrgico acotado a las carpetas
    # models--ResembleAI--* del proyecto, misma defensa en profundidad que cleanup.
    if getattr(args, "force_update", False):
        import shutil
        from .model_cache import model_cache_dirs

        def _dir_size(path):
            return sum(f.stat().st_size for f in path.rglob("*") if f.is_file())

        freed_total = 0
        print("\n[force-update] Eliminando el modelo en caché para re-descargarlo...", file=sys.stderr)
        for p in model_cache_dirs():
            if not p.name.startswith("models--ResembleAI--"):
                raise RuntimeError(f"Ruta inesperada fuera del proyecto: {p}")
            if p.exists():
                size = _dir_size(p)
                shutil.rmtree(p)
                freed_total += size
                print(f"[force-update] Eliminado: {p} ({size / 1_048_576:.1f} MB)", file=sys.stderr)
        if freed_total:
            print(f"[force-update] Espacio liberado total: {freed_total / 1_048_576:.1f} MB", file=sys.stderr)

    try:
        from .model_cache import is_model_cached, model_for

        language = getattr(args, "language", "all")
        # Default "all": baja ambos modelos (garantiza offline es+en desde el
        # primer uso). El flag sirve para reducir el alcance, no ampliarlo.
        requested_languages = ["es-latam", "en"] if language == "all" else [language]
        models_to_provision = [model_for(lang) for lang in requested_languages]

        def _emit_setup_json(results: dict):
            """Payload --json de setup (los [PASS]/[FAIL] de progreso van a stderr)."""
            if getattr(args, "json", False):
                emit_json({
                    "language": language,
                    "models": results,
                    "cache_dir": model_dir,
                })

        def _purge_incomplete():
            """Limpia los '*.incomplete' huérfanos tras una provisión completa.

            Solo se invoca con el/los modelo(s) ya íntegro(s) en caché: en ese
            punto ningún .incomplete es una descarga reanudable, son parciales
            huérfanos.
            """
            from .model_cache import purge_incomplete_downloads
            freed = purge_incomplete_downloads()
            if freed:
                print(
                    f"[PASS] Limpieza: {freed / 1_048_576:.1f} MB de descargas "
                    "parciales (.incomplete) huérfanas eliminadas.",
                    file=sys.stderr,
                )

        results = {}
        pending = []
        for model in models_to_provision:
            if is_model_cached(model):
                print(f"\n[PASS] El modelo '{model}' ya está en caché en: {model_dir}", file=sys.stderr)
                results[model] = {"already_cached": True, "downloaded": False}
            else:
                pending.append(model)
                results[model] = {"already_cached": False, "downloaded": False}

        if not pending:
            print("Provisión completa. No hay nada que descargar.", file=sys.stderr)
            _purge_incomplete()
            _emit_setup_json(results)
            return

        # Pre-chequeo de espacio en disco antes de descargar: cada modelo pesa
        # ~4 GB; por debajo del umbral (escalado por nº de modelos pendientes)
        # la descarga puede fallar a medias y dejar una caché truncada. Se
        # aborta antes de empezar. disk_usage exige una ruta existente: en una
        # máquina limpia la caché aún no existe, así que se sube al primer
        # ancestro presente.
        import shutil
        probe = hub_cache_path()
        while not probe.exists() and probe != probe.parent:
            probe = probe.parent
        free = shutil.disk_usage(probe).free
        required = MIN_FREE_DISK_BYTES * len(pending)
        if free < required:
            free_gb = free / (1024 ** 3)
            raise CliError(
            EXIT_PRECONDITION_FAILED,
            "disk_insufficient",
            f"[FAIL] Espacio en disco insuficiente: {free_gb:.1f} GB libres, "
            f"se requieren al menos {required // 1024 ** 3} GB para {len(pending)} "
            "modelo(s) (~4 GB c/u). Libera espacio y reintenta 'tts-sidecar setup'.",
        )

        # snapshot_download es solo red/disco, sin cargar el modelo en RAM
        # (~2 GB) como hacía ChatterboxEngine.get_instance; la carga real queda
        # para doctor/el primer 'speech synthesize', que ya validan el header safetensors.
        # revision fijada: la descarga es determinista y un push
        # posterior al repo del modelo no se propaga a los usuarios.
        from huggingface_hub import snapshot_download
        from .model_cache import MODELS, MODEL_REVISIONS
        for model in pending:
            print(f"\nDescargando el modelo {model}...", file=sys.stderr)
            print("(Puede tardar varios minutos en la primera ejecución)\n", file=sys.stderr)
            snapshot_download(
                repo_id=MODELS[model],
                revision=MODEL_REVISIONS[model],
                token=os.getenv("HF_TOKEN"),
            )
            results[model]["downloaded"] = True

        # El language pack es-mx-latam no incluye ve.safetensors (Voice
        # Encoder): se comparte con el modelo base. Solo hace falta este paso
        # explícito si es-mx-latam se acaba de descargar en esta corrida; si
        # "en" también se provisionó (su snapshot completo ya trae ve), la
        # descarga queda consolidada (is_ve_cached ya lo encuentra).
        if "es-mx-latam" in pending:
            from .model_cache import is_ve_cached, BASE_MODEL_REPO, BASE_MODEL_REVISION
            if not is_ve_cached():
                print("\nDescargando el Voice Encoder (ve.safetensors)...", file=sys.stderr)
                from huggingface_hub import hf_hub_download
                hf_hub_download(
                    repo_id=BASE_MODEL_REPO,
                    filename="ve.safetensors",
                    revision=BASE_MODEL_REVISION,
                    token=os.getenv("HF_TOKEN"),
                )
                print("[PASS] Voice Encoder descargado.", file=sys.stderr)

        print("\n[PASS] ¡Modelo(s) descargado(s) correctamente!", file=sys.stderr)
        print(f"  Ubicación: {model_dir}", file=sys.stderr)
        _purge_incomplete()
        _emit_setup_json(results)

    except Exception as e:
        code, reason, message = _describe_provision_failure(e)
        raise CliError(code, reason, message)


from typing import NamedTuple


class CleanupResult(NamedTuple):
    """Resultado observable de cmd_cleanup para los llamadores internos.

    - `removed`: rutas efectivamente eliminadas (vacío en dry-run, en «no hay
      nada que limpiar» y cuando el usuario cancela).
    - `cancelled`: True solo cuando el usuario declinó la confirmación
      interactiva (respuesta negativa o stdin cerrado). El camino «no hay nada
      que limpiar» y el dry-run NO son cancelaciones (rutas vacías, cancelled
      apagado). El uninstall usa esta bandera para abortar atómicamente sin
      borrar el PATH ni el binario.
    """
    removed: list
    cancelled: bool


def cmd_cleanup(args):
    """Desaprovisiona los datos del proyecto: modelos en caché y/o voces de usuario.

    Borrado quirúrgico: solo las carpetas de los dos repos HF del proyecto
    (model_cache_dirs + sus locks huérfanos en model_lock_dirs) y el
    directorio de voces de usuario; nunca la caché de HuggingFace completa ni
    datos de otros proyectos.

    Devuelve un `CleanupResult` (rutas eliminadas + bandera de cancelación) en
    todos sus caminos, para que el uninstall encadenado pueda distinguir un
    borrado exitoso de una cancelación y atestiguar las rutas en su payload. El
    contrato CLI standalone no cambia: main() ignora el retorno de los comandos.
    """
    import shutil

    json_mode = getattr(args, "json", False)
    # Con --json los listados informativos van a stderr: stdout queda reservado
    # para el único objeto JSON final (contrato stdout-datos/stderr-diagnóstico).
    info_out = sys.stderr if json_mode else sys.stdout

    if json_mode and not (getattr(args, "yes", False) or getattr(args, "dry_run", False)):
        raise CliError(
            EXIT_INVALID_INPUT,
            "usage_error",
            "Error: cleanup --json requiere --yes o --dry-run (la confirmación "
            "interactiva contaminaría stdout).",
        )

    def _emit_cleanup_json(removed_paths):
        if json_mode:
            emit_json({
                "removed": [str(p) for p in removed_paths],
                "dry_run": getattr(args, "dry_run", False),
            })

    do_model = getattr(args, "model", False) or getattr(args, "all", False)
    do_voices = getattr(args, "voices", False) or getattr(args, "all", False)
    do_synthetic = getattr(args, "synthetic_speech", False) or getattr(args, "all", False)

    if not do_model and not do_voices and not do_synthetic:
        # Sin flags no se borra nada: se muestra la ayuda del comando
        # (a stderr en modo --json, para no contaminar stdout).
        if json_mode:
            args.cleanup_parser.print_usage(sys.stderr)
            _emit_cleanup_json([])
            return CleanupResult([], False)
        args.cleanup_parser.print_help()
        return CleanupResult([], False)

    targets = []
    if do_model:
        from .model_cache import model_cache_dirs, model_lock_dirs
        for p in model_cache_dirs():
            # Defensa en profundidad: solo carpetas de modelos del proyecto.
            if not p.name.startswith("models--ResembleAI--"):
                raise RuntimeError(f"Ruta inesperada fuera del proyecto: {p}")
            targets.append((p, "modelo"))
        for p in model_lock_dirs():
            # Mismo repo, misma defensa: lock huérfano que huggingface_hub
            # deja en .locks/<repo>/ y no limpia solo tras la descarga.
            if not p.name.startswith("models--ResembleAI--"):
                raise RuntimeError(f"Ruta inesperada fuera del proyecto: {p}")
            targets.append((p, "lock de modelo"))
    if do_voices:
        from . import voices
        targets.append((Path(voices.voices_root()), "voces de usuario"))
    if do_synthetic:
        # --synthetic-speech (y --all) borran la raíz del almacén entera,
        # 'default' incluido (§2.11).
        from . import synthetic_speech
        targets.append((Path(synthetic_speech.store_root()), "habla sintética"))
    elif do_voices:
        # Arrastre de --voices: solo los namespaces de habla sintética de las
        # voces que se borran, excepto 'default' (voz de fábrica de solo lectura
        # que --voices no borra). Con la raíz separada del registro, el arrastre
        # es código explícito y no un efecto del rmtree (§2.8, §2.11).
        from . import synthetic_speech
        store_root = Path(synthetic_speech.store_root())
        if store_root.exists():
            for entry in sorted(os.listdir(store_root)):
                if entry == "default":
                    continue
                namespace = store_root / entry
                if namespace.is_dir():
                    targets.append((namespace, "habla sintética de voz"))

    existing = [(p, kind) for p, kind in targets if p.exists()]

    if not existing:
        print("No hay nada que limpiar: ninguna de las rutas del proyecto existe.", file=info_out)
        _emit_cleanup_json([])
        return CleanupResult([], False)

    print("Rutas a eliminar:", file=info_out)
    for p, kind in existing:
        print(f"  [{kind}] {p}", file=info_out)

    if getattr(args, "dry_run", False):
        print("\n(dry-run) No se borró nada.", file=info_out)
        _emit_cleanup_json([p for p, _kind in existing])
        return CleanupResult([], False)

    if not getattr(args, "yes", False):
        # Inalcanzable en modo --json (el gate de arriba exige --yes o --dry-run).
        try:
            response = input("\n¿Eliminar estas rutas? (s/n): ").strip().lower()
        except EOFError:
            # stdin cerrado (invocado vía subprocess sin --yes) no debe
            # producir un traceback crudo indistinguible de un error real.
            print("\nCancelado: no se borró nada.")
            return CleanupResult([], True)
        if response not in ("s", "si", "sí", "y", "yes"):
            print("Cancelado: no se borró nada.")
            return CleanupResult([], True)

    for p, _kind in existing:
        shutil.rmtree(p)
        print(f"Eliminado: {p}", file=info_out)
    print(
        "Limpieza completa. 'tts-sidecar setup' reprovisiona el modelo cuando lo necesites.",
        file=info_out,
    )
    removed = [p for p, _kind in existing]
    _emit_cleanup_json(removed)
    return CleanupResult(removed, False)


def cmd_daemon(args):
    """Gestiona el daemon de tts-sidecar."""
    if args.action == "serve":
        # Servidor en primer plano. Lo usa el ejecutable congelado para autoinvocar
        # el daemon (el .exe no puede ejecutar `python -m ...`).
        # Exige el modelo en caché antes de cargar el engine, igual que 'start':
        # sin este gate, 'daemon serve' sin 'setup' dispararía la red de seguridad
        # del engine (descarga de ~4 GB). Las descargas son responsabilidad
        # exclusiva de 'setup'.
        language = getattr(args, "language", "all") or "all"
        _require_models_cached_for_daemon(language)
        from .daemon.run import serve
        serve(
            auto_restart=getattr(args, "auto_restart", False),
            max_retries=getattr(args, "max_retries", 0) or 0,
            language=language,
        )
        return

    from .daemon import DaemonManager

    manager = DaemonManager()

    if args.action == "start":
        # Exige que el/los modelo(s) a precargar estén en caché antes de
        # lanzar el servidor. Las descargas son responsabilidad exclusiva de 'setup'.
        language = getattr(args, "language", "all") or "all"
        _require_models_cached_for_daemon(language)
        json_mode = getattr(args, "json", False)
        success = manager.start(
            background=True,
            auto_restart=args.autorestart,
            max_retries=args.max_retries or 0,
            language=language,
        )
        # Levantar antes de emitir: en fallo, main() emite solo el objeto
        # 'error' bajo --json; el payload de acción se emite solo en éxito.
        if not success:
            if not json_mode:
                print("No se pudo iniciar el daemon", file=sys.stderr)
            raise CliError(EXIT_DAEMON_UNREACHABLE, "daemon_unreachable",
                           "No se pudo iniciar el daemon.")
        if json_mode:
            payload = {"action": "start"}
            pid = manager._read_pid()
            if pid is not None:
                payload["pid"] = pid
            emit_json(payload)
        else:
            print("Daemon iniciado correctamente")

    elif args.action == "stop":
        json_mode = getattr(args, "json", False)
        success = manager.stop()
        # Levantar antes de emitir: en fallo solo el objeto 'error'; el payload
        # de acción se emite solo en éxito.
        if not success:
            if not json_mode:
                print("No se pudo detener el daemon", file=sys.stderr)
            raise CliError(EXIT_DAEMON_UNREACHABLE, "daemon_unreachable",
                           "No se pudo detener el daemon.")
        if json_mode:
            emit_json({"action": "stop"})
        else:
            print("Daemon detenido")

    elif args.action == "restart":
        json_mode = getattr(args, "json", False)
        success = manager.restart()
        # Levantar antes de emitir: en fallo solo el objeto 'error'; el payload
        # de acción se emite solo en éxito.
        if not success:
            if not json_mode:
                print("No se pudo reiniciar el daemon", file=sys.stderr)
            raise CliError(EXIT_DAEMON_UNREACHABLE, "daemon_unreachable",
                           "No se pudo reiniciar el daemon.")
        if json_mode:
            payload = {"action": "restart"}
            pid = manager._read_pid()
            if pid is not None:
                payload["pid"] = pid
            emit_json(payload)
        else:
            print("Daemon reiniciado")

    elif args.action == "status":
        status = manager.status()

        if getattr(args, "json", False):
            emit_json(dict(status))
            return

        if status.get("running"):
            print(f"Daemon en ejecución:")
            print(f"  Estado: {status.get('status', 'desconocido')}")
            # model_loaded es un dict por idioma (§3.11): qué modelos están
            # calientes en RAM, no un booleano único.
            loaded = status.get("model_loaded") or {}
            cargados = ", ".join(lang for lang, ok in loaded.items() if ok) or "ninguno"
            print(f"  Modelos cargados: {cargados}")
            print(f"  Tiempo activo: {status.get('uptime_seconds', 0):.1f}s")
        else:
            print("Daemon no está en ejecución")


def top_level_subparsers(parser: argparse.ArgumentParser) -> argparse.Action:
    """Devuelve la acción `_SubParsersAction` de nivel superior de `parser`.

    argparse no expone esto como atributo público estable; se busca entre
    `parser._actions` en vez de depender de la variable local `subparsers` de
    `build_parser()` (que ya no está en su scope una vez construido el parser).
    Usado por `main()` (ayuda de un grupo sin sub-acción) y por el test
    estructural del contrato --json (recorrido de todos los subcomandos).
    """
    for action in parser._actions:
        if isinstance(action, argparse._SubParsersAction):
            return action
    raise RuntimeError("El parser no tiene subparsers de nivel superior")


class _CLIParser(argparse.ArgumentParser):
    """Parser CLI que traduce errores de análisis a CliError en vez de
    llamar a sys.exit, para que main() pueda capturarlos y traducirlos
    como parte del canal de error único."""

    def error(self, message: str) -> None:
        raise CliError(EXIT_INVALID_INPUT, "usage_error", message)


def build_parser() -> argparse.ArgumentParser:
    """Construye el parser completo del CLI (subcomandos, flags, despacho).

    Extraído de `main()` para que el contrato --json sea introspeccionable: el
    test estructural (tests/test_cli.py::TestJSONContractStructure) recorre
    este mismo parser para descubrir qué subcomandos declaran --json, en vez de
    mantener una lista aparte que puede desincronizarse del código real.
    """
    parser = _CLIParser(
        prog="tts-sidecar",
        description="TTS Sidecar - TTS 100% local con clonación de voz"
    )

    subparsers = parser.add_subparsers(dest="command", help="Comandos disponibles")

    # grupo de comandos speech (synthesize / say / play / list / remove)
    speech_parser = subparsers.add_parser("speech", help="Sintetiza, reproduce y gestiona habla")
    speech_subparsers = speech_parser.add_subparsers(dest="action", help="Acciones de habla")

    # speech synthesize: sintetiza y guarda en el almacén (persiste siempre).
    speech_synth = speech_subparsers.add_parser("synthesize", help="Sintetiza voz y la guarda en el almacén")
    speech_synth.add_argument("--text", "-t", required=True, help="Texto a sintetizar")
    speech_synth.add_argument("--label", "-l", required=True,
                              help="Etiqueta de la locución en el almacén (se normaliza a minúsculas: "
                                   "'Saludo' y 'saludo' son la misma etiqueta)")
    speech_synth.add_argument("--voice", "-v", help="Nombre de la voz a usar (default: 'default')")
    speech_synth.add_argument("--play", "-p", action="store_true",
                              help="Reproduce la toma y pregunta antes de guardar (requiere terminal; "
                                   "incompatible con --json)")
    speech_synth.add_argument("--force", "-f", action="store_true",
                              help="Sobrescribir la locución si la etiqueta ya existe para la voz")
    speech_synth.add_argument("--compute-backend", "-cb", default="auto",
                              choices=["auto", "cpu", "cuda", "mps"],
                              help="Backend de cómputo para la inferencia (solo surte efecto en la ruta "
                                   "directa); 'auto' detecta el mejor disponible (default: auto)")
    speech_synth.add_argument("--language", choices=["es-latam", "en"], default="es-latam",
                              help="Idioma/modelo de síntesis (default: es-latam); 'en' reutiliza el "
                                   "timbre clonado para producir audio en inglés (síntesis cross-lingual)")
    speech_synth.add_argument("--exaggeration", type=float, default=None,
                              help="Override de exaggeration (default: el de la ruta de --language)")
    speech_synth.add_argument("--cfg-weight", type=float, default=None,
                              help="Override de cfg_weight (default: el de la ruta de --language); "
                                   "0.0 no está permitido (crash conocido en el inglés base)")
    speech_synth.add_argument("--temperature", type=float, default=None,
                              help="Override de temperature (default: el de la ruta de --language)")
    speech_synth_daemon = speech_synth.add_mutually_exclusive_group()
    speech_synth_daemon.add_argument("--daemon", action="store_true",
                              help="Exige el daemon; si no está activo, sale con 5 (sin flags se sondea "
                                   "y se usa solo si responde). Mutuamente excluyente con --no-daemon")
    speech_synth_daemon.add_argument("--no-daemon", action="store_true",
                              help="Forzar modo directo, sin sondear el daemon. "
                                   "Mutuamente excluyente con --daemon")
    speech_synth.add_argument("--json", action="store_true",
                              help="Emitir a stdout un payload JSON (voz, etiqueta, tiempos t3/s3gen, "
                                   "vía daemon o no); incompatible con --play")
    speech_synth.set_defaults(func=cmd_speech_synthesize)

    speech_say = speech_subparsers.add_parser("say", help="Sintetiza voz y la reproduce")
    speech_say.add_argument("--text", "-t", required=True, help="Texto a sintetizar")
    speech_say.add_argument("--voice", "-v", help="Nombre de la voz a usar (auto-carga timbre-reference.wav + speech-reference.wav)")
    speech_say.add_argument("--compute-backend", "-cb", default="auto",
                              choices=["auto", "cpu", "cuda", "mps"],
                              help="Backend de cómputo para la inferencia; 'auto' detecta el mejor "
                                   "disponible (default: auto)")
    speech_say.add_argument("--language", choices=["es-latam", "en"], default="es-latam",
                              help="Idioma/modelo de síntesis (default: es-latam); 'en' reutiliza el "
                                   "timbre clonado para producir audio en inglés (síntesis cross-lingual)")
    speech_say.add_argument("--exaggeration", type=float, default=None,
                              help="Override de exaggeration (default: el de la ruta de --language)")
    speech_say.add_argument("--cfg-weight", type=float, default=None,
                              help="Override de cfg_weight (default: el de la ruta de --language); "
                                   "0.0 no está permitido (crash conocido en el inglés base)")
    speech_say.add_argument("--temperature", type=float, default=None,
                              help="Override de temperature (default: el de la ruta de --language)")
    speech_say_daemon = speech_say.add_mutually_exclusive_group()
    speech_say_daemon.add_argument("--daemon", action="store_true",
                              help="Usar el daemon sin sondeo previo; si falla, se reporta el error "
                                   "(sin flags, se sondea el daemon y se usa solo si responde). "
                                   "Mutuamente excluyente con --no-daemon (exit 2 si se combinan)")
    speech_say_daemon.add_argument("--no-daemon", action="store_true",
                              help="Forzar modo directo, sin sondear el daemon. "
                                   "Mutuamente excluyente con --daemon (exit 2 si se combinan)")
    speech_say.add_argument("--json", action="store_true",
                              help="Emitir a stdout un payload JSON con la voz efectiva usada")
    speech_say.set_defaults(func=cmd_speech_say)

    # speech play: reproduce una locución guardada (no toca modelo ni daemon).
    speech_play = speech_subparsers.add_parser("play", help="Reproduce una locución guardada")
    speech_play.add_argument("--label", "-l", required=True, help="Etiqueta de la locución (normalizada a minúsculas)")
    speech_play.add_argument("--voice", "-v", help="Nombre de la voz (default: 'default')")
    speech_play.add_argument("--json", action="store_true", help="Emitir JSON legible por máquina (voz, etiqueta)")
    speech_play.set_defaults(func=cmd_speech_play)

    # speech list: lista las locuciones guardadas (filtro opcional por voz).
    speech_list = speech_subparsers.add_parser("list", help="Lista las locuciones guardadas")
    speech_list.add_argument("--voice", "-v", help="Filtra por voz; sin él lista todas las voces")
    speech_list.add_argument("--json", action="store_true", help="Emitir JSON legible por máquina (texto completo)")
    speech_list.set_defaults(func=cmd_speech_list)

    # speech remove: borra una locución guardada.
    speech_remove = speech_subparsers.add_parser("remove", help="Borra una locución guardada")
    speech_remove.add_argument("--label", "-l", required=True, help="Etiqueta de la locución (normalizada a minúsculas)")
    speech_remove.add_argument("--voice", "-v", help="Nombre de la voz (default: 'default')")
    speech_remove.add_argument("--json", action="store_true", help="Emitir JSON legible por máquina (voz, etiqueta)")
    speech_remove.set_defaults(func=cmd_speech_remove)

    # grupo de comandos voice (list / clone / remove)
    voice_parser = subparsers.add_parser("voice", help="Gestiona las voces registradas")
    voice_subparsers = voice_parser.add_subparsers(dest="action", help="Acciones de voz")

    voice_list = voice_subparsers.add_parser("list", help="Lista las voces registradas")
    voice_list.add_argument("--json", action="store_true", help="Emitir JSON legible por máquina")
    voice_list.set_defaults(func=cmd_voice_list)

    voice_clone = voice_subparsers.add_parser("clone", help="Clona una voz desde dos archivos de audio")
    voice_clone.add_argument("--name", "-n", required=True, help="Nombre de la voz")
    voice_clone.add_argument("--timbre-reference", "-t", required=True,
                             help="Archivo de audio de referencia para el timbre (cualquier largo, se usa el audio completo)")
    voice_clone.add_argument("--speech-reference", "-s", required=True,
                             help="Archivo de audio de habla para el conditioning del T3 (10+ segundos de habla limpia)")
    voice_clone.add_argument("--force", "-f", action="store_true",
                             help="Sobrescribir la voz si ya existe (usuario o fábrica homónima)")
    voice_clone_daemon = voice_clone.add_mutually_exclusive_group()
    voice_clone_daemon.add_argument("--daemon", action="store_true",
                             help="Exige el daemon para precomputar los conditionals; si no está activo, "
                                  "sale con 5 (sin flags se sondea y se usa solo si responde). "
                                  "Mutuamente excluyente con --no-daemon")
    voice_clone_daemon.add_argument("--no-daemon", action="store_true",
                             help="Fuerza el precómputo en modo directo, sin sondear el daemon. "
                                  "Mutuamente excluyente con --daemon")
    voice_clone.add_argument("--json", action="store_true", help="Emitir JSON legible por máquina")
    voice_clone.set_defaults(func=cmd_voice_clone)

    voice_remove = voice_subparsers.add_parser("remove", help="Elimina una voz registrada")
    voice_remove.add_argument("--name", "-n", required=True, help="Nombre de la voz")
    voice_remove.add_argument("--json", action="store_true", help="Emitir JSON legible por máquina")
    voice_remove.set_defaults(func=cmd_voice_remove)

    # comando devices
    devices_parser = subparsers.add_parser("devices", help="Lista los dispositivos de audio")
    devices_parser.add_argument("--json", action="store_true", help="Emitir JSON legible por máquina")
    devices_parser.set_defaults(func=cmd_devices)

    # comando doctor
    doctor_parser = subparsers.add_parser("doctor", help="Ejecuta diagnósticos")
    doctor_parser.add_argument("--json", action="store_true", help="Emitir JSON legible por máquina")
    doctor_parser.set_defaults(func=cmd_doctor)

    # comando setup
    setup_parser = subparsers.add_parser("setup", help="Provisiona el runtime: corre chequeos, descarga el modelo si falta "
                                                       "y en Linux (AppImage) integra el comando en el PATH")
    # --remove-path, --force-update y --uninstall son modos mutuamente
    # excluyentes de setup: cada uno corta el flujo normal de provisión.
    setup_mode = setup_parser.add_mutually_exclusive_group()
    setup_mode.add_argument("--remove-path", action="store_true",
                            help="Elimina el symlink de PATH (~/.local/bin/tts-sidecar) creado por setup en Linux "
                                 "y termina sin correr chequeos ni descargas")
    setup_mode.add_argument("--force-update", action="store_true",
                            help="Elimina el modelo en caché y lo vuelve a descargar (fuerza una "
                                 "re-descarga limpia, p. ej. para actualizarlo)")
    setup_mode.add_argument("--uninstall", action="store_true",
                            help="Desinstala tts-sidecar en un paso (canal nativo, los 3 SO): encadena "
                                 "'cleanup --all', revierte la integración de PATH y borra el binario")
    setup_parser.add_argument("--yes", "-y", action="store_true",
                              help="Omite la confirmación interactiva del cleanup encadenado por --uninstall")
    setup_parser.add_argument("--language", choices=["es-latam", "en", "all"], default="all",
                              help="Idioma(s) a provisionar (default: all, ambos modelos)")
    setup_parser.add_argument("--json", action="store_true", help="Emitir JSON legible por máquina")
    setup_parser.set_defaults(func=cmd_setup)

    # comando cleanup
    cleanup_parser = subparsers.add_parser(
        "cleanup",
        help="Desaprovisiona los datos del proyecto (modelo descargado y/o voces de usuario)",
    )
    cleanup_parser.add_argument("--model", action="store_true",
                                help="Elimina el modelo descargado (solo las carpetas de este proyecto "
                                     "dentro de la caché de HuggingFace)")
    cleanup_parser.add_argument("--voices", action="store_true",
                                help="Elimina las voces de usuario registradas con 'voice clone' "
                                     "(arrastra sus locuciones de habla sintética, salvo las de 'default')")
    cleanup_parser.add_argument("--synthetic-speech", action="store_true",
                                help="Elimina la raíz de habla sintética entera "
                                     "(todas las locuciones guardadas con 'speech synthesize', 'default' incluida)")
    cleanup_parser.add_argument("--all", action="store_true",
                                help="Elimina el modelo, las voces de usuario y la habla sintética")
    cleanup_parser.add_argument("--dry-run", action="store_true",
                                help="Lista lo que se borraría sin borrar nada")
    cleanup_parser.add_argument("--yes", "-y", action="store_true",
                                help="Omite la confirmación interactiva")
    cleanup_parser.add_argument("--json", action="store_true",
                                help="Emitir JSON legible por máquina (requiere --yes o --dry-run)")
    cleanup_parser.set_defaults(func=cmd_cleanup, cleanup_parser=cleanup_parser)

    # comando daemon
    daemon_parser = subparsers.add_parser("daemon", help="Gestión del ciclo de vida del daemon")
    daemon_subparsers = daemon_parser.add_subparsers(dest="action", help="Acciones del daemon")

    daemon_start = daemon_subparsers.add_parser("start", help="Inicia el daemon")
    daemon_start.add_argument("--autorestart", action="store_true", help="Auto-reinicio en caso de crash")
    daemon_start.add_argument("--max-retries", type=int, help="Máximo de intentos de reinicio")
    daemon_start.add_argument("--language", choices=["es-latam", "en", "all"], default="all",
                              help="Idioma(s) a precargar en caliente (default: all, ambos modelos)")
    daemon_start.add_argument("--json", action="store_true", help="Emitir JSON legible por máquina")
    daemon_start.set_defaults(func=cmd_daemon)

    daemon_stop = daemon_subparsers.add_parser("stop", help="Detiene el daemon")
    daemon_stop.add_argument("--json", action="store_true", help="Emitir JSON legible por máquina")
    daemon_stop.set_defaults(func=cmd_daemon)

    daemon_restart = daemon_subparsers.add_parser("restart", help="Reinicia el daemon")
    daemon_restart.add_argument("--json", action="store_true", help="Emitir JSON legible por máquina")
    daemon_restart.set_defaults(func=cmd_daemon)

    daemon_status = daemon_subparsers.add_parser("status", help="Muestra el estado del daemon")
    daemon_status.add_argument("--json", action="store_true", help="Emitir JSON legible por máquina")
    daemon_status.set_defaults(func=cmd_daemon)

    daemon_serve = daemon_subparsers.add_parser("serve", help="Ejecuta el servidor del daemon en primer plano")
    daemon_serve.add_argument("--auto-restart", action="store_true", help="Auto-reinicio en caso de crash")
    daemon_serve.add_argument("--max-retries", type=int, default=0, help="Máximo de intentos de reinicio (0 = infinito)")
    daemon_serve.add_argument("--language", choices=["es-latam", "en", "all"], default="all",
                              help="Idioma(s) a precargar en caliente (default: all, ambos modelos)")
    daemon_serve.set_defaults(func=cmd_daemon)

    # comando version
    version_parser = subparsers.add_parser("version", help="Muestra la versión")
    version_parser.add_argument("--json", action="store_true", help="Emitir JSON legible por máquina")
    version_parser.set_defaults(func=cmd_version)

    return parser


def main():
    """Punto de entrada principal de la CLI."""
    bootstrap.apply()
    parser = build_parser()

    try:
        args = parser.parse_args()
    except CliError as e:
        _translate_cli_error(e)
        return

    if not args.command:
        parser.print_help()
        sys.exit(EXIT_OK)

    if not hasattr(args, "func"):
        top_level_subparsers(parser).choices[args.command].print_help()
        sys.exit(EXIT_OK)

    try:
        result = args.func(args)
    except CliError as e:
        _translate_cli_error(e)
    except KeyboardInterrupt:
        # Cierre limpio ante Ctrl+C: sin traceback, mensaje de una línea a stderr
        # y el código 130 convencional (128 + SIGINT). Solo actúa si la excepción
        # escapa hasta aquí: el shutdown graceful de uvicorn en 'daemon serve'
        # la maneja antes y no pasa por esta rama.
        _translate_cli_error(
            CliError(
                EXIT_INTERRUPTED, "interrupted",
                "Interrumpido por el usuario.",
            )
        )
    else:
        # Salida por veredicto: el comando ya emitió su payload propio y pide
        # salir con código ≠ 0 devolviendo un entero (p. ej. 'doctor' con FAIL).
        # main() sigue siendo el único punto de salida no-cero; no se adjunta
        # objeto 'error'. Un retorno None (o 0) conserva el exit 0 implícito.
        if isinstance(result, int) and result != 0:
            sys.exit(result)


if __name__ == "__main__":
    main()
