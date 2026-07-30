"""
Reproducción de audio nativa para Windows, Linux y macOS.
Usa APIs nativas de cada SO para un rendimiento óptimo.
"""

import logging
import platform
import sys
import wave
import io
from typing import Optional

import numpy as np

logger = logging.getLogger(__name__)

# Divisor de normalización PCM int16 -> float32 en [-1, 1). 2**15: el rango
# entero de int16 es [-32768, 32767] (asimétrico); dividir por 32768.0 (no por
# 32767.0) es la convención estándar para que -32768 mapee exactamente a -1.0,
# aceptando que +32767 quede a un ULP de +1.0 en vez de tocarlo.
INT16_MAX_F = 32768.0


class AudioPlayer:
    """
    Reproducción de audio multiplataforma usando APIs nativas.

    Prioridad por plataforma:
    1. Windows: winsound (built-in)
    2. Linux: sounddevice (PortAudio)
    3. macOS: afplay (nativo)
    """

    def __init__(self):
        self.system = platform.system()
        self._player = self._init_player()

    def _init_player(self):
        """Inicializa el player de audio apropiado para la plataforma."""
        if self.system == "Windows":
            return self._init_windows()
        elif self.system == "Darwin":
            return self._init_macos()
        elif self.system == "Linux":
            return self._init_linux()
        else:
            raise RuntimeError(f"Plataforma no soportada: {self.system}")

    def _init_windows(self):
        """Inicializa el player de audio para Windows."""
        # winsound es built-in: no requiere dependencias externas
        return WindowsAudioPlayer()

    def _init_macos(self):
        """Inicializa el player de audio para macOS usando afplay (built-in)."""
        try:
            import subprocess  # noqa: F401 — verificación de disponibilidad
            return MacOSAudioPlayer()
        except Exception as e:
            raise RuntimeError(f"Error al inicializar audio en macOS: {e}")

    def _init_linux(self):
        """Inicializa el player de audio para Linux."""
        try:
            import sounddevice as sd
            return SoundDevicePlayer(sd)
        except ImportError:
            raise ImportError(
                "No hay librería de reproducción de audio disponible para Linux. "
                "Instala sounddevice."
            )
        except OSError as e:
            # El paquete sounddevice SÍ está instalado, pero PortAudio (la
            # librería nativa del sistema que sounddevice carga vía CFFI al
            # importarse) no se encontró. Es el caso típico del canal PyPI en
            # Linux: el wheel de sounddevice no empaqueta PortAudio (a
            # diferencia del AppImage, que la trae embebida).
            raise OSError(
                "No se encontró la librería nativa PortAudio requerida por "
                "sounddevice. Instala el paquete del sistema: "
                "'sudo apt install libportaudio2' (Debian/Ubuntu) o "
                "'sudo dnf install portaudio' (Fedora). No es necesaria si "
                "solo usas 'speech synthesize' para guardar a archivo. "
                f"Detalle: {e}"
            ) from e

    def play(self, audio_bytes: bytes) -> None:
        """Reproduce audio desde bytes WAV."""
        self._player.play(audio_bytes)

    def play_file(self, file_path: str) -> None:
        """Reproduce audio desde un archivo WAV."""
        with open(file_path, 'rb') as f:
            audio_bytes = f.read()
        self.play(audio_bytes)


class WindowsAudioPlayer:
    """Reproducción de audio en Windows usando winsound (built-in)."""

    def play(self, audio_bytes: bytes) -> None:
        """Reproduce bytes WAV en Windows usando winsound built-in."""
        import winsound
        import tempfile
        import os

        # winsound.PlaySound requiere una ruta de archivo, no bytes en memoria
        with tempfile.NamedTemporaryFile(suffix='.wav', delete=False) as f:
            f.write(audio_bytes)
            temp_path = f.name

        try:
            winsound.PlaySound(temp_path, winsound.SND_FILENAME)
        finally:
            os.unlink(temp_path)


class MacOSAudioPlayer:
    """Reproducción de audio en macOS usando afplay (built-in)."""

    def __init__(self):
        import subprocess
        self.subprocess = subprocess

    def play(self, audio_bytes: bytes) -> None:
        """Reproduce audio usando afplay."""
        import tempfile
        import os

        # afplay requiere una ruta de archivo
        with tempfile.NamedTemporaryFile(suffix='.wav', delete=False) as f:
            f.write(audio_bytes)
            temp_path = f.name

        try:
            self.subprocess.run(['afplay', temp_path], check=True)
        finally:
            os.unlink(temp_path)


class SoundDevicePlayer:
    """Reproducción de audio multiplataforma usando sounddevice (PortAudio)."""

    def __init__(self, sd):
        self.sd = sd

    def play(self, audio_bytes: bytes) -> None:
        """Reproduce bytes WAV usando sounddevice."""
        wav_io = io.BytesIO(audio_bytes)
        with wave.open(wav_io, 'rb') as wf:
            n_channels = wf.getnchannels()
            sample_rate = wf.getframerate()
            sample_width = wf.getsampwidth()
            audio_data = wf.readframes(wf.getnframes())

        if sample_width != 2:
            raise ValueError(
                f"WAV con ancho de muestra no soportado ({sample_width * 8} bits); "
                "se espera PCM de 16 bits."
            )

        # Convierte a float32 normalizado en [-1, 1]
        audio_np = np.frombuffer(audio_data, dtype=np.int16)
        audio_np = audio_np.astype(np.float32) / INT16_MAX_F

        # Un WAV multicanal llega intercalado: sin el reshape, sounddevice lo
        # reproduciría como mono al doble de velocidad.
        if n_channels > 1:
            audio_np = audio_np.reshape((-1, n_channels))

        self.sd.play(audio_np, samplerate=sample_rate, blocking=True)


def get_audio_devices_with_status() -> tuple[list[dict], bool]:
    """
    Lista los dispositivos de salida de audio disponibles.

    Returns:
        Tupla (dispositivos, degraded): `degraded` es True cuando la enumeración
        real falló y se devolvió el fallback genérico "Default" — usado por
        `used by `doctor`/`setup` to distinguish a real audio subsystem
        real de uno degradado, algo que `import pycaw` por sí solo no revela.
    """
    system = platform.system()

    if system == "Windows":
        try:
            from pycaw.pycaw import AudioUtilities, EDataFlow, DEVICE_STATE
            # Enumera SOLO endpoints de render (salida) activos, descartando los de
            # captura (micrófonos). GetAllDevices() no distingue el data-flow, así que
            # se usa el IMMDeviceEnumerator con eRender, análogo al filtro de Linux.
            enumerator = AudioUtilities.GetDeviceEnumerator()
            collection = enumerator.EnumAudioEndpoints(
                EDataFlow.eRender.value, DEVICE_STATE.ACTIVE.value
            )
            count = collection.GetCount()
            result = []
            for i in range(count):
                dev = AudioUtilities.CreateDevice(collection.Item(i))
                result.append({
                    "id": i,
                    "name": dev.FriendlyName,
                    "latency": getattr(dev, "Latency", 0.0),
                })
            return result, False
        except Exception:
            # No solo ImportError: un fallo COM de pycaw (sesiones RDP, hosts
            # sin audio) también debe degradar al fallback, no crashear.
            logger.debug("Enumeración de dispositivos de audio (pycaw) falló; se usa el fallback", exc_info=True)
            return [{"id": 0, "name": "Default", "latency": 0.1}], True

    elif system in ("Darwin", "Linux"):
        # sounddevice (PortAudio) enumera en ambas plataformas; se filtran los
        # dispositivos de salida, análogo al filtro eRender de Windows.
        try:
            import sounddevice as sd
            return [
                {"id": i, "name": info['name'], "latency": info['default_low_output_latency']}
                for i, info in enumerate(sd.query_devices())
                if info['max_output_channels'] > 0
            ], False
        except Exception:
            # No solo ImportError: un fallo de PortAudio en tiempo de
            # enumeración (sin backend ALSA/CoreAudio, host sin audio) también
            # debe degradar al fallback, no crashear, igual que en Windows.
            logger.debug("Enumeración de dispositivos de audio (sounddevice) falló; se usa el fallback", exc_info=True)
            return [{"id": 0, "name": "Default", "latency": 0.1}], True

    return [{"id": 0, "name": "Default", "latency": 0.1}], True


def get_audio_devices() -> list[dict]:
    """
    Lista los dispositivos de salida de audio disponibles.

    Returns:
        Lista de dicts con claves 'id', 'name', 'latency'
    """
    devices, _degraded = get_audio_devices_with_status()
    return devices
