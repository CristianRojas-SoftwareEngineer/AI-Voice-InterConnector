# Revisión: auditoría de paridad de superficie CLI oráculo Python → Rust

- **Fecha**: 2026-09-02
- **Estado**: Cerrado (diagnóstico) — actualizado 2026-09-04: **P7 corregido** en `src/main.rs:942` (guarda `audio.is_none() && !mic → usage_error 2` espejo de `Transcribe`, sin `panic 101`) — ver §9; **P8 corregido** en `src/main.rs:1438`/`1848`/`1555` + `crates/avi-store/src/lib.rs:537` (`CT2` incondicional en `hf_cache_dir/ct2`, `doctor` exige `model.bin`, `cleanup` purga) — ver §10. Auditoría documental y de código; ninguna corrección fue aplicada en v0.18.26 salvo P7/P8 post-release. Este documento es el registro para planificar las correcciones y sirve de insumo a la fase 3 del orden recomendado en `docs/reviews/2026-09-02-hallazgos-e2e-windows-v0.18.26.md`.
- **Alcance**: Diff flag a flag de la superficie completa de CLI entre el oráculo Python en su estado final y el CLI Rust actual, arbitrado por `docs/CLI/CONTRACT.md`.
- **Naturaleza**: Diagnóstico post-prueba. Cada divergencia se clasifica como rotura de paridad (el oráculo la implementaba y el contrato la promete, pero el Rust no), cambio deliberado de la migración, o defecto nuevo introducido por el port.
- **Origen**: Hallazgo 5 de la revisión E2E de v0.18.26 (`speech list --voice` rechazado) resultó ser una **regresión de paridad del port Python→Rust** — no un drift documental como se clasificó originalmente — lo que motivó auditar la superficie completa en busca de roturas del mismo tipo. Un cruce posterior con los hallazgos de la E2E trasladó a este documento dos de ellos con evidencia ampliada: `speech list --voice` (P6.1, con su reproducción E2E) y `translate es→en` roto (P8, con la verificación de que el oráculo sí provisionaba la conversión CT2).

## Tabla de contenidos

- 1. Resumen
- 2. Método y fuentes
- 3. Hallazgo P1 — `cleanup` perdió todo el borrado granular y `--all` cambió de semántica
- 4. Hallazgo P2 — `speech synthesize`/`say` perdieron los flags de idioma y los overrides de síntesis
- 5. Hallazgo P3 — `speech dub` renombró `--source-language`/`--target-language` sin reflejarlo en el contrato
- 6. Hallazgo P4 — `daemon start`/`serve` perdieron todos sus flags (auto-reinicio incluido)
- 7. Hallazgo P5 — `setup` perdió los modos `--force-update`/`--remove-path`/`--yes`
- 8. Hallazgo P6 — Divergencias menores (`speech list --voice`, `translate`, `--play`)
- 9. Hallazgo P7 — Probable panic en `speech dub` sin fuente de audio (defecto nuevo, no paridad)
- 10. Hallazgo P8 — `translate es→en` roto para el usuario final: la provisión CT2 del `setup` nunca fue portada
- 11. Lo que sí migró con paridad (verificado)
- 12. Diagnóstico de fondo
- 13. Orden de corrección recomendado

## 1. Resumen

La migración Python→Rust portó fielmente las **rutas calientes** (síntesis, clonación, dispatch daemon, transcribe, play/remove) pero perdió superficie de forma sistemática en tres zonas: la **periferia de gestión** (`cleanup` granular, modos de `setup`), los **flags de idioma y cross-lingual** (`--target-language`/`--source-language`), y los **controles del daemon** (`--autorestart`, `--language`). `CONTRACT.md` —que se declara normativo— fue sincronizado técnicamente al stack Rust (Parakeet, crates) pero sus secciones §11 (`cleanup`) y §13 (cross-lingual/`dub`) describen un CLI que no existe: prometen flags que ni el binario implementa.

Además, la auditoría detectó un defecto nuevo que no es de paridad: una probable condición de **panic** en `speech dub` invocado sin `--audio` ni `--mic` (P7), hoy enmascarada por el orden de los chequeos. Y el cruce con la E2E de v0.18.26 sumó una rotura de paridad **conductual** que el diff de flags no podía ver: la provisión CT2 de `setup` existía en el oráculo y no fue portada, lo que deja `translate` es→en inutilizable para el usuario final (P8).

| # | Hallazgo | Tipo | ¿Contrato lo promete? | Gravedad |
|---|---|---|---|---|
| P1 | `cleanup` granular perdido (`--voices`/`--synthetic-speech`/`--model`/`--dry-run`/`--yes`); `cleanup` pelado borra todo; `--all` ahora desinstala | Rotura de paridad + cambio de semántica | Sí (§11: 534-543, 593) | **Alta**: superficie de gestión íntegra perdida + contradicción con contrato |
| P2 | `synthesize`/`say` sin `--target-language`/`--source-language` (cross-lingual integrado) ni overrides (`--compute-backend`/`--exaggeration`/`--cfg-weight`/`--temperature`) | Rotura de paridad (parcialmente deliberada por cambio de engine) | Sí (§13: 587-593, 619) | Media-alta: cross-lingual solo sobrevive vía `dub` |
| P3 | `dub`: `--source-language`/`--target-language` renombrados a `--from`/`--to` sin actualizar el contrato | Renombrado no documentado | Sí, con los nombres viejos (§13: 619) | Media |
| P4 | `daemon start`/`serve` son variantes unitarias: `--autorestart`/`--auto-restart`, `--max-retries`, `--language`, `--with-stt` perdidos | Rotura de paridad | Parcial (593: `daemon start --language`) | Media: auto-reinicio del daemon desaparecido |
| P5 | `setup`: `--force-update`, `--remove-path`, `--yes` perdidos; `--language` sin choices y default `all`→`es` | Rotura de paridad (parcial: `--uninstall` rediseñado a comando propio) | Sí (593: `setup --language {en, all}`) | Media: sin forma de re-descargar sin purga manual |
| P6 | Menores: `speech list --voice` (P6.1), `translate --from/--to` opcionales sin choices, `--play` sin bucle interactivo | Roturas de paridad menores | Sí (`list --voice`: 170, 278; `--play`: §4) | Baja |
| P7 | `speech dub` sin `--audio`/`--mic` alcanza `audio.expect("validado arriba")` sin guarda previa → probable panic exit 101 | **Defecto nuevo del port** (el oráculo exigía la fuente con grupo `required=True`) | El contrato dice "uno requerido" (603, 619) | Media: crash; hoy enmascarado por exit 4 temprano |
| P8 | `translate es→en` exit 4 `model_missing` con `doctor` en "ready" | Rotura de paridad **conductual**: la provisión CT2 del `setup` del oráculo no fue portada; ruta resuelta contra el CWD en vez del cache dir | Sí (593: `setup --language {en, all}` descarga y convierte a CT2) | **Alta**: funcionalidad inutilizable para usuario final |

## 2. Método y fuentes

**Oráculo Python**: estado final de `src/ai_voice_interconnector/cli.py` (antes `src/tts_sidecar/cli.py`) en el commit `7542962` (2026-08-25), padre del borrado `cd965af` ("retirar el canal Python huérfano"). Verificado que la superficie argparse es idéntica a través del rename `ca7d00c` (TTS-Sidecar → AI-Voice-InterConnector). Se inventariaron todas las llamadas `add_parser`/`add_argument`/`add_mutually_exclusive_group`/`set_defaults`, más el parser secundario de `daemon/run.py`.

**CLI Rust actual**: HEAD `443805c` (v0.18.26). Todo el parseo vive en `src/main.rs` (struct `Cli` + enums `Commands`/`VoiceCommands`/`SpeechCommands`/`DaemonCommands`, `src/main.rs:55-268`); verificado que ningún otro binario del workspace aporta superficie.

**Árbitro**: `docs/CLI/CONTRACT.md`, que se autodefine como "descripción normativa del contrato público de la CLI" y referencia el stack Rust (p. ej. `crates/avi-stt/src/parakeet.rs` en §11), por lo que sus promesas vigentes son exigibles al binario actual.

**Limitación**: la comparación es de **superficie de parseo y validación visible en código**. No audita comportamiento runtime completo de cada flag sobreviviente (p. ej. fidelidad del payload) ni el daemon HTTP interno. Esa limitación es la que dejó escapar P8 — una pérdida de comportamiento de provisión es invisible a un diff de flags — detectado solo al cruzar los hallazgos de la E2E con el código del oráculo.

## 3. Hallazgo P1 — `cleanup` perdió todo el borrado granular y `--all` cambió de semántica

### Síntoma

`cleanup` en Rust acepta un único flag: `--all` (`src/main.rs:136-139`). Los cinco modos granulares del oráculo no existen.

### Evidencia

| Superficie | Oráculo (`7542962`) | `CONTRACT.md` | Rust (HEAD) |
|---|---|---|---|
| `--synthetic-speech` | ✅ borra la raíz de habla sintética | ✅ líneas 112, 183, 534 | ❌ |
| `--voices` | ✅ voces de usuario + sus locuciones (arrastra namespaces) | ✅ líneas 183, 535, 539 | ❌ |
| `--model` | ✅ modelos en caché HF | ✅ línea 593 | ❌ |
| `--dry-run` | ✅ lista sin borrar | ✅ línea 537 | ❌ |
| `--yes` | ✅ omite confirmación | — | ❌ |
| `--all` | modelos + voces + habla sintética | "Modelo + voces + habla sintética" (536) | **delega en `handle_uninstall`** |

Triple divergencia:

1. **Borrado granular perdido**: el reparto documentado —"`speech remove` cubre el borrado individual y `cleanup --synthetic-speech` el masivo, exactamente el reparto que existe entre `voice remove` y `cleanup --voices`" (contrato, línea 183)— no tiene implementación.
2. **`cleanup` sin flags ya no es error de uso**: en el oráculo, invocar `cleanup` sin modo era un error; en Rust, `cleanup` pelado ejecuta `handle_cleanup` que borra `models/`, `speech/` y `voices/` completos más los snapshots HF (`src/main.rs:1441-1470`). Es un "borra todo" disfrazado de operación inocua.
3. **`--all` cambió de semántica**: el contrato lo define como limpieza (sin tocar binario ni PATH); Rust lo define como "desinstalación completa, alias de `uninstall`" (`src/main.rs:321-327`, help del flag). Un usuario que siga el contrato y ejecute `cleanup --all` esperando solo limpieza de datos obtiene una desinstalación.

### Corrección propuesta

Restaurar los modos granulares (`--voices`, `--synthetic-speech`, `--model`, `--dry-run`, `--yes`) con la semántica de arrastre documentada en §11, y desacoplar `--all` de `handle_uninstall` (o desdocumentar el alias si se decide que `uninstall` es la única vía de desinstalación — pero entonces el contrato §11 debe reescribirse).

**Confianza**: Alta (inventarios completos de ambos lados + lectura del dispatch y del handler).

## 4. Hallazgo P2 — `speech synthesize`/`say` perdieron los flags de idioma y los overrides de síntesis

### Síntoma

`synthesize` y `say` en Rust aceptan solo `--text`/`--voice` (+ `--label`/`--output`/`--force`/`--play` en `synthesize`, y los globales `--json`/`--daemon`/`--no-daemon`). El oráculo ofrecía además: `--target-language` (default `es-latam`, choices `es-latam|en`), `--source-language` (si difiere del target, traduce antes de sintetizar), `--compute-backend` (`auto|cpu|cuda|mps`), `--exaggeration`, `--cfg-weight`, `--temperature`.

### Análisis

Dos naturalezas distintas dentro del mismo hallazgo:

1. **Cross-lingual integrado (pérdida funcional real)**: el contrato §13 es explícito — *"speech say/speech synthesize reemplazan --language por --target-language"* (línea 589) — pero el Rust **no tiene ninguno de los dos**. La capacidad de sintetizar en un idioma distinto del texto de entrada desapareció de `synthesize`/`say` y solo sobrevive como pipeline en `speech dub` (que exige audio de entrada, no texto). La E2E de v0.18.26 ejercitó cross-lingual únicamente vía `dub es→es`, por lo que esta pérdida no quedó cubierta.
2. **Overrides de engine (caída probablemente deliberada, no desdocumentada)**: `--exaggeration` y `--cfg-weight` son parámetros de Chatterbox, el engine anterior; con Qwen3-TTS no aplican. Pero el contrato §13 (línea 619) sigue prometiéndolos para `dub`, junto con `--compute-backend` y `--temperature`. Nótese que el engine Qwen3 **sí** acepta temperatura (`-T 0.35` en la configuración de producción usada por la E2E), hoy inaccesible desde el CLI.

### Corrección propuesta

Decidir por bloque: restaurar `--target-language`/`--source-language` en `synthesize`/`say` (el contrato les dedica §13 completo y el oráculo lo implementaba); desdocumentar los overrides de Chatterbox; evaluar exponer la temperatura del engine Qwen3 si el tuning manual vuelve a ser necesario.

**Confianza**: Alta.

## 5. Hallazgo P3 — `speech dub` renombró sus flags de idioma sin reflejarlo en el contrato

El oráculo usaba `--source-language` (requerido, `es-latam|en`) y `--target-language` (default `es-latam`); el contrato §13 (línea 619) documenta esos nombres. El Rust usa `--from` (default `es`) y `--to` (default `en`) sin `value_parser` (`src/main.rs:229-232`). El renombrado es razonable (paridad con `translate`), pero el contrato quedó describiendo flags inexistentes y además se perdió la validación de choices (`es-latam`/`en` → texto libre).

**Corrección**: actualizar §13 y/o restaurar `value_parser` con los valores válidos.

**Confianza**: Alta.

## 6. Hallazgo P4 — `daemon start`/`serve` perdieron todos sus flags

`DaemonCommands` en Rust tiene las cinco variantes **unitarias** (`src/main.rs:257-268`): `start`, `stop`, `restart`, `status`, `serve` no aceptan ningún flag (más allá de los globales).

El oráculo ofrecía en `start`: `--autorestart`, `--max-retries`, `--language` (default `all`), `--with-stt`; en `serve`: `--auto-restart`, `--max-retries` (default `0` = infinito), `--language`, `--with-stt`. El contrato (línea 593) documenta `daemon start --language {en, all}` precargando el modelo de traducción.

La consecuencia funcional mayor es la pérdida del **auto-reinicio ante crash** como superficie configurable. (Nota de procedencia del oráculo: ya traía una inconsistencia interna — `start` usaba `--autorestart` y `serve` `--auto-restart` — que una restauración debería unificar.)

**Confianza**: Alta.

## 7. Hallazgo P5 — `setup` perdió los modos `--force-update`/`--remove-path`/`--yes`

| Superficie | Oráculo | Rust | Naturaleza |
|---|---|---|---|
| `--force-update` | re-descarga ambos modelos | ❌ | **Pérdida funcional**: la E2E de v0.18.26 tuvo que purgar manualmente para forzar la re-descarga de los ~14 GB |
| `--remove-path` | quita el symlink de PATH y termina | ❌ (subsumido por `uninstall`) | Rediseño aceptable |
| `--uninstall` | desinstala en un paso | ❌ (rediseñado como comando propio `uninstall` + `cleanup --all`) | Rediseño deliberado, documentado |
| `--yes` | omite confirmación | ❌ | Perdido |
| `--language` | choices `es-latam\|en\|all`, default `all` | texto libre, default `"es"` (`src/main.rs:127-128`) | Divergente: el contrato (593) promete `setup --language {en, all}` con conversión CT2 — exactamente la provisión que falta en el P8 |

**Corrección**: restaurar `--force-update` (o equivalente) y el `value_parser` de `--language`; decidir la política de confirmación.

**Confianza**: Alta.

## 8. Hallazgo P6 — Divergencias menores

1. **`speech list --voice` (P6.1)** — el hallazgo que originó esta auditoría (hallazgo 5 de la E2E de v0.18.26). **Síntoma observado en la E2E**: `speech list --voice mi_voz --json` rechazado con exit 2 — el paso 6 del guion (`.claude/skills/test-windows-e2e-as-final-user`) lo ordena esperando una lista filtrada, y el filtrado que el guion asume es client-side sobre el campo `voice` de cada entrada del payload `{"speech":[{label, voice, …}]}`. **Confirmado como regresión de paridad**: el oráculo lo implementaba con validación exit 3 (`_require_voice_exists`) y filtrado (`list_entries(voice=...)`); el contrato lo promete (líneas 170, 278, 321-325); el Rust es variante unitaria (`src/main.rs:181-182`). Corrección recomendada: **restaurar el flag** (la justificación de UX del contrato línea 278 — distinguir "voz mal escrita" de "sin resultados" — sigue siendo válida), no desdocumentarlo; con la restauración, el guion de E2E vuelve a ser fiel al contrato sin corrección.
2. **`translate --from/--to`** — oráculo: requeridos con choices `es|en`; Rust: opcionales con defaults `es`/`en`, sin choices (`src/main.rs:105-108`). Drift menor; el default razonable puede quedarse si se documentan los choices.
3. **`--play` en `speech synthesize`** — el contrato §4 documenta un bucle interactivo (reproduce y pregunta antes de guardar, incompatible con `--json`); el Rust reproduce y guarda incondicionalmente (`src/main.rs:853-861`) y es compatible con `--json`. Cambio de semántica no reflejado en §4.

**Confianza**: Alta.

## 9. Hallazgo P7 — Probable panic en `speech dub` sin fuente de audio (defecto nuevo)

### Diagnóstico (lectura de código)

La rama `Dub` (`src/main.rs:929-976`) valida: `--duration` solo con `--mic`, `--mic` requiere `--duration` sin TTY, existencia del archivo `--audio` **si se pasó**, y provisionamiento de modelos. Pero **no valida que se haya pasado `--audio` o `--mic`** — a diferencia de `Transcribe`, que sí lo hace (`if audio.is_none() && !mic` en `src/main.rs:683`).

Con modelos provisionados, `speech dub --from es --to es` (sin fuente) llega a la captura de audio:

```rust
// src/main.rs:1004 (rama Dub, feature native-stt)
avi_audio::load_wav_16k_mono_pcm(audio.expect("validado arriba"))
```

`audio` es `None` y el comentario `// validado arriba` es falso para esta rama — la validación que cita nunca se escribió. Resultado esperado: **panic con exit 101**.

### Por qué no lo alcanzó la E2E

Doble máscara: (1) el guion de E2E siempre pasa `--audio`; (2) el chequeo de provisionamiento de modelos (exit 4, `src/main.rs:966-977`) va **antes** de la captura, así que en una instalación sin modelos el panic es inalcanzable. El defecto solo se manifiesta en una instalación completa — justo el escenario del usuario final.

### Evidencia pendiente

No se reprodujo empíricamente en esta auditoría (requeriría una instalación con modelos provisionados, purgada al cierre de la E2E). La lectura de código es concluyente sobre la ausencia de guarda; el `expect` con `None` es un panic por construcción.

### Corrección propuesta

Copiar la guarda de `Transcribe` a la rama `Dub` (`if audio.is_none() && !mic` → `usage_error` exit 2) y añadir un test de regresión que invoque `speech dub` sin fuente (con modelos simulados o reordenando los chequeos para que la validación de uso preceda a la de provisionamiento, como manda el eje de clasificación del contrato §1: la invocación mal formada se clasifica antes que la precondición de entorno).

**Confianza**: Alta en la lectura de código; pendiente la reproducción empírica.

> **Actualización 2026-09-04 — Corregido:** guarda añadida en `src/main.rs:942` (`if audio.is_none() && !mic` antes de `model_missing`) con `usage_error` `2` y mensaje `Debe especificarse --audio o --mic.` — espejo exacto de `Transcribe` `src/main.rs:685`. Elimina `audio.expect("validado arriba")` `src/main.rs:1006` como `panic 101` y cumple eje `CONTRACT.md §1` (`2` antes que `4`). Verificado con `cargo test` (gates `dub` y `transcribe`) y `speech dub --json` sin fuente `→ 2` sin modelo.

## 10. Hallazgo P8 — `translate es→en` roto para el usuario final: la provisión CT2 del `setup` nunca fue portada

> Trasladado desde la revisión E2E de v0.18.26 (hallazgo 3 de aquel documento, donde se registró inicialmente). La auditoría de flags no lo detectó — es una rotura de paridad **conductual**, invisible a un diff de superficie — y fue identificada al cruzar ambos documentos con el código del oráculo.

### Síntoma

En una instalación limpia con `setup` completado y `doctor --json` en estado `ok`/`ready`, `translate --text "Hola" --from es --to en --json` termina **exit 4** con:

```json
{"error": "El modelo de traducción no está provisionado en 'models/ct2/opus-mt-es-en'.",
 "reason": "model_missing", "schema_version": "3"}
```

### Causa raíz (exacta, localizada — y confirmada como regresión del port)

Doble defecto en `src/main.rs`:

1. **La provisión CT2 existía en el oráculo y no fue portada.** El `setup` del oráculo (`7542962`, `cli.py`) incluía `_provision_translation_pairs`, gateado por `--language en|all`: descargaba los snapshots HF de Marian **y los convertía a formato CT2** con `_convert_translation_model` (equivalente en código a `ct2-transformers-converter`) hacia `default_cache_dir(source, target)` — idempotente, con aviso de "ya convertido" por dirección. El `setup` Rust solo descarga los snapshots HF, que alimentan exclusivamente el chequeo `is_provisioned("marian-es-en")` de `doctor` (`src/main.rs:1746-1749`); la conversión no existe en ningún subcomando del CLI. (En desarrollo los modelos CT2 se convierten manualmente con `ct2-transformers-converter` hacia `models/ct2/` del repo, gitignored.)
2. **Ruta relativa al CWD (degradación respecto del oráculo).** El chequeo de `handle_translate` es `Path::new("models/ct2/opus-mt-es-en").exists()` (`src/main.rs:430-440`), relativo al directorio desde el que se invoque el CLI; el oráculo resolvía contra `default_cache_dir`, un directorio de caché absoluto. Aunque el modelo existiera, traducir funcionaría solo desde el CWD correcto.

El resultado es contradictorio: `doctor` reporta los modelos de traducción como provisionados (por los snapshots HF) mientras `translate` no puede ejecutarse.

### Impacto

`translate` es→en y en→es es inutilizable para cualquier usuario final instalado vía `install-windows.ps1`, con un mensaje de error que además sugiere una ruta que el usuario no puede provisionar por ningún medio soportado.

### Corrección propuesta

Restaurar el comportamiento del oráculo: provisionar la conversión CT2 en `setup` (convertir el snapshot Marian descargado hacia el `data_dir`/cache con la toolchain existente) y resolver la ruta contra el `data_dir`, no contra el CWD. La corrección es una **restauración** — el código de referencia existe en el oráculo — no una decisión de diseño abierta. Alternativa si no se quiere restaurar: deshabilitar el subcomando (o documentarlo como no soportado) y alinear `doctor` para que no reporte "ready" una capacidad que no puede ejecutarse.

**Confianza**: Alta (mensaje reproducido en la E2E + lectura del código Rust de resolución + verificación del oráculo `7542962`: `_provision_translation_pairs` y `_convert_translation_model` en `cli.py`).

> **Actualización 2026-09-04 — Corregido:** `CT2` migrado a `hf_cache_dir/ct2` (`crates/avi-store/src/lib.rs:537`) y provisión hecha determinista e incondicional en `src/main.rs:1438` (sin `language` gate, `ct2_conversion_failed` si `python/ctranslate2` falta), `doctor` `src/main.rs:1848` exige `model.bin` por par cuando `Marian HF` está, y `cleanup` `src/main.rs:1555` purga `ct2` junto a `hub/xet`. Invariante canónico: `Marian HF presente ⇒ CT2 model.bin presente`. Mensajes `translate`/`dub` sin flag `language` (`ejecuta setup`).

## 11. Lo que sí migró con paridad (verificado)

Para dejar constancia de que la migración no fue negligente en bloque — atendió la paridad en las rutas calientes y en varios detalles finos:

- `speech transcribe`: la fuente obligatoria `--audio`/`--mic` se valida en runtime (`src/main.rs:683-697`), con el comentario `// Validaciones del oráculo`.
- `speech play`/`remove` (`--label`/`--voice`), `voice clone` (`--name`/`--speech-reference` requerido, `--timbre-reference` opcional, `--force`), `voice remove`/`list`.
- `daemon stop`/`restart`/`status`, `doctor`, `devices`, `version`.
- `--json` pasó de por-subcomando a global — mejora deliberada que además le dio `--json` a `dub` y `serve`, que en el oráculo no lo tenían.
- Alias legacy conservados: `speech dub --audio` acepta `--file`; `setup --with-base` acepta `--with-clone`/`--clone`.
- `uninstall` como comando propio con `--force`/`--yes` (rediseño documentado de `setup --uninstall`).

## 12. Diagnóstico de fondo

El patrón es consistente en las roturas de superficie (P1-P6): el port fue fiel donde el guion de pruebas ejercitaba el código (síntesis, clonación, dispatch, store) y perdió superficie donde nada la verificaba (gestión, configuración, controles finos). Ni la suite ni el contrato actuaron como red: los tests dorados cubren las rutas calientes, y `CONTRACT.md` se sincronizó para el stack técnico (engine Parakeet, rutas de crates) sin auditar la tabla de flags contra el binario. El H5 de la E2E fue detectable solo porque un humano ejecutó el guion que citaba el contrato.

P8 agrega una segunda lección: la pérdida no se limitó a flags. Un diff de superficie — como esta auditoría — no ve comportamiento de provisión; la conversión CT2 perdida solo emergió al cruzar el síntoma de la E2E con el código del oráculo. Por eso el corolario operativo debe ir más allá de un drift-detector de flags: **cualquier corrección de esta lista debería acompañarse de un mecanismo que impida reincidir** — un test de contrato que afirme la tabla de flags de `CONTRACT.md` contra el `--help` del binario (drift detector documental↔binario) **y** gates E2E/CI que cubran los comportamientos que el contrato promete (provisión incluida), no solo las rutas calientes del guion.

## 13. Orden de corrección recomendado

1. **P7 (panic de `dub`)**: defecto puro, fix trivial (una guarda), test de regresión barato. Primero por relación costo/beneficio.
2. **P8 (provisión CT2 de `translate`)**: funcionalidad inutilizable para el usuario final y restauración con código de referencia existente (el oráculo). Se planificaba como fase 2 de la revisión E2E; la ruta de corrección ya estaba definida allí.
3. **P1 (cleanup granular)**: la rotura de mayor superficie. Decisión de producto: restaurar los cinco modos o desdocumentarlos; en cualquiera de los dos caminos, `--all` debe dejar de significar dos cosas distintas.
4. **P6.1 (`speech list --voice`)**: restaurar (la decisión ya está tomada por el contrato y el oráculo; es el hallazgo original de la E2E).
5. **P2 (cross-lingual en `synthesize`/`say`)** + **P3/P5 (renombrados y choices)**: bloque de idiomas; restaurar lo que el diseño sostiene, desdocumentar lo que el engine nuevo vuelve irrelevante.
6. **P4 (flags de `daemon`)**: evaluar si el auto-reinicio sigue siendo un requisito del producto.
7. **Sincronización de `CONTRACT.md` §11/§13** con lo que se decida en cada punto — hoy el documento normativo miente en dos secciones enteras.

Todo esto es compatible con (y debería planificarse junto a) las fases 1-3 del orden de corrección de la revisión E2E de v0.18.26.
