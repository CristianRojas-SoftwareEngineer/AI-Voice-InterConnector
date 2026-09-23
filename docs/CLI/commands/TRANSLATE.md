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
ai-voice-interconnector [--daemon|--no-daemon] [--json] translate --text <TEXTO> [--from <ISO>] [--to <ISO>]
```

Definición del subcomando: `Commands::Translate` (`src/main.rs`).

| Flag | Tipo | Default | Descripción |
|---|---|---|---|
| `-t`, `--text` | string | (requerido) | Texto a traducir |
| `--from` | `es`\|`en` | `es` | Opcional. Idioma origen, alfabeto estricto del parser (`value_parser = ["es", "en"]`, `src/main.rs`) |
| `--to` | `es`\|`en` | `en` | Opcional. Idioma destino, mismo alfabeto estricto (`src/main.rs`) |
| `--daemon` | flag global | `false` | Fuerza el uso exclusivo del daemon IPC (exit 5 `daemon_unreachable` si no responde) |
| `--no-daemon` | flag global | `false` | Fuerza la ejecución local directa, sin daemon |
| `--json` | flag global | `false` | Emite JSON legible por máquina en stdout |

**Cambio deliberado sobre `es-latam`.** El CLI ya no acepta `es-latam` en
`--from`/`--to`: `clap` lo rechaza con exit 2 antes de llegar al handler. El
token solo sigue vivo en la vía IPC del daemon (`POST /translate` acepta
`es-latam` y lo normaliza a `es` vía `resolve_translation_language`,
`crates/avi-daemon/src/lib.rs`), y en la taxonomía del grupo `speech`,
que no es este comando.

Sin `--daemon`/`--no-daemon` el modo es `Auto` (`DaemonMode::Auto`,
`src/main.rs`): se hace un probe de `GET /health` con timeout de 500 ms
(`probe_health`, `src/main.rs`, vía `daemon_activo` en
`src/main.rs`) y, si responde, se delega al
daemon; si no, se ejecuta local.

---

## Flujo en dos carriles: parser primero, handler como defensa

`handle_translate` (`src/main.rs`):

**Carril 1 — parser (`clap`, antes del handler).** `--from`/`--to` declaran
`value_parser = ["es", "en"]` (`src/main.rs`): cualquier otro valor
(`fr`, `de`, `es-latam`, …) sale con exit 2 sin entrar al handler y, por
tanto, sin payload de error con `reason` — el comando nunca corre. Es el
rechazo efectivo para todo uso vía CLI.

**Carril 2 — handler y daemon (defensa en profundidad).** `handle_translate`
conserva intactas sus tres guardas para llamantes programáticos y la vía IPC,
que no pasan por `clap`:

```
handle_translate
    │
    ▼
texto vacío (trim)? ──sí──► Err InvalidInput "empty_text" (exit 2)
    │ no
    ▼
resolve_stt_language(from/to)          ← normaliza "es-latam" → "es" (vivo solo vía IPC); el resto pasa verbatim
    │
    ▼
source == target? ──sí──► passthrough: devuelve el texto intacto, sin tocar el motor (exit 0)
    │ no
    ▼
(source, target) ∈ {(es,en), (en,es)}? ──no──► Err InvalidInput "unsupported_language_pair" (exit 2)
    │ sí
    ▼
route_to_daemon(daemon_mode, client)   ← Auto: probe /health; ForceDaemon: siempre; ForceDirect: nunca
    │
    ├─ true  → translate_via_daemon: POST /translate (timeout 1500ms) al daemon
    │
    └─ false → rama local:
                is_ct2_provisioned(pair)? ──no──► Err ModelMissing "model_missing" (exit 4)
                    │ sí
                    ▼
                avi_translation::translate(text, source, target, ct2_dir)
                    │
                    ├─ Ok  → emitir {translated, source, target}
                    └─ Err → Err TranslationFailed "translation_failed" (exit 9)
```

La validación de texto vacío, el passthrough y el chequeo de par soportado son
comunes a ambas rutas (local y daemon): se resuelven antes del despacho, así
que las invariantes de contrato no dependen de si el daemon está activo.

---

## Passthrough: source == target

Cuando `from` y `to` normalizan al mismo idioma ISO, `handle_translate`
devuelve el texto de entrada intacto sin instanciar ningún motor
(`src/main.rs`). Es el único caso donde no se exige el modelo CT2
provisionado.

---

## Motor CT2 local (`avi-translation`)

Sin daemon activo (o con `--no-daemon`), la traducción corre en el propio
proceso CLI:

- `store::is_ct2_provisioned(pair)` (`crates/avi-store`) exige el derivado
  sano: `model.bin` más tokenizador (`tokenizer.json`, o
  `source.spm`+`target.spm`), el mismo gate documentado en `setup`
  (`docs/CLI/commands/SETUP.md`).
- `avi_translation::translate` (`crates/avi-translation/src/lib.rs`)
  segmenta el texto jerárquicamente con `HierarchicalSegmenter`
  (`avi-core::engine`), agrupa las oraciones de cada párrafo en lotes de a lo
  sumo `MAX_ORACIONES_POR_LOTE = 10` (`crates/avi-translation/src/lib.rs`)
  y traduce cada lote con una única llamada a `translate_batch`.
- `Ct2TranslationEngine` (`crates/avi-translation/src/lib.rs`) envuelve
  `ct2rs::Translator` sobre el modelo Marian/opus-mt convertido a CT2
  (`ComputeType::INT8`); anexa manualmente el token `</s>` al origen de cada
  oración (el encoder Marian lo exige y el conversor CT2 no lo añade) y sanea
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
`{text, from, to}` y timeout de 1500 ms; un timeout o fallo de conexión mapea
a `ExitCode::DaemonUnreachable` (`daemon_unreachable`). El endpoint
`translate_handler` (`crates/avi-daemon/src/lib.rs`) replica la misma
validación (`empty_text` → 400, `unsupported_language_pair` → 400,
`model_missing` → 404 si `is_ct2_provisioned` falla) y, si el motor CT2 del
par ya está precargado en `DaemonState::ct2_engine`, traduce con el residente;
si no, cae a carga bajo demanda con `avi_translation::translate` (misma
función que la rama local). El CLI mapea el `reason` del cuerpo de error del
daemon al mismo `ExitCode` que produciría la ruta local
(`empty_text`/`unsupported_language_pair` → `InvalidInput`, `model_missing` →
`ModelMissing`, `translation_failed` → `TranslationFailed`), preservando un
contrato de salida idéntico entre ambas rutas.

Ver `docs/CLI/commands/DAEMON.md` para el ciclo de vida del daemon y el detalle
de `DaemonState::ct2_engine`.

---

## Contrato `--json`

Éxito (traducción o passthrough), stdout:

| Clave | Tipo | Significado |
|---|---|---|
| `schema_version` | string | `"3"`, inyectada por `with_schema_version`/`emit_raw_json` (`crates/avi-core/src/json_emitter.rs`) |
| `translated` | string | Texto traducido (o el texto de entrada intacto en passthrough) |
| `source` | string | Token de `--from` tal como se pasó (no el ISO normalizado) |
| `target` | string | Token de `--to` tal como se pasó (no el ISO normalizado) |

Error, stdout (vía el manejador genérico de `main`, `src/main.rs`):

| Clave | Tipo | Significado |
|---|---|---|
| `schema_version` | string | `"3"` |
| `error` | string | Mensaje humano del error |
| `reason` | string | Código de motivo (`empty_text`, `unsupported_language_pair`, `model_missing`, `translation_failed`, `translation_unsupported`, `daemon_unreachable`, `daemon_error`) |

No hay passthrough de idiomas normalizados en la salida: `source`/`target`
reflejan literalmente `--from`/`--to`. Vía CLI solo pueden ser `es` o `en`
(el parser rechaza el resto); `es-latam` es irrepresentable por CLI y solo
aparece en `source`/`target` cuando la petición entra por la vía IPC del
daemon, que lo normaliza internamente a `es` para el enrutado y la validación
(`crates/avi-daemon/src/lib.rs`).

---

## Errores

| Reason | Exit code | Causa |
|---|---|---|
| valor fuera de alfabeto (`--from`/`--to` ∉ `{es, en}`) | 2 (rechazo del parser, sin `reason` de envelope) | `clap` rechaza el valor antes del handler (`value_parser`, `src/main.rs`); incluye `es-latam` vía CLI |
| `empty_text` | 2 (`InvalidInput`) | `--text` vacío o solo espacios |
| `unsupported_language_pair` | 2 (`InvalidInput`) | Par distinto de `es→en`/`en→es` tras normalizar; solo alcanzable vía handler local programático o vía IPC del daemon (por CLI el parser rechaza primero) |
| `model_missing` | 4 (`ModelMissing`) | Derivado CT2 no provisionado para el par (`is_ct2_provisioned` falla); ejecutar `setup` |
| `translation_failed` | 9 (`TranslationFailed`) | El motor CT2 cargó pero la inferencia falló |
| `translation_unsupported` | 1 (`Error`) | Binario compilado sin el feature `native-translation` (solo rama local) |
| `daemon_unreachable` | 5 (`DaemonUnreachable`) | `--daemon` sin daemon activo, o timeout/fallo de conexión en la ruta `Auto`/`ForceDaemon` |
| `daemon_error` | 1 (`Error`) | El daemon respondió con un cuerpo no JSON o un `reason` no reconocido |

---

## Ejemplos

```bash
ai-voice-interconnector translate --text "Hola, ¿cómo estás?" --from es --to en
ai-voice-interconnector --json translate --text "Hola" --from es --to es   # passthrough, sin motor
ai-voice-interconnector --no-daemon translate --text "Hello" --from en --to es
ai-voice-interconnector --daemon --json translate --text "Buenos días" --from es --to en
```
