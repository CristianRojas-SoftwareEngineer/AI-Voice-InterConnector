# UX interactiva de audio — captura push-to-talk y revisión de síntesis

- **Alcance**: `speech transcribe --mic`, `speech dub --mic` (captura por voz sin duración fija) y `speech synthesize --play` (revisión interactiva de la toma sintetizada).
- **Documentos relacionados**: [`docs/CLI/CONTRACT.md`](CLI/CONTRACT.md) §4 (contrato normativo), [`docs/CLI/commands/SPEECH.md`](CLI/commands/SPEECH.md) (comportamiento por comando), [`docs/MANUAL-VALIDATION.md`](MANUAL-VALIDATION.md) (validación de las rutas TTY).

## Tabla de contenidos

- [1. Qué resuelve](#1-qué-resuelve)
- [2. Principios de diseño transversales](#2-principios-de-diseño-transversales)
- [3. Captura push-to-talk (`--mic` sin `--duration`)](#3-captura-push-to-talk---mic-sin---duration)
- [4. Revisión interactiva de síntesis (`--play`)](#4-revisión-interactiva-de-síntesis---play)
- [5. Diseño de implementación (Rust)](#5-diseño-de-implementación-rust)
- [6. Verificación](#6-verificación)
- [7. No-objetivos](#7-no-objetivos)

## 1. Qué resuelve

Dos comandos de audio necesitan interacción con la persona en la terminal, no solo parámetros fijos:

- **Captura por voz sin cronómetro**: al grabar del micrófono (`speech transcribe --mic`, `speech dub --mic`) no siempre se conoce de antemano cuántos segundos durará el habla. La captura **push-to-talk** graba mientras la persona habla y termina cuando pulsa Enter, en lugar de exigir un `--duration N` adivinado. `--duration` sigue disponible para grabación de duración fija no interactiva (scripts, tuberías).
- **Aceptar o rehacer una síntesis antes de guardarla**: al sintetizar voz con reproducción (`speech synthesize --play`), la primera toma puede no convencer. El **bucle de revisión** reproduce la toma y ofrece un menú de cuatro opciones (reproducir de nuevo, aceptar y guardar, regenerar, descartar) en lugar de reproducir y persistir de forma incondicional.

Ambas capacidades son interactivas por naturaleza: solo se habilitan con una terminal en la entrada estándar (TTY). Sin TTY, el comportamiento es determinista (`--duration` para la captura) o un error de uso explícito, nunca un cuelgue esperando entrada que no llegará.

## 2. Principios de diseño transversales

Reglas comunes a ambas capacidades que garantizan una UX interactiva coherente:

- **P1 — Validación previa pura.** Toda guarda (TTY, exclusión de flags, incompatibilidad `--json`) se evalúa **antes de abrir dispositivos, sintetizar o transcribir**. Un caso rechazado nunca produce efectos secundarios.
- **P2 — Nunca panic por ruta alcanzable.** Ninguna precondición violada se manifiesta como `panic`; siempre es un `CliError` con exit code de dominio.
- **P3 — Separación de canales.** Avisos, menús y prompts interactivos van **siempre a stderr**; stdout queda reservado al payload de datos (texto o JSON) y a los mensajes finales de confirmación en modo humano (p. ej. «Descartado.», «Voz clonada.»), que no son prompts sino el resultado de la invocación.
- **P4 — TTY como guarda de interactividad.** La interactividad (push-to-talk, bucle `--play`) solo se habilita con stdin en terminal (`is_terminal()`); sin TTY el comportamiento es determinista o error explícito.
- **P5 — Exit codes canónicos.** `InvalidInput = 2`, `NotFound = 3`, `StateConflict = 6`. Sin códigos nuevos.
- **P6 — Captura siempre client-side.** El micrófono está en la máquina del cliente; incluso en modo daemon el CLI captura el PCM y lo envía. Push-to-talk **no toca el daemon**.
- **P7 — Interactividad silenciosa por seguridad.** Cualquier salvaguarda que altere el flujo (techo de duración, aviso al grabar) se comunica por stderr al ocurrir; nunca deja a la persona sin señal ni contamina el payload.

## 3. Captura push-to-talk (`--mic` sin `--duration`)

Aplica **por igual** a `speech transcribe` y `speech dub`, en sus rutas directa y vía daemon.

### 3.1 Superficie de flags

| Flag | Semántica |
|---|---|
| `--audio <ruta>` / `--mic` | Mutuamente excluyentes; **uno obligatorio**. |
| `--duration <N>` | **Solo válido con `--mic`**. Graba `N` segundos de duración fija, sin interacción. |
| `--mic` sin `--duration` | **Push-to-talk**: exige TTY; graba hasta Enter. |

### 3.2 Matriz de comportamiento con `--mic`

| stdin | `--duration` | Comportamiento |
|---|---|---|
| No-TTY | ausente | Error `usage_error` (exit 2), antes de tocar hardware. |
| No-TTY | presente | Grabación fija de `N` s. |
| TTY | presente | Grabación fija de `N` s. |
| TTY | ausente | Push-to-talk (grabar hasta Enter). |

### 3.3 Reglas de validación (previas, puras — P1/P2)

- `--mic` sin `--duration` y **sin TTY** → `usage_error` (exit 2): «sin terminal interactiva, usa `--duration N` para grabar sin Enter.» La captura **no se inicia**.
- `--duration` sin `--mic` → `usage_error` (exit 2): «`--duration` solo es válido con `--mic`.»
- Ni `--audio` ni `--mic` → `usage_error` (exit 2): «Debe especificarse `--audio` o `--mic`.»

### 3.4 Contrato de push-to-talk

- La captura arranca al entrar en la rama `--mic` sin `--duration` (solo alcanzable en TTY por las reglas de §3.3).
- **Aviso al arrancar**: se emite a **stderr** un aviso mínimo (p. ej. «Grabando… pulsa Enter para detener.»); nunca a stdout (P3).
- **Termina cuando la persona pulsa Enter** (lectura de una línea completa de stdin).
- **Techo de seguridad**: la grabación tiene un tope máximo configurable por la variable de entorno `AVI_PUSH_TO_TALK_MAX_SECS` (predeterminado **300 s**). Al alcanzarlo, la captura se detiene sola, emite a **stderr** un aviso («Límite de grabación alcanzado (300 s); deteniendo.») y el pipeline continúa con lo grabado. **No es error: exit 0.** Es una salvaguarda interna contra un Enter olvidado, no un flag de la CLI.
- Formato de captura idéntico al de duración fija: 16 kHz, mono, int16.
- Tras detener (por Enter o por techo), el pipeline sigue igual que en duración fija (transcribe → o dub: transcribe → traduce → sintetiza → reproduce).

### 3.5 Canales y exit codes

- El resultado de `transcribe` va a **stdout** (texto plano, o JSON `{"text","source"}` con `--json`); sin diferencia de canal entre mic-fijo y mic-push-to-talk. Los avisos de captura van a **stderr** (P3).

| Código | Causa |
|---|---|
| 0 | Completado (incluye corte por techo) |
| 2 | Reglas de validación de §3.3 |
| 3 | (solo `--audio`) fichero inexistente |
| 4 / 5 / 10 | modelo no provisionado / daemon inalcanzable / fallo de transcripción |

## 4. Revisión interactiva de síntesis (`--play`)

### 4.1 Precondiciones (previas a sintetizar — P1)

- `--play` + `--json` → `usage_error` (exit 2): «`--play` y `--json` son incompatibles: el bucle interactivo usa la entrada y la salida estándar y contaminaría el payload.»
- `--play` sin TTY en stdin → `usage_error` (exit 2): «`--play` requiere una terminal interactiva en la entrada estándar; no la hay en esta invocación.»

### 4.2 Flujo del bucle

1. Se sintetiza **una vez** antes del bucle.
2. Esa primera toma se **reproduce** al entrar.
3. Se imprime en **stderr** el menú fijo y se lee una línea de stdin con prompt `"Opción [1-4]: "`:
   ```
   ¿Qué quieres hacer con esta toma?
     1) Reproducir otra vez
     2) Aceptar y guardar
     3) Rechazar y regenerar
     4) Rechazar y descartar
   ```

| Opción | Efecto | Re-sintetiza | Persiste | Termina | Exit |
|---|---|---|---|---|---|
| `1` | Reproduce la **misma** toma en memoria | No | No | No | — |
| `2` | Acepta: **revalida colisión** y guarda | No | Sí (si libre) | Sí | 0, o **6** si colisiona sin `--force` |
| `3` | Regenera: nueva síntesis completa + reproduce | Sí | No | No | — |
| `4` | Descarta | No | No | Sí | 0 |
| otro | Aviso «Opción no válida; escribe 1, 2, 3 o 4.» (stderr) | No | No | No | — |
| EOF (Ctrl-D) | Igual que `4`; imprime «Descartado.» (stdout) | No | No | Sí | 0 |

### 4.3 Revalidación de colisión de etiqueta (crítico)

Al elegir **opción 2**, el sistema **recomprueba** si la etiqueta ya existe para la voz **en el instante de guardar** (no basta la comprobación al inicio del comando). Si existe y no hay `--force` → `StateConflict` (exit 6), sin persistir. Cierra la ventana de carrera del intervalo interactivo, que puede durar minutos.

### 4.4 Invariantes de eficiencia

- Opción 1 → **cero** re-síntesis (secuencia `1,2` = 1 síntesis total).
- Opción 3 → **exactamente una** síntesis nueva por regeneración (secuencia `3,4` = 2 síntesis).
- La reproducción ocurre: al entrar (toma inicial), en cada `1` y en cada `3`; **nunca** en `2` ni `4`.

### 4.5 Efectos observables

- Fin por `2` (aceptado): existe el WAV + sidecar (`speech_store.find(voice,label)` presente).
- Fin por `4` / EOF: **no** queda WAV/sidecar nuevo.

## 5. Diseño de implementación (Rust)

### 5.1 Captura push-to-talk (`crates/avi-audio`)

- `capture_16k_mono_pcm_until_enter(&self) -> Result<Vec<i16>>` convive con `capture_16k_mono_pcm`. Ambas comparten el post-procesado (mono → resample 16 kHz → i16) extraído a un helper privado `finish(recorded, channels, sample_rate)`; difieren **solo** en el disparador de fin: `thread::sleep` (duración fija) vs. espera de una línea de stdin (push-to-talk). El callback de `cpal` llena el buffer en su hilo de audio mientras el hilo principal espera Enter.
- La variante `until_enter` arma el **techo**: si transcurre `AVI_PUSH_TO_TALK_MAX_SECS` antes del Enter, detiene el stream por timeout y devuelve lo grabado (no es error).

### 5.2 Selector en las cuatro vías (`src/main.rs`)

- En las cuatro rutas de captura (transcribe/dub × directo/daemon) la duración se resuelve con un `match`: `Some(d)` → `capture_16k_mono_pcm(d)`; `None` → `capture_16k_mono_pcm_until_enter()` (solo alcanzable en TTY por las guardas de §3.3). Sin `expect` en la rama mic (P2).
- La captura del micrófono se ejecuta dentro de `tokio::task::spawn_blocking` en las cuatro vías, para no bloquear el reactor de tokio durante la espera (bloqueante) de Enter.
- **Ctrl-C durante la grabación** se delega en el manejador global de señales del CLI; sin manejo específico en esta ruta.

### 5.3 Bucle de revisión de síntesis (`src/main.rs`)

- Un `synthesize_play_loop(...)` recibe la primera toma (bytes en memoria) más el contexto (voz, etiqueta, `force`, store) y ejecuta el flujo de §4.2–4.4.
- **Ruta directa**: si `--play`, delega en el bucle en vez de reproducir y guardar de forma incondicional.
- **Ruta daemon**: el bucle vive **en el cliente** (reproducción y prompts son client-side). La **opción 3 (regenerar)** re-despacha la síntesis por la **misma** vía que el camino no interactivo (directo o daemon según corresponda), heredando traducción y daemon sin lógica especial. Los bytes de audio se conservan en memoria en el cliente para las opciones 1 y 2.
- La recomprobación de colisión (§4.3) reutiliza `speech_store.find(&voice, &label)` en el punto de guardar dentro de la opción 2, replicando el patrón fast-fail existente.
- Las precondiciones de §4.1 se validan como guarda previa pura al inicio del brazo `Synthesize`, antes de cualquier síntesis.

## 6. Verificación

Las guardas de validación (no interactivas) se cubren con golden tests en `tests/cli_golden.rs`, que corren en **no-TTY**. Las rutas interactivas (push-to-talk activo, bucle `--play`) requieren TTY y entrada humana; se validan manualmente según [`docs/MANUAL-VALIDATION.md`](MANUAL-VALIDATION.md):

- `transcribe --mic` sin `--duration` y sin TTY → exit 2, sin iniciar captura.
- `--duration` sin `--mic` → exit 2.
- `--mic --duration N` (no-TTY) → captura fija, sin panic. Idéntico para `dub` en las cuatro vías.
- (manual/TTY) sin pulsar Enter, al alcanzar `AVI_PUSH_TO_TALK_MAX_SECS` la captura se detiene sola, avisa a stderr y transcribe lo grabado (exit 0).
- `synthesize --play --json` → exit 2; `synthesize --play` sin TTY → exit 2.
- (manual/TTY) opción 4 y EOF → sin WAV nuevo, exit 0; opción 2 → persiste, y colisión al aceptar sin `--force` → exit 6; secuencia `1,2` = 1 síntesis, `3,4` = 2 síntesis.

## 7. No-objetivos

- No reintroducir el endpoint `/voices/precompute` u otra superficie de daemon: la captura push-to-talk es client-side (P6) y el bucle `--play` también.
- No cambiar el formato de audio, el pipeline de traducción ni los exit codes canónicos.
