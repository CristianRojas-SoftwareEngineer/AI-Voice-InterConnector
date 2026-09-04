# Revisión: superficie CLI vs daemon — diseño ideal

- **Fecha:** 2026-09-04
- **Estado:** Especificación — reemplaza a `models-preload.md` (diagnóstico de precarga)
- **Alcance:** Superficie de la CLI (`src/main.rs`) y del daemon (`crates/avi-daemon/src/lib.rs`), arbitrada por `docs/CLI/CONTRACT.md` y `docs/CLI/commands/DAEMON.md`
- **Origen:** Auditoría sistémica del desfase CLI↔daemon tras cerrar `models-preload` (`42095b6` — CT2 residente + `POST /translate`/`/voices/clone`/`/dub`)
- **Propósito original del daemon:** reducir latencia de inferencia en frío — mantener en memoria `Qwen3TtsEngine`/`ParakeetEngine`/`Ct2TranslationEngine` (`DaemonState:69-87`, `warmup_tts:1169` `default→ryan`, `GET /health:197` `warm: Warm`) y evitar `15-30s` de carga + init `CUDA/cuDNN`/`oneDNN` por invocación

## Tabla de contenidos

- 1. Resumen
- 2. Superficie actual — CLI vs router
- 3. Criterio de auditoría (4 ejes)
- 4. Clasificación ideal
- 5. Gaps, sobrantes y optimizaciones
- 6. Diseño ideal y recomendaciones
- 7. Referencias

## 1. Resumen

El daemon nació como **acelerador de inferencia con `warm`**, no como backend completo. Tras `42095b6` los 6 comandos que cargan pesos tienen vía `daemon` y vía directa (`route_to_daemon:2335` 3 modos `Auto/--daemon/--no-daemon`), y los comandos sin modelo permanecen `local-only` por disponibilidad. El desfase restante no es de precarga — es **híbrido de estado**: `DaemonState:69-71` contiene `voice_store`/`speech_store` file-backed (`crates/avi-store/src/lib.rs:65,249`) mientras el CLI también instancia `VoiceStore::new()`/`SpeechStore::new()` directo para `list`/`remove`/`play`; `GET /voices:203` existe huérfana sin consumidor y `POST /voices/precompute:233` duplica `POST /voices/clone:1129`. El diseño ideal es **daemon = acelerador puro** (6 `Ambos`, 15 `Local-only`, 2 `Daemon-only`, podar 2 huérfanas) o, si el daemon pasa a remoto/no file-backed, **daemon = fuente de verdad con 5 CRUD con fallback**. Exponer todos los comandos por el daemon sin distinguir carga de pesos no da beneficio de precarga y añade `500ms` + `DaemonUnreachable:5` a I/O trivial.

## 2. Superficie actual — CLI vs router

**CLI `src/main.rs:92-288`** — 21 superficies:

- `version`, `devices`, `translate {text,from,to}`, `voice {list,clone,remove}`, `speech {list,transcribe,synthesize,say,dub,play,remove}`, `daemon {start,stop,restart,status,serve}`, `setup {language,with_stt,with_base}`, `cleanup {voices,synthetic_speech,model,all}`, `uninstall`, `doctor`

**Router `crates/avi-daemon/src/lib.rs:1124-1141`** — 9 rutas:

- `GET /health:197` (`health_handler` `warm: warming|warm|warm_failed`)
- `GET /voices:203` (`voices_handler`)
- `POST /voices/precompute:233` (`voices_precompute_handler` — regraft `.wav→.qvoice` `clone_voice:253`)
- `POST /voices/clone:1129` (`voices_clone_handler:670` `audio_b64`+`validate_name:148`+`save_reference:599`)
- `POST /synthesize:1131` (`synthesize_handler:310` NDJSON `start→progress→result{audio_b64}`)
- `POST /transcribe:1138` (`transcribe_handler:464` `cfg native-stt` `audio_b64` `i16le 16kHz:471`→`stt_engine.transcribe:484`)
- `POST /translate:1140` (`translate_handler:555` `cfg native-translation` `ct2_engine:87` `HashMap es-en/en-es`)
- `POST /dub:1130` (`dub_handler:817` pipeline `transcribe→ct2→synthesize:371` bajo `synthesis_lock:321`)
- `POST /shutdown:1101` (`shutdown_handler` `shutdown_notify:88`)

**Desfase:** `voice list:493`/`remove:605`, `speech list:641`/`play:1152`/`remove:1186` no tienen ruta y son `require_local:495,606,643,1154,1188`; `voice clone:511`/`speech dub:933`/`translate:398` sí tienen ruta vía `clone_via_daemon:2679`/`dub_via_daemon:2776`/`translate_via_daemon:2428`; `GET /voices:203` y `POST /voices/precompute:233` existen sin consumidor CLI directo; `devices`/`version`/`setup`/`cleanup`/`uninstall`/`doctor` sin ruta — correcto.

## 3. Criterio de auditoría (4 ejes, no solo precarga)

Un endpoint se justifica si suma en alguno:

1. **Precarga** — evita recarga de `Qwen3TtsEngine`/`ParakeetEngine`/`Ct2TranslationEngine` y paga `warm` una vez (`DaemonState:69,77,87`, `warmup_tts:1169`, `GET /health:197`). Solo 6 comandos cargan pesos.
2. **Estructura** — elimina doble camino al mismo estado (`VoiceStore`/`SpeechStore` file-backed `crates/avi-store/src/lib.rs:65,249` vs `DaemonState:69-71`). Hoy ambos apuntan al mismo `data_dir` en disco — funciona por filesystem, no por invariante; con daemon remoto o `SQLite` se rompe.
3. **Eficiencia** — reduce round-trips o `p50` con `warm: warm`. `POST /dub:817` en 1 `POST` `10000ms` `src/main.rs:2776` vs 3 fríos local (`Parakeet:1052`+`ct2_model_dir:540`+`Qwen3:1141`).
4. **Disponibilidad/mantenibilidad** — no hace `list` trivial dependiente de `127.0.0.1:8765` vivo (`daemon_activo:2342` `500ms` + `DaemonUnreachable:5`).

## 4. Clasificación ideal

### A. Ambos — `route_to_daemon:2335` 3 modos (`Auto` sonda `500ms` → daemon si `warm`, `ForceDaemon→true:2337` exige daemon `5`, `ForceDirect→false:2338` fuerza local)

| Comando | Carga pesos | Endpoint daemon | Por qué Ambos |
|---|---|---|---|
| `speech synthesize` `src/main.rs:794` | sí `Qwen3TtsEngine::new:111` | `POST /synthesize:1131` `synthesize_handler:310` | Precarga + estructura: evita `warm` por invocación |
| `speech say` `src/main.rs:885` | sí `Qwen3` | `POST /synthesize` reuso + `AudioService::play_wav` local `src/main.rs:2649` `say_via_daemon` | Mismo motor que `synthesize`, `play` es local post-síntesis |
| `speech transcribe` `src/main.rs:679` | sí `ParakeetEngine::new:791` `nemo128.onnx` | `POST /transcribe:1138` `transcribe_handler:464` `cfg native-stt` | Precarga `Parakeet` + `stt_unsupported` si falta feature |
| `speech dub` `src/main.rs:933` | sí triple `Parakeet:1052`+`CT2:540`+`Qwen3:1141` | `POST /dub:1130` `dub_handler:817` pipeline + fallback `transcribe_via_daemon:2358`+`translate` local+`daemon_synthesize_wav` | Precarga: `3 fríos→0` (pipeline) o `→1` (composición) |
| `translate` `src/main.rs:398` | sí `CT2TranslationEngine` `ct2_model_dir:540` `model.bin` | `POST /translate:1140` `translate_handler:555` `ct2_engine:87` | Precarga `CT2` residente `HashMap es-en/en-es` |
| `voice clone` `src/main.rs:511` | sí `Qwen3TtsEngine::new:587`+`clone_voice:253` | `POST /voices/clone:1129` `voices_clone_handler:670` `audio_b64`+`validate_name:148`+`save_reference:599` | Precarga + escritura en `state.voice_store:69` |

Validaciones puras (`empty_text:417`, `unsupported_language_pair`, `audio_not_found:995`) antes de sondeo — eje `CONTRACT.md:235-249` `2` antes que `5`.

### B. Local-only — `require_local:495,606,643,1154,1188` (funcionan sin daemon vivo)

| Comando | Carga pesos | Por qué Local-only |
|---|---|---|
| `voice list:493`, `voice remove:605` | no (`VoiceStore::list`/`remove`) | Sin `warm` que ahorrar; `GET /voices:203` existe huérfana — pasar por daemon añadiría `500ms`+`5` a `O(n)` I/O sin beneficio, pierde disponibilidad |
| `speech list:641`, `speech remove:1186`, `speech play:1152` | no (`SpeechStore:249`/`play_wav`) | `play` es dispositivo local `audio::AudioService`; `GET /speech/{label}/audio` solo tendría sentido remoto |
| `version:27`, `devices` (`audio::get_devices_json`), `setup:123`, `cleanup:132`, `uninstall:148`, `doctor:157` | no | Provisión/diagnóstico/`VERSION`/`devices` — `DaemonState::new:69` exige modelos provisionados, `setup` no puede ir vía daemon |
| `daemon start/stop/restart/status:262` | no (gestionan `spawn_background:21` `0x02000000|0x8|0x200` + `POST /shutdown:1101` `1500ms` + `GET /health:197`) | Son el ciclo de vida del daemon, no inferencia |

Total `Local-only`: 15 superficies (5 estado + 6 provisión/diagnóstico + 4 gestión daemon). Suman con `Ambos` (6) y `Daemon-only` (2) las 21 superficies de la CLI.

### C. Daemon-only

| Comando/superficie | Por qué Daemon-only |
|---|---|
| `daemon serve:280` `run_daemon_server:1196` (`TcpListener:618`+`spawn_blocking(warmup_tts):1169`+`with_graceful_shutdown:640`) | Es el servidor |
| `GET /health:197` `warm: warming|warm|warm_failed` + `POST /shutdown:1101` `shutdown_notify:88` | Señalización del residente, no CLI cliente |

## 5. Gaps, sobrantes y optimizaciones

**Sobrantes a podar (daemon = acelerador):**

- `GET /voices:203` — existe sin consumidor CLI (`voice list` va a `VoiceStore` local `src/main.rs:493`). Si se mantiene `B. Local-only`, borrar ruta.
- `POST /voices/precompute:233` — duplica `POST /voices/clone:1129` (ambos `clone_voice:253→save_reference:599`, uno desde `reference.wav` legado `lib.rs:233`, otro desde `audio_b64:670`). Mantener `clone` y borrar `precompute` como ruta pública (dejar función interna si hace falta migración).

**Gaps solo si daemon pasa a fuente de verdad remota/no file-backed (no por precarga):**

- `GET /voices` (consumida), `DELETE /voices/{name}`, `GET /speech`, `DELETE /speech/{voice}/{label}`, `GET /speech/{voice}/{label}/audio` (para `play` remoto). Con `127.0.0.1` file-backed no se justifican; con daemon remoto, añadir las 5 con **fallback local** (igual que `synthesize:794` — `route_to_daemon` con degradación) para no perder disponibilidad. No exponer `devices`/`version`/`setup` por daemon.

**Optimizaciones ya aplicadas y a preservar:**

- `POST /dub:817` 1 round-trip `10000ms` `src/main.rs:2776` preferente, composición `transcribe_via_daemon:2358`+`CT2` local+`daemon_synthesize_wav` como fallback si `native-stt`/`native-translation` no compiladas.
- `POST /say` no se añade — `say:885` es `synthesize:310`+`play` local.
- `GET /health:197` debería reportar `ct2: warm`/`stt: warm` además de `tts: warm` cuando `ct2_engine:87`/`stt_engine:77` están residentes (hoy solo `tts`).

## 6. Diseño ideal y recomendaciones

**Mantener daemon como acelerador con `warm`**, no como backend completo:

1. **Conservar 6 `Ambos` + 15 `Local-only` + 2 `Daemon-only`** (`CONTRACT.md:235-249` tabla `Comando|Delegable|Endpoint|Razón local-only: sin modelo`, `DAEMON.md:52-64` 9→7 rutas tras poda).
2. **Podar `GET /voices:203` y `POST /voices/precompute:233`** como rutas públicas (o completar 5 CRUD con fallback solo si roadmap es daemon remoto).
3. **No añadir `POST /say` ni `GET /speech`/`play` remoto** para `127.0.0.1` — `play` es `AudioService` local.
4. **No exponer todos los comandos por el daemon:** operaciones sin modelo por daemon no dan beneficio de precarga y añaden `500ms`+`5` a I/O trivial, cambiando el propósito de "acelerador" a "backend completo" sin retorno.

La medida de éxito es objetiva: con daemon `warm: warm` (`GET /health` sin `warm_error`), `speech dub en→es` de `3 fríos→0` (`POST /dub`) o `→1` (composición), `cargo test --lib` `53 passed` + `cargo test -p avi-daemon` `6 passed` + `cli_golden` con `warm: warm` sin `warm_failed`.

## 7. Referencias

- `crates/avi-daemon/src/lib.rs:69-87` `DaemonState`, `lib.rs:197,203,233,310,464,555,670,817,1101,1124` handlers/router, `lib.rs:1169` `warmup_tts`
- `src/main.rs:27,71,398,493,511,605,633,679,794,885,933,1152,1186,2305,2335,2358,2428,2602,2649,2679,2776` `DaemonMode`/`daemon_client`/`route_to_daemon`/`*_via_daemon`/`require_local`
- `crates/avi-store/src/lib.rs:65,249,540` `VoiceStore`/`SpeechStore`/`ct2_model_dir`
- `docs/CLI/CONTRACT.md:235-249` (tabla delegabilidad), `docs/CLI/commands/DAEMON.md:52-64` (endpoints)
