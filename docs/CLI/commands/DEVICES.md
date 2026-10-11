# `devices`

Enumera los dispositivos de salida de audio del sistema. Es un comando de
inspección de mínima complejidad: sin subcomandos, sin argumentos
posicionales, no modifica estado y no depende del daemon.

Implementación: `handle_devices` (`src/main.rs`), que delega la
enumeración real a `avi_audio::get_devices_json` (`crates/avi-audio/src/lib.rs`),
apoyado en `AudioService::list_output_devices` (`crates/avi-audio/src/lib.rs`).

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

Ningún valor se inventa: lo que el backend no expone queda como `null` en
`--json` y se muestra como «desconocido» en texto.

---

## Flujo del handler

```
handle_devices(json_mode)
    │
    ▼
avi_audio::get_devices_json()          ← crates/avi-audio/src/lib.rs
    │  AudioService::new() → list_output_devices()
    │  mapea cada AudioDevice a {"id", "name", "latency": latency_ms / 1000.0},
    │  con "name" y "latency" a null cuando el backend no los expone
    ▼
Err → CliError(ExitCode::Error, "audio_enumeration_failed", e.to_string())
Ok  → --json: emit_raw_json({"devices": devices})
      sin --json: imprime línea por dispositivo
```

Fuente: `handle_devices` (`src/main.rs`).

---

## Contrato `--json`

Con `--json`, `handle_devices` llama a `emit_raw_json(json!({ "devices": devices }))`
(`src/main.rs`). `emit_raw_json` (`crates/avi-core/src/json_emitter.rs`)
inyecta `schema_version` sobre el objeto antes de serializar
(`with_schema_version`, `crates/avi-core/src/json_emitter.rs`).

```json
{
  "devices": [
    { "id": 0, "name": "Altavoces (Realtek Audio)", "latency": 0.0116 },
    { "id": 1, "name": "Auriculares (USB)", "latency": 0.0102 }
  ],
  "schema_version": "5"
}
```

| Clave | Tipo | Significado |
|---|---|---|
| `devices` | array de objetos | Lista de dispositivos de salida enumerados por `cpal` |
| `devices[].id` | integer | Índice secuencial 0-based asignado durante la iteración de `host.output_devices()` (`crates/avi-audio/src/lib.rs`) |
| `devices[].name` | string o `null` | Nombre de la descripción del dispositivo (`DeviceDescription::name`); `null` si el backend no lo expone |
| `devices[].latency` | number o `null` | Latencia estimada **en segundos** (`latency_ms / 1000.0`, `crates/avi-audio/src/lib.rs`); `null` si no se puede estimar. En salida texto se reconvierte a milisegundos para mostrarse |
| `schema_version` | string | `"5"`, inyectado por `emit_raw_json`/`with_schema_version` — no forma parte del payload que construye el handler |

Nota de orden de claves: `with_schema_version` inserta `schema_version` en el
mapa ya construido, por lo que en la salida serializada aparece **después**
de `devices`. Es un detalle de serialización, no de contrato: el conjunto de
claves es el mismo con independencia del orden.

---

## Formato de salida texto

Sin `--json` (`src/main.rs`):

```
Dispositivos de salida de audio:
  [0] Altavoces (Realtek Audio) (latencia: 11.6ms)
  [1] Auriculares (USB) (latencia: 10.2ms)
```

Cada línea sigue el patrón `[id] name (latencia: X.Xms)`, con la latencia
reconvertida de segundos a milisegundos (`* 1000.0`) solo para esta vista. Un
nombre o una latencia `null` se muestran como `desconocido`:

```
  [2] desconocido (latencia: desconocido)
```

---

## Errores

| Reason | Código | Causa |
|---|---|---|
| `audio_enumeration_failed` | 1 (`ExitCode::Error`) | `AudioService::list_output_devices` devolvió `Err` (el host `cpal` no pudo enumerar los dispositivos de salida; la ausencia de dispositivos por sí sola NO es un error, produce lista vacía) |

El error se envuelve en `main` (`src/main.rs`): con `--json` emite
`{"error": <mensaje>, "reason": "audio_enumeration_failed", "schema_version": "5"}`
a stdout; sin `--json`, `Error: <mensaje>` a stderr. En ambos casos el
proceso termina con exit code 1.

`devices` no depende del daemon, no requiere modelos provisionados y no
verifica prerrequisitos: es completamente offline y autónomo.

---

## Ejemplos

```bash
ai-voice-interconnector devices          # lista en texto plano
ai-voice-interconnector --json devices   # payload legible por máquina
```
