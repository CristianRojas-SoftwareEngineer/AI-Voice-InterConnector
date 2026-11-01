# `devices`

Enumera los dispositivos de salida de audio del sistema e informa de la
entrada y la salida predeterminadas, que son las que usan la captura de
micrófono y la reproducción. Es un comando de inspección de mínima
complejidad: sin subcomandos, sin argumentos posicionales, no modifica estado
y no depende del daemon.

Implementación: `handle_devices` (`src/main.rs`), que delega el informe a
`avi_audio::get_devices_json` (`crates/avi-audio/src/lib.rs`), apoyado en
`AudioService::list_output_devices`, `AudioService::default_input` y
`AudioService::default_output` (`crates/avi-audio/src/lib.rs`).

---

## Superficie CLI

```
ai-voice-interconnector devices [--json]
```

| Flag | Tipo | Default | Descripción |
|---|---|---|---|
| `--json` | flag global | `false` | Emite JSON legible por máquina en stdout |

Definición del subcomando: `enum Commands::Devices` (`src/main.rs`),
despachado en `main` (`src/main.rs`) con `Some(Commands::Devices) => handle_devices(json_mode)`.

---

## Backend de enumeración

La enumeración usa un único backend multiplataforma: **`cpal`** (sin
ramificación por sistema operativo).

`AudioService::list_output_devices` (`crates/avi-audio/src/lib.rs`):

1. Obtiene el host por defecto de `cpal` (`cpal::default_host()`,
   `crates/avi-audio/src/lib.rs`).
2. Itera `host.output_devices()`; si la llamada falla, `list_output_devices`
   devuelve `Err` (no hay dispositivo "Default" sintético ni lista vacía en su
   lugar). Un host sin dispositivos devuelve una lista vacía, que no es un error.
3. Para cada dispositivo, el nombre es el de su descripción
   (`DeviceDescription::name`) y es `None` si el backend no puede describirlo.
   Difiere por plataforma: en Windows (WASAPI) es el nombre descriptivo del
   sistema (`FriendlyName`); en Linux (ALSA) la enumeración sigue el criterio de
   `aplay -L` y puede listar más entradas; en macOS enumerar salidas no abre la
   entrada de audio (micrófono), así que no pide permiso ni se bloquea sin sesión
   de usuario.
4. La latencia se **estima**, no se lee de un campo nativo del sistema: se
   toma `buffer_size().min` de `default_output_config()` y se calcula
   `(buffer_size / sample_rate) * 1000.0` para obtener milisegundos. Es `None`
   si `default_output_config()` falla, si el backend reporta
   `SupportedBufferSize::Unknown` o si la frecuencia es 0.

`AudioService::default_input` y `AudioService::default_output` resuelven el
dispositivo predeterminado con la misma función que usan la captura y la
reproducción, así que lo que informa `devices` es exactamente lo que esas
operaciones encontrarían. Devuelven `None` si el sistema no tiene ese
dispositivo y un error solo si falta el backend de audio. Consultar la entrada
predeterminada lee una propiedad del sistema y no abre el micrófono.

En Linux, `cpal` usa ALSA (`libasound.so.2`), que se carga en tiempo de
ejecución: si falta, el comando falla con `audio_backend_unavailable`.

Ningún valor se inventa: lo que el backend no expone queda como `null` en
`--json` y se muestra como «desconocido» en texto.

---

## Flujo del handler

```
handle_devices(json_mode)
    │
    ▼
avi_audio::get_devices_json()          ← crates/avi-audio/src/lib.rs
    │  AudioService::new() → list_output_devices(), default_input(), default_output()
    │  mapea cada AudioDevice a {"id", "name", "latency": latency_ms / 1000.0},
    │  con "name" y "latency" a null cuando el backend no los expone, y cada
    │  predeterminado a {"name"} o a null si el sistema no lo tiene
    ▼
Err → audio_error: sin backend de audio, CliError(NotFound, "audio_backend_unavailable");
      cualquier otro fallo, CliError(Error, "audio_enumeration_failed")
Ok  → --json: emit_raw_json({"devices", "default_input", "default_output"})
      sin --json: imprime línea por dispositivo y las dos predeterminadas
```

Fuente: `handle_devices` (`src/main.rs`).

---

## Contrato `--json`

Con `--json`, `handle_devices` emite con `emit_raw_json` el objeto que devuelve
`get_devices_json` (`src/main.rs`). `emit_raw_json`
(`crates/avi-core/src/json_emitter.rs`) inyecta `schema_version` sobre el
objeto antes de serializar (`with_schema_version`,
`crates/avi-core/src/json_emitter.rs`).

```json
{
  "default_input": { "name": "Micrófono (Realtek Audio)" },
  "default_output": { "name": "Altavoces (Realtek Audio)" },
  "devices": [
    { "id": 0, "name": "Altavoces (Realtek Audio)", "latency": 0.0116 },
    { "id": 1, "name": "Auriculares (USB)", "latency": 0.0102 }
  ],
  "schema_version": "6"
}
```

| Clave | Tipo | Significado |
|---|---|---|
| `default_input` | objeto o `null` | Entrada predeterminada (micrófono) que usaría la captura; `null` si el sistema no tiene ninguna |
| `default_input.name` | string o `null` | Nombre de la entrada predeterminada; `null` si el backend no lo expone |
| `default_output` | objeto o `null` | Salida predeterminada que usaría la reproducción; `null` si el sistema no tiene ninguna |
| `default_output.name` | string o `null` | Nombre de la salida predeterminada; `null` si el backend no lo expone |
| `devices` | array de objetos | Lista de dispositivos de salida enumerados por `cpal` |
| `devices[].id` | integer | Índice secuencial 0-based asignado durante la iteración de `host.output_devices()` (`crates/avi-audio/src/lib.rs`) |
| `devices[].name` | string o `null` | Nombre de la descripción del dispositivo (`DeviceDescription::name`); `null` si el backend no lo expone |
| `devices[].latency` | number o `null` | Latencia estimada **en segundos** (`latency_ms / 1000.0`, `crates/avi-audio/src/lib.rs`); `null` si no se puede estimar. En salida texto se reconvierte a milisegundos para mostrarse |
| `schema_version` | string | Inyectado por `emit_raw_json`/`with_schema_version` (ver [las dos versiones de esquema](../CONTRACT.md#las-dos-versiones-de-esquema)); no forma parte del payload que construye el handler |

Nota de orden de claves: `with_schema_version` inserta `schema_version` en el
mapa ya construido y las claves se serializan en orden alfabético, por lo que
`schema_version` aparece **después** de `devices`. Es un detalle de
serialización, no de contrato: el conjunto de claves es el mismo con
independencia del orden.

---

## Formato de salida texto

Sin `--json` (`src/main.rs`):

```
Dispositivos de salida de audio:
  [0] Altavoces (Realtek Audio) (latencia: 11.6ms)
  [1] Auriculares (USB) (latencia: 10.2ms)
Entrada predeterminada: Micrófono (Realtek Audio)
Salida predeterminada: Altavoces (Realtek Audio)
```

Cada línea de dispositivo sigue el patrón `[id] name (latencia: X.Xms)`, con la
latencia reconvertida de segundos a milisegundos (`* 1000.0`) solo para esta
vista. Un nombre o una latencia `null` se muestran como `desconocido`:

```
  [2] desconocido (latencia: desconocido)
```

Las dos últimas líneas muestran la entrada y la salida predeterminadas: su
nombre, `desconocido` si el backend no lo expone o `ninguna` si el sistema no
tiene ese dispositivo.

---

## Errores

| Reason | Código | Causa |
|---|---|---|
| `audio_backend_unavailable` | 3 (`ExitCode::NotFound`) | El sistema no tiene backend de audio: en Linux falta ALSA (`libasound.so.2`) o no define alguna función que se usa. Es un recurso ausente del sistema, no un fallo del producto |
| `audio_enumeration_failed` | 1 (`ExitCode::Error`) | El host `cpal` no pudo enumerar los dispositivos de salida. La ausencia de dispositivos por sí sola NO es un error: produce lista vacía y predeterminados `null` |

El error se envuelve en `main` (`src/main.rs`): con `--json` emite
`{"error": <mensaje>, "reason": <reason>, "schema_version": "6"}` a stdout;
sin `--json`, `Error: <mensaje>` a stderr. El proceso termina con el código
de la tabla.

`devices` no depende del daemon, no requiere modelos provisionados y no
verifica prerrequisitos: es completamente offline y autónomo.

---

## Ejemplos

```bash
ai-voice-interconnector devices          # lista en texto plano
ai-voice-interconnector --json devices   # payload legible por máquina
```
