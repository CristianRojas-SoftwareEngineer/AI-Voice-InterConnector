## Recorrido

La investigación examinó la implementación completa de `voice` (list, clone, remove) explorando siete fuentes principales: el parser CLI (`cli.py:2616-2646`), los tres handlers (`cmd_voice_clone` en `cli.py:839`, `cmd_voice_list` en `cli.py:991`, `cmd_voice_remove` en `cli.py:945`), los helpers de validación (`_validate_identifier` en `cli.py:332`, `_require_voice_exists` en `cli.py:346`, `_resolve_voice_paths` en `cli.py:109`), el módulo de almacenamiento (`voices.py` completo), el daemon FastAPI (`daemon/server.py:432-460`), el protocolo IPC (`daemon/protocol.py:176-189`), el cliente IPC (`daemon/ipc.py:310-343`), el engine (`engine.py:558-595`), y los códigos de salida (`exit_codes.py`). No hubo desviaciones del plan ni fuentes faltantes.

---

## Respuestas a los objetivos

**Diseño de `voice`:** Es un gestor de voces con tres operaciones atómicas (listar, clonar, eliminar) que opera sobre un registro de dos niveles (usuario escribible + fábrica solo lectura). El clonado es la operación más compleja: valida y copia audios, luego precomputa conditionals (con o sin daemon). Las otras dos operaciones son puros wrappers sobre el módulo `voices.py`, libre de modelo.

**Implementación:** Los handlers CLI delegan en `voices.py` para todas las operaciones de filesystem (resolución, copia, eliminación). El precómputo de conditionals se despacha tri-modal (daemon explícito / autodetección / directo), idéntico al patrón de `speech synthesize`. El módulo `voices.py` es deliberadamente libre de torch/modelo: ningún import de engine o torch ocurre ahí.

**Proceso de ejecución:** `voice clone` → validar modelo en caché → `voices.clone_voice_files` (validar audios con librosa, validar duración ≥10s, copiar WAVs) → `_precompute_cloned_voice` (daemon o directo) → informar resultado. `voice list` → `voices.list_voices` → imprimir o JSON. `voice remove` → `voices.remove_voice` → informar (con manejo especial para voces de fábrica y archivos en uso).

---

## Hallazgos por tema

### Definición CLI y parámetros

El parser se define en `cli.py:2616-2646`. `voice` es un subcomando de segundo nivel con tres sub-acciones:

```
ai-voice-interconnector voice list [--json]
ai-voice-interconnector voice clone --name NAME --speech-reference FILE [--timbre-reference FILE] [--force] [--daemon|--no-daemon] [--json]
ai-voice-interconnector voice remove --name NAME [--json]
```

**Parámetros de `voice clone`:**

| Parámetro | Tipo | Requerido | Descripción |
|---|---|---|---|
| `--name, -n` | str | sí | Nombre de la voz (validado contra regex `^[A-Za-z0-9._-]+$`) |
| `--speech-reference, -s` | file | sí | Audio de habla para conditioning del T3 (≥10s, se valida duración) |
| `--timbre-reference, -t` | file | no | Audio de timbre para el Voice Encoder (cualquier largo; si se omite, el habla cubre ambos) |
| `--force, -f` | flag | no | Sobrescribe si la voz ya existe (usuario o fábrica homónima) |
| `--daemon` | flag | no | Exige daemon para precómputo; exit 5 si no está activo |
| `--no-daemon` | flag | no | Fuerza precómputo directo, sin sondear daemon |
| `--json` | flag | no | Emite JSON legible por máquina |

`--daemon` y `--no-daemon` son mutuamente excluyentes (`cli.py:2632`).

**Parámetros de `voice list`:**

| Parámetro | Tipo | Descripción |
|---|---|---|
| `--json` | flag | Emite JSON legible por máquina |

**Parámetros de `voice remove`:**

| Parámetro | Tipo | Requerido | Descripción |
|---|---|---|---|
| `--name, -n` | str | sí | Nombre de la voz a eliminar |
| `--json` | flag | no | Emite JSON legible por máquina |

### Implementación de handlers

**`cmd_voice_clone`** (`cli.py:839-897`):

```
is_provisioned() (`crates/avi-store/src/lib.rs:550`)                          ← aborta si modelo no está en `hf_cache_dir()`
    │
    ▼
voices.clone_voice_files(name, timbre, speech)   ← valida + copia audios
    │                                               VoiceExistsError → exit 6
    │                                               ValueError → exit 2
    ▼
_precompute_cloned_voice(args)                   ← 3 modos de despacho
    │
    ▼
informar resultado (texto o JSON)
```

El handler captura tres excepciones específicas:
- `VoiceExistsError` → exit 6 (`EXIT_STATE_CONFLICT`), razón `voice_exists`
- `ValueError` → exit 2 (`EXIT_INVALID_INPUT`), razón `usage_error`
- `Exception` genérica → exit 1 (`EXIT_ERROR`), razón `generic`

**`cmd_voice_remove`** (`cli.py:945-988`):

Flujo con tres ramas:
1. `voices.remove_voice` devuelve `True` → eliminación exitosa
2. `voices._resolve_voice_dir` no es `None` pero `remove_voice` devuelve `False` → voz de fábrica, exit 6
3. Ambos `False`/`None` → voz no encontrada, exit 3

Manejo especial para `PermissionError`/`OSError` (`cli.py:967-982`): en Windows, `shutil.rmtree` falla si los `.wav` están abiertos por otro proceso (daemon, reproductor, Explorador, antivirus). El error sugiere cerrar el proceso bloqueante. Exit 6.

**`cmd_voice_list`** (`cli.py:991-1014`):

Simple wrapper sobre `voices.list_voices()`. Si la lista está vacía, imprime un ejemplo de uso de `voice clone`. Captura `FileNotFoundError` (directorio de voces inexistente) → exit 3.

### Almacenamiento de voces

`voices.py` implementa un modelo de dos niveles con precedencia **usuario → fábrica**:

| Nivel | Directorio | Escritura |
|---|---|---|
| Usuario | `data_root()/voices/<nombre>/` | Sí |
| Fábrica | `bundled_voices_dir()/<nombre>/` | No (solo lectura) |

**Estructura de una voz:**

```
<nombre>/
├── speech-reference.wav    ← obligatorio (conditioning T3)
├── timbre-reference.wav    ← opcional (Voice Encoder)
└── conditionals.pt         ← precómputo (generado por clone o primera síntesis)
```

**`_is_valid_voice_dir`** (`voices.py:90-111`): una voz es válida solo si tiene `speech-reference.wav` y ninguno de sus componentes es un symlink. Esta defensa anti-symlink cierra la ventana de ataque donde un enlace apunte a un `.wav` arbitrario.

**`voice_dir`** (`voices.py:66-75`): compone la ruta y ejecuta defensa en profundidad con `os.path.realpath` para garantizar que el directorio resuelto nunca escape del registro de voces.

**`_resolve_voice_dir`** (`voices.py:114-121`): resolución con precedencia usuario→fábrica. Si el mismo nombre existe en ambos niveles, gana el de usuario.

**`clone_voice_files`** (`voices.py:158-218`):

1. Valida cargabilidad de audios con `librosa.load(path, sr=24000, duration=1.0)` — audio ilegible aborta antes de tocar el filesystem
2. Valida duración del habla ≥10s con `librosa.get_duration(path=speech_reference)`
3. Colisión de nombre con `VoiceExistsError` si no se pasó `--force`
4. Crea el directorio destino con `os.makedirs`
5. Copia `timbre-reference.wav` si se proporcionó; si no, limpia uno existente de clonado previo (incondicional a `--force`)
6. Copia `speech-reference.wav`

**`remove_voice`** (`voices.py:144-155`): solo elimina voces de usuario. Operación atómica: verifica que el directorio sea una voz válida antes de `shutil.rmtree`.

**`list_voices`** (`voices.py:124-141`): iteración usuario→fábrica, alfabética dentro de cada raíz, deduplicación con `set` (precedencia usuario sobre fábrica).

### Precómputo de conditionals

**Ruta de despacho `voice clone` (3 modos, `POST /voices/clone`)** (`src/main.rs:519` `clone_via_daemon`):

```
Clone (3 modos, POST /voices/clone)
    │
    ├─ --daemon ──────────► route_to_daemon? → Sí → POST /voices/clone (timeout 1500ms) → {name,speech,precomputed:false} : exit 5
    ├─ --no-daemon ───────► Qwen3TtsEngine::new + avi_tts::clone_voice → save_reference (local)
    └─ sin flags ─────────► route_to_daemon? → Sí → POST /voices/clone : local
```

Precompute previo (`POST /voices/precompute`) se mantiene como fallback post-registro; `POST /voices/clone` (`crates/avi-daemon/src/lib.rs:669` `voices_clone_handler`) decodifica `audio_b64`, valida `VoiceStore::validate_name`, `exists`/`force`, `clone_voice` con `DEFAULT_CLONE_LANGUAGE` y `save_reference`.

**Invariante de degradación:** salvo con `--daemon` caído (exit 5), cualquier fallo del precómputo se captura y devuelve `False` (`cli.py:935-941`). La voz queda registrada y el primer `speech synthesize --voice <nombre>` computa los conditionals on-the-fly.

**Daemon-side** (`daemon/server.py:432-460`):

- Endpoint síncrono `POST /voices/precompute` → FastAPI lo despacha al threadpool
- Corre bajo `_synthesis_lock` para serializar con síntesis en vuelo (forward passes sobre `tts.ve/s3gen/t3`)
- Lee audios desde el registro vía `voices.voice_paths` — nunca recibe rutas del cliente
- Error 404 si la voz no existe, 500 si falla el precómputo

**Protocolo IPC** (`daemon/protocol.py:176-189`):

```python
class PrecomputeVoiceRequest(ProtocolModel):
    name: str  # Field(min_length=1, max_length=255)

class PrecomputeVoiceResponse(ProtocolModel):
    name: str
    precomputed: bool
```

**Cliente IPC** (`daemon/ipc.py:310-343`): envía `POST /voices/precompute` con `json={"name": name}`, timeout `REQUEST_TIMEOUT`. Valida respuesta contra `PrecomputeVoiceResponse`. Eleva `DaemonIPCError` en caso de HTTP error, timeout, o cuerpo no conforme.

### Validación

**`_validate_identifier`** (`cli.py:332-343`): wrapper que delega en `voices._validate_path_segment` y convierte `ValueError` en `CliError` exit 2. Se usa para validar nombres de voz en comandos `speech`.

**`_validate_path_segment`** (`voices.py:37-53`): validación robusta de nombre de voz:
- Regex `^[A-Za-z0-9._-]+$` (`voices.py:25`)
- Rechaza vacíos, `..`, `.`, separadores de ruta, rutas absolutas
- Normaliza a minúsculas (previene colisiones en APFS/NTFS)
- Parametrizable con `kind` para mensajes de error específicos

**`_require_voice_exists`** (`cli.py:346-358`): verifica que la voz exista (usuario o fábrica); exit 3 si no. Se aplica en seis sub-acciones de `speech` (`synthesize`, `say`, `dub`, `play`, `remove` y `list` condicionalmente con `--voice`) para que «voz mal escrita» nunca se disfrace de «sin resultados».

**`_resolve_voice_paths`** (`cli.py:109-123`): resuelve nombre de voz a rutas absolutas de audio. Resuelve contra CWD del cliente antes de que crucen la frontera hacia el daemon. Se usa en los handlers de `speech`.

### Manejo de errores

| Excepción / Condición | Subcomando | Código exit | Razón |
|---|---|---|---|
| Modelo no en caché | clone | 4 | `model_missing` (vía `is_provisioned` `hf_cache_dir`) |
| Audio no cargable (librosa) | clone | 2 | `usage_error` |
| Habla < 10s | clone | 2 | `usage_error` |
| Voz ya existe (sin `--force`) | clone | 6 | `voice_exists` |
| `--daemon` y daemon caído | clone | 5 | `daemon_unreachable` |
| Precómputo falla (sin `--daemon`) | clone | — | aviso stderr, no aborta |
| Voz es de fábrica | remove | 6 | `factory_voice` |
| Voz no encontrada | remove | 3 | `voice_not_found` |
| Archivo en uso (Windows) | remove | 6 | `voice_remove_io_error` |
| Nombre ilegal | remove | 2 | `invalid_voice_name` |
| Error genérico | remove | 1 | `voice_remove_error` |
| Directorio voces inexistente | list | 3 | `not_found` |
| Error genérico | list | 1 | `generic` |

### Salida JSON

Los tres subcomandos soportan `--json` para salida legible por máquina:

- `voice clone --json`: `{"name": "...", "timbre": "..."|null, "speech": "...", "precomputed": true|false}`
- `voice list --json`: `{"voices": ["voz1", "voz2", ...]}`
- `voice remove --json`: `{"name": "...", "removed": true}`

---

## Conclusiones

`voice` es un gestor de registro de voces con separación clara de responsabilidades: `voices.py` maneja filesystem (libre de modelo), los handlers CLI orquestan validación y precómputo, y el daemon provee precómputo con modelo caliente. Las decisiones de diseño más notables son:

1. **Precómputo degradable:** el fallo del precómputo nunca aborta el clonado — la voz queda registrada y la primera síntesis computa on-the-fly. Solo `--daemon` explícito con daemon caído produce exit 5.

2. **Defensa anti-symlink en profundidad:** tanto `voice_dir` (realpath contra escape) como `_is_valid_voice_dir` (rechazo de symlinks en componente o `.wav`) cierran la ventana de ataque donde un enlace simbólico dentro del registro pudiera cargar un `.wav` arbitrario.

3. **Timbre fantasma:** cuando se clona sin `--timbre-reference`, se elimina activamente un `timbre-reference.wav` existente de clonado previo (`voices.py:213-216`), evitando que un audio quede huérfano y se use por error.

4. **Normalización a minúsculas:** los nombres de voz se normalizan a minúsculas en `_validate_path_segment` (`voices.py:53`), previniendo colisiones en filesystems case-insensitive (macOS APFS, Docker volumes sobre NTFS).

5. **Validación temprana con librosa:** `clone_voice_files` carga parcialmente los audios (`duration=1.0`) antes de copiarlos, asegurando que un audio corrupto no deje una voz rota en el registro (`voices.py:186-189`).

6. **Precómputo serializado:** en el daemon, `_synthesis_lock` comparte exclusión mutua entre precómputo y síntesis, ya que ambos ejecutan forward passes sobre los mismos submodelos (`server.py:450`).
