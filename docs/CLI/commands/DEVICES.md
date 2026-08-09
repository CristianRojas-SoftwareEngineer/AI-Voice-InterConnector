## Recorrido

La investigación examinó la implementación completa de `devices` explorando tres fuentes principales: el parser CLI (`cli.py:2648-2651`), el handler `cmd_devices` (`cli.py:1017-1033`), y las funciones de enumeración de dispositivos en `audio.py:184-250`. Se leyeron en paralelo el handler, las funciones `get_audio_devices` y `get_audio_devices_with_status`, el módulo de códigos de salida (`exit_codes.py`), y el helper `emit_json` (`cli.py:69-81`). No hubo desviaciones del plan ni fuentes faltantes.

---

## Respuestas a los objetivos

**Diseño de `devices`:** Es un comando de inspección sin subcomandos que enumera los dispositivos de salida de audio del sistema. Su único parámetro es `--json` para salida estructurada. No modifica estado ni requiere argumentos posicionales.

**Implementación:** El handler `cmd_devices` (`cli.py:1017`) delega la enumeración a `get_audio_devices()` (`audio.py:242`), que a su vez invoca `get_audio_devices_with_status()` (`audio.py:184`). Esta última implementa estrategias por plataforma: `pycaw` en Windows, `sounddevice` (PortAudio) en macOS/Linux, con fallback a un dispositivo genérico "Default" si la enumeración falla.

**Proceso de ejecución:** Llamada a `get_audio_devices()` → selección de formato de salida (texto plano o JSON) → impresión a stdout. Errores de enumeración producen exit 1 con razón `generic`.

---

## Hallazgos por tema

### Definición CLI

El parser se define en `cli.py:2648-2651`:

```python
devices_parser = subparsers.add_parser("devices", help="Lista los dispositivos de audio")
devices_parser.add_argument("--json", action="store_true", help="Emitir JSON legible por máquina")
devices_parser.set_defaults(func=cmd_devices)
```

| Parámetro | Tipo | Requerido | Descripción |
|---|---|---|---|
| `--json` | flag (bool) | No | Emite salida en formato JSON con `schema_version` inyectado |

No existen subcomandos ni argumentos posicionales. Es el comando más simple del CLI en cuanto a interfaz.

### Handler: cmd_devices

`cmd_devices` (`cli.py:1017-1033`):

```python
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
```

Puntos clave:
- Importa `get_audio_devices` de forma diferida (dentro de la función), evitando carga innecesaria de `pycaw`/`sounddevice` si el comando no se ejecuta.
- Captura toda excepción y la envuelve en `CliError(EXIT_ERROR, "generic", ...)`.
- El flag `--json` se lee con `getattr(args, "json", False)` por robustez.

### Descubrimiento de dispositivos de audio

`get_audio_devices_with_status()` (`audio.py:184-239`) implementa la enumeración real. Devuelve una tupla `(list[dict], bool)` donde el segundo elemento indica si la enumeración fue degradada (fallback).

#### Windows (`audio.py:196-220`)

Usa `pycaw.pycaw` con la API COM:
1. `AudioUtilities.GetDeviceEnumerator()` obtiene el enumerador de dispositivos.
2. `enumerator.EnumAudioEndpoints(EDataFlow.eRender.value, DEVICE_STATE.ACTIVE.value)` filtra solo endpoints de render (salida) activos, descartando micrófonos.
3. Itera la colección creando dispositivos con `AudioUtilities.CreateDevice()`.
4. Extrae `FriendlyName` y `Latency` de cada dispositivo.

#### macOS / Linux (`audio.py:222-237`)

Usa `sounddevice` (wrapper de PortAudio):
1. `sd.query_devices()` lista todos los dispositivos del sistema.
2. Filtra por `max_output_channels > 0` (solo dispositivos de salida).
3. Extrae `name` y `default_low_output_latency` de cada dispositivo.

#### Fallback degradado

Si la enumeración falla en cualquier plataforma (no solo `ImportError`, también errores COM, fallos de PortAudio, etc.), se devuelve:

```python
[{"id": 0, "name": "Default", "latency": 0.1}]
```

con `degraded=True`. Esto permite que `doctor`/`setup` distingan un subsistema de audio real de uno degradado. `cmd_devices` no usa el flag `degraded` — lo descarta en `get_audio_devices()` (`audio.py:249`).

#### Wrapper simplificado

`get_audio_devices()` (`audio.py:242-250`) es un wrapper que descarta el flag `degraded` y devuelve solo la lista de dispositivos:

```python
def get_audio_devices() -> list[dict]:
    devices, _degraded = get_audio_devices_with_status()
    return devices
```

### Formato de contrato JSON

Cuando se usa `--json`, `cmd_devices` llama a `emit_json({"devices": devices})` (`cli.py:1027`). `emit_json` (`cli.py:69-81`) inyecta `schema_version` y serializa a stdout:

```json
{
  "schema_version": "3",
  "devices": [
    {"id": 0, "name": "Speakers (Realtek Audio)", "latency": 0.015},
    {"id": 1, "name": "Headphones (USB)", "latency": 0.022}
  ]
}
```

Cada elemento del array `devices` tiene esta estructura:

| Campo | Tipo | Descripción | Fuente |
|---|---|---|---|
| `id` | int | Índice secuencial (0-based) asignado durante la enumeración | `audio.py:211,228` |
| `name` | str | Nombre amigable del dispositivo (`FriendlyName` en Windows, `name` en PortAudio) | `audio.py:212,228` |
| `latency` | float | Latencia en segundos (`Latency` en Windows, `default_low_output_latency` en PortAudio) | `audio.py:213,228` |

**Nota sobre latencia:** El valor se almacena en segundos. En formato texto se imprime convertido a milisegundos (`*1000:.1f`), pero en JSON se conserva en segundos.

### Formato de salida texto

Sin `--json`, la salida es (`cli.py:1030-1032`):

```
Dispositivos de salida de audio:
  [0] Speakers (Realtek Audio) (latency: 15.0ms)
  [1] Headphones (USB) (latency: 22.0ms)
```

Cada línea muestra: `[id] name (latency: X.Xms)`.

### Manejo de errores

| Excepción | Código exit | Razón | Mensaje |
|---|---|---|---|
| Fallo de enumeración (cualquier Exception) | 1 (`EXIT_ERROR`) | `generic` | `"Error al enumerar los dispositivos de audio: {e}"` |

El comando no tiene otros caminos de error: no valida argumentos, no verifica prerequisitos, y no interactúa con el daemon. La única causa de fallo es una excepción en `get_audio_devices()`.

---

## Conclusiones

`devices` es un comando de inspección de mínima complejidad que cumple una única responsabilidad: listar dispositivos de audio de salida. Su diseño se distingue por: (1) delegación completa de la lógica de enumeración a `audio.py`, manteniendo el handler CLI trivial; (2) estrategia multiplataforma con fallback degradado — Windows usa la API COM vía `pycaw`, macOS/Linux usa PortAudio vía `sounddevice`, y ambos degradan gracefulmente a un dispositivo "Default" genérico; (3) contrato JSON consistente con `schema_version` inyectado centralmente por `emit_json`; y (4) ausencia total de dependencias del daemon o de otros subsistemas — es un comando completamente offline y autónomo.
