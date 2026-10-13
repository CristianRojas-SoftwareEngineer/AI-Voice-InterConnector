# `translate`

Traducción de texto es↔en. Comando standalone texto→texto: no toca audio ni el
motor TTS. Es delegable al daemon (CT2 residente) en 3 modos, o ejecuta el
motor CT2 local si el daemon no está activo o `--no-daemon` lo fuerza.

Implementación: `handle_translate` (`src/main.rs`), despacho al daemon vía
`translate_via_daemon` (`src/main.rs`) y `route_to_daemon`
(`src/main.rs`), motor CT2 local en `avi-translation`
(`crates/avi-translation/src/lib.rs`), endpoint del daemon
`translate_handler` (`crates/avi-daemon/src/lib.rs`).

---

## Superficie CLI

```
ai-voice-interconnector [--json] translate --text <TEXTO> [--source-language <es|en>] [--target-language <es|en>] [--daemon|--no-daemon]
```

Definición del subcomando: `Commands::Translate` (`src/main.rs`).

| Flag | Tipo | Default | Descripción |
|---|---|---|---|
| `-t`, `--text` | string | (requerido) | Texto a traducir |
| `--source-language` | `es`\|`en` | `es` | Opcional. Idioma origen, alfabeto estricto del parser (`value_parser = ["es", "en"]`, `src/main.rs`) |
| `--target-language` | `es`\|`en` | `en` | Opcional. Idioma destino, mismo alfabeto estricto (`src/main.rs`) |
| `--daemon` | flag | `false` | Fuerza el uso exclusivo del daemon IPC (exit 5 `daemon_unreachable` si no responde); excluyente con `--no-daemon` |
| `--no-daemon` | flag | `false` | Fuerza la ejecución local directa, sin daemon |
| `--json` | flag global | `false` | Emite JSON legible por máquina en stdout |

El vocabulario de idioma de la CLI es `es|en` en todos los comandos: `clap` rechaza cualquier otro valor con exit 2 antes de llegar al handler. El daemon acepta las mismas claves `source_language`/`target_language` con los mismos valores. `translate` no tiene límite de longitud.

Sin `--daemon`/`--no-daemon` el modo es `Auto` (`DaemonMode::Auto`,
`src/main.rs`): se hace un probe de `GET /health` con el timeout de `probe_health`
(`probe_health`, `src/main.rs`, vía `daemon_active` en
`src/main.rs`) y, si responde, se delega al
daemon; si no, se ejecuta local.

---

## Flujo en dos carriles: parser primero, handler como defensa

`handle_translate` (`src/main.rs`):

**Carril 1 — parser (`clap`, antes del handler).** `--source-language`/`--target-language` declaran
`value_parser = ["es", "en"]` (`src/main.rs`): cualquier otro valor
sale con exit 2 sin entrar al handler —el comando nunca corre—: sin `--json`,
mensaje de `clap` por stderr; con `--json`, envelope `usage_error` (ver
[el mecanismo](../CONTRACT.md#el-mecanismo-un-solo-punto-de-traducción)). Es el
rechazo efectivo para todo uso vía CLI.

**Carril 2 — handler y daemon (defensa en profundidad).** `handle_translate`
conserva intactas sus guardas, que también cubren la vía IPC,
que no pasa por `clap`:

```
handle_translate
    │
    ▼
texto vacío (trim)? ──sí──► Err InvalidInput "empty_text" (exit 2)
    │ no
    ▼
source == target? ──sí──► passthrough: devuelve el texto intacto, sin tocar el motor (exit 0)
    │ no
    ▼
route_to_daemon(daemon_mode, client)   ← Auto: probe /health; ForceDaemon: siempre; ForceDirect: nunca
    │
    ├─ true  → translate_via_daemon: POST /translate (con timeout acotado) al daemon
    │
    └─ false → rama local:
                is_provisioned(modelo del par)? ──no──► Err ModelMissing "model_missing" (exit 4)
                    │ sí
                    ▼
                avi_translation::translate(text, model_dir)
                    │
                    ├─ Ok  → emitir {translated, source_language, target_language}
                    └─ Err → Err TranslationFailed "translation_failed" (exit 9)
```

La validación de texto vacío, el passthrough y la elección del modelo por par son
comunes a ambas rutas (local y daemon): se resuelven antes del despacho, así
que las invariantes de contrato no dependen de si el daemon está activo. Los traits de los motores no reciben el idioma: la dirección la fija el modelo elegido desde el par validado en el borde.

---

## Passthrough: source == target

Cuando origen y destino coinciden, `handle_translate`
devuelve el texto de entrada intacto sin instanciar ningún motor
(`src/main.rs`). Es el único caso donde no se exige el modelo de traducción
provisionado.

---

## Motor CT2 local (`avi-translation`)

Sin daemon activo (o con `--no-daemon`), la traducción corre en el propio
proceso CLI:

- `ModelStore::is_provisioned("opus-mt-<par>")` (`crates/avi-store`) exige
  que cada fichero de `MODEL_FILE_PATTERNS` del snapshot esté presente y pese
  más de 0 bytes (`config.json`, `model.bin`, `shared_vocabulary.json`,
  `source.spm`, `target.spm`). El motor lee el modelo directo del snapshot
  (`ModelStore::new().model_dir("opus-mt-<par>")`).
- `avi_translation::translate` (`crates/avi-translation/src/lib.rs`)
  segmenta el texto jerárquicamente con `HierarchicalSegmenter`
  (`avi-core::engine`), agrupa las oraciones de cada párrafo en lotes de a lo
  sumo `MAX_SENTENCES_PER_BATCH = 10` (`crates/avi-translation/src/lib.rs`)
  y traduce cada lote con una única llamada a `translate_batch`.
- `Ct2TranslationEngine` (`crates/avi-translation/src/lib.rs`) envuelve
  `ct2rs::Translator` sobre el modelo opus-mt publicado ya convertido a CT2 int8
  (`ComputeType::INT8`); anexa manualmente el token `</s>` al origen de cada
  oración (el encoder Marian lo exige y el modelo publicado no lo añade) y sanea
  la hipótesis del decoder quitando el `</s>` final.
- El reensamblado une las oraciones de cada párrafo con espacio y los párrafos
  entre sí con `"\n\n"`, preservando la separación de la entrada
  (`crates/avi-translation/src/lib.rs`).

Sin el feature de compilación `native-translation`, la rama local devuelve
`Err(ExitCode::Error, "translation_unsupported", …)` sin intentar cargar
ningún modelo (`src/main.rs`).

---

## Despacho al daemon

`translate_via_daemon` (`src/main.rs`) hace `POST /translate` con
`{text, source_language, target_language}` y el timeout de inactividad del cliente; un timeout o fallo de conexión mapea
a `ExitCode::DaemonUnreachable` (`daemon_unreachable`). El endpoint
`translate_handler` (`crates/avi-daemon/src/lib.rs`) replica la misma
validación (`empty_text` → 400,
`model_missing` → 404 si `is_provisioned` falla) y, si el motor CT2 del
par ya está precargado en `DaemonState::ct2_engine`, traduce con el residente;
si no, cae a carga bajo demanda con `avi_translation::translate` (misma
función que la rama local). El CLI mapea el `reason` del cuerpo de error del
daemon al mismo `ExitCode` que produciría la ruta local
(`empty_text` → `InvalidInput`, `model_missing` →
`ModelMissing`, `translation_failed` → `TranslationFailed`), preservando un
contrato de salida idéntico entre ambas rutas.

Ver `docs/CLI/commands/DAEMON.md` para el ciclo de vida del daemon y el detalle
de `DaemonState::ct2_engine`.

---

## Contrato `--json`

Éxito (traducción o passthrough), stdout:

| Clave | Tipo | Significado |
|---|---|---|
| `schema_version` | string | Inyectada por `with_schema_version`/`emit_raw_json` (`crates/avi-core/src/json_emitter.rs`); ver [las dos versiones de esquema](../CONTRACT.md#las-dos-versiones-de-esquema) |
| `translated` | string | Texto traducido (o el texto de entrada intacto en passthrough) |
| `source_language` | string | Valor de `--source-language` tal como se pasó |
| `target_language` | string | Valor de `--target-language` tal como se pasó |

Error, stdout (vía el manejador genérico de `main`, `src/main.rs`):

| Clave | Tipo | Significado |
|---|---|---|
| `schema_version` | string | Ver [las dos versiones de esquema](../CONTRACT.md#las-dos-versiones-de-esquema) |
| `error` | string | Mensaje humano del error |
| `reason` | string | Código de motivo (`empty_text`, `model_missing`, `translation_failed`, `translation_unsupported`, `daemon_unreachable`, `daemon_error`) |

`source_language`/`target_language` reflejan los flags tal como se pasaron.
Vía CLI solo pueden ser `es` o `en` (el parser rechaza el resto).

---

## Errores

| Reason | Exit code | Causa |
|---|---|---|
| valor fuera de alfabeto (`--source-language`/`--target-language` ∉ `{es, en}`) | 2 (rechazo del parser: sin `--json`, mensaje de `clap` por stderr; con `--json`, envelope [`usage_error`](../CONTRACT.md#el-mecanismo-un-solo-punto-de-traducción)) | `clap` rechaza el valor antes del handler (`value_parser`, `src/main.rs`) |
| `empty_text` | 2 (`InvalidInput`) | `--text` vacío o solo espacios |
| `model_missing` | 4 (`ModelMissing`) | Modelo de traducción no provisionado para el par (`is_provisioned` falla); ejecutar `setup` |
| `translation_failed` | 9 (`TranslationFailed`) | El motor CT2 cargó pero la inferencia falló |
| `translation_unsupported` | 1 (`Error`) | Binario compilado sin el feature `native-translation` (solo rama local) |
| `daemon_unreachable` | 5 (`DaemonUnreachable`) | `--daemon` sin daemon activo, o timeout/fallo de conexión en la ruta `Auto`/`ForceDaemon` |
| `daemon_error` | 1 (`Error`) | El daemon respondió con un cuerpo no JSON o un `reason` no reconocido |
| `lifecycle_locked` | 17 (`LifecycleLocked`) | Hay una operación de ciclo de vida en curso; el comando no llega a ejecutarse |

---

## Ejemplos

```bash
ai-voice-interconnector translate --text "Hola, ¿cómo estás?" --source-language es --target-language en
ai-voice-interconnector --json translate --text "Hola" --source-language es --target-language es   # passthrough, sin motor
ai-voice-interconnector translate --no-daemon --text "Hello" --source-language en --target-language es
ai-voice-interconnector --json translate --daemon --text "Buenos días" --source-language es --target-language en
```
