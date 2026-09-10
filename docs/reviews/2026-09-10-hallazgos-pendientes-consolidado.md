# Hallazgos pendientes — revisión consolidada

- **Fecha**: 2026-09-10
- **Estado**: 0 resueltos — 14 pendientes
- **Alcance**: todos los defectos, gaps y deudas de medición pendientes del producto, unificados en un solo índice. Sin historia, sin referencias cruzadas a revisiones previas, sin identificadores heredados.
- **Orden**: IDs secuenciales por severidad (críticos → bajos); dentro de cada sección, primero ciclo de vida, luego superficie CLI, luego medición.

> **Leyenda de severidad**: 🔴 **Crítica** — fuga de recursos o cuelgue en producción · 🟠 **Alta** — feature contratada inexistente, crash o degradación silenciosa · 🟡 **Media** — palanca operativa ausente o gap funcional menor · ⚪ **Baja** — pulido de superficie o medición pendiente.

## Tabla de contenidos

- [1. Índice](#1-índice)
- [2. Críticos](#2-críticos)
  - [H-01 — Huérfanos tras abortos y fallos](#h-01--abortos-y-fallos-dejan-daemon--motor-huérfanos)
  - [H-02 — Warmup sin deadline](#h-02--el-warmup-se-cuelga-sin-deadline-visible)
- [3. Altos](#3-altos)
  - [H-03 — Daemon retiene stdio](#h-03--el-daemon-retiene-el-stdio-del-proceso-que-lo-lanzó)
  - [H-04 — Residente inobservable](#h-04--el-motor-de-voz-no-deja-traza-observable)
  - [H-05 — Degradación por reutilización](#h-05--el-daemon-reutilizado-degrada-sirve-estado-pero-falla-síntesis)
  - [H-06 — Sin control de idioma/STT en daemon](#h-06--daemon-startserve-sin-control-de-idioma-ni-stt)
  - [H-07 — Precarga de clone inexistente](#h-07--voice-clone---daemon-promete-precarga-inexistente)
  - [H-08 — Panic con `--mic` sin `--duration`](#h-08--panic-con---mic-sin---duration-en-terminal)
- [4. Medios](#4-medios)
  - [H-09 — Setup sin reinstalación forzada](#h-09--setup-sin-reinstalación-forzada-ni-confirmación)
  - [H-10 — List sin filtro por voz](#h-10--speech-list-sin-filtro-por-voz)
- [5. Bajos](#5-bajos)
  - [H-11 — Translate sin choices](#h-11--translate---from--to-acepta-cualquier-texto)
  - [H-12 — Play sin flujo interactivo](#h-12--speech-synthesize---play-sin-flujo-interactivo)
  - [H-13 — Rama alternativa del gate](#h-13--rama-alternativa-del-gate-de-traducción-sin-evidencia)
  - [H-14 — Techos de guards sin re-medir](#h-14--techos-de-guards-de-tests-sin-re-medir)
- [6. Grafo de relaciones y orden de ataque](#6-grafo-de-relaciones-y-orden-de-ataque)

## 1. Índice

| ID | Título | Severidad | Área |
|---|---|---|---|
| H-01 | Abortos y fallos dejan daemon + motor huérfanos | 🔴 Crítica | Ciclo de vida |
| H-02 | El warmup se cuelga sin deadline visible | 🔴 Crítica | Warmup |
| H-03 | El daemon retiene el stdio del proceso que lo lanzó | 🟠 Alta | Ciclo de vida |
| H-04 | El motor de voz no deja traza observable (stdio a `null`) | 🟠 Alta | Observabilidad |
| H-05 | El daemon reutilizado degrada: sirve estado pero falla síntesis | 🟠 Alta | Daemon |
| H-06 | `daemon start/serve` sin control de idioma ni STT | 🟠 Alta | CLI/Daemon |
| H-07 | `voice clone --daemon` promete precarga inexistente | 🟠 Alta | CLI/Contrato |
| H-08 | Panic con `--mic` sin `--duration` en terminal | 🟠 Alta | CLI |
| H-09 | `setup` sin reinstalación forzada ni confirmación | 🟡 Media | CLI/Setup |
| H-10 | `speech list` sin filtro por voz | 🟡 Media | CLI |
| H-11 | `translate --from/--to` acepta cualquier texto | ⚪ Baja | CLI |
| H-12 | `speech synthesize --play` sin flujo interactivo | ⚪ Baja | CLI/UX |
| H-13 | Rama alternativa del gate de traducción sin evidencia | ⚪ Baja | Motor |
| H-14 | Techos de guards de tests sin re-medir | ⚪ Baja | Tests |

## 2. Críticos

### H-01 — Abortos y fallos dejan daemon + motor huérfanos

- **Severidad**: 🔴 Crítica · **Área**: ciclo de vida (`crates/avi-daemon/src/spawn.rs`, apagado en `src/main.rs:1405-1433`)
- **Síntoma**: toda vía anormal (aborto externo, timeout, panic del test antes del apagado) deja vivos a `ai-voice-interconnector` + `qwen_tts`, ociosos, ocupando puertos y ficheros. Observado en 4 ocasiones en un solo día; un antecedente consumió 8 horas de CPU.
- **Causa**: el apagado solo corre en la vía feliz; ningún guard (`Drop`, watchdog, reaper) lo garantiza en vías anormales. Demostrado por eliminación (los procesos sobreviven a la muerte de su padre).
- **Impacto**: quema de energía, puertos ocupados, y cada huérfano contamina la siguiente corrida (ver H-05).
- **Corrección propuesta**: apagado garantizado (guard de scope + watchdog con deadline que mata el árbol) y verificación de limpieza al cierre de cada test pesado.
- **Relaciones**: alimenta a H-05 · comparte zona de código con H-03 · H-04 lo vuelve indetectable a tiempo.
- **Decisión requerida**: sí — ¿watchdog en producto, en harness, o en ambos?

### H-02 — El warmup se cuelga sin deadline visible

- **Severidad**: 🔴 Crítica · **Área**: warmup (`crates/avi-daemon/src/lib.rs:1218-1257`, warmup en segundo plano tras el bind)
- **Síntoma**: corridas idénticas del mismo comando varían entre warm en ~17s y 150s+ quemando CPU sin llegar al audio, sin mensaje ni fase identificable.
- **Causa**: no demostrada. El warmup es una síntesis única sin deadline propio con diagnóstico; si el residente no responde, nada lo declara fallido a tiempo.
- **Impacto**: cuelgues indistinguibles de lentitud; el observador solo puede abortar a ciegas (ver H-04).
- **Corrección propuesta**: deadline al warmup con diagnóstico de fase + causa visible; depende de H-04 para saber dónde se atasca.
- **Relaciones**: bloqueado por H-04 · alimenta a H-01 (el aborto del observador deja huérfanos) · una vez estable, desbloquea H-06 (preload) y H-14 (re-medir).
- **Decisión requerida**: sí — ¿qué techo (medido, no supuesto) y con qué diagnóstico?

## 3. Altos

### H-03 — El daemon retiene el stdio del proceso que lo lanzó

- **Severidad**: 🟠 Alta · **Área**: ciclo de vida (`crates/avi-daemon/src/spawn.rs:21-66`)
- **Síntoma**: matar el motor no libera el log del lanzador; matar el daemon sí (dos ocasiones). El daemon mantiene ocupados archivos del spawner y puede atar su consola.
- **Causa**: no demostrada del todo (hipótesis: se hereda `stderr` pese a anular stdio). La anulación actual es necesaria pero insuficiente.
- **Impacto**: ficheros bloqueados, consolas atadas, misma familia que H-01.
- **Corrección propuesta**: auditar los tres streams en el lanzamiento y en el respawn de supervisión + regresión con tempfile.
- **Relaciones**: misma zona que H-01 y H-04 (los tres se resuelven en el lanzamiento del daemon).
- **Decisión requerida**: sí — ¿en qué scope se agenda?

### H-04 — El motor de voz no deja traza observable

- **Severidad**: 🟠 Alta · **Área**: observabilidad (lanzamiento del residente en `crates/avi-tts/src/lib.rs`, healthcheck `wait_health` en `:1000-1018`)
- **Síntoma**: el hijo del motor arranca con los tres streams a `null`; un atasco pre-audio no deja ninguna traza en ningún canal.
- **Causa**: decisión de diseño (silencio total del residente), no bug puntual. Demostrada en código.
- **Impacto**: H-02, H-05 y cualquier atasco futuro son imposibles de diagnosticar; toda verificación es a ciegas.
- **Corrección propuesta**: log del residente a fichero rotativo en vez de `null`, con niveles; prerrequisito de H-02 y H-05.
- **Relaciones**: desbloquea a H-02 y H-05 · comparte zona con H-03.
- **Decisión requerida**: sí — ¿fichero siempre o solo con flag de diagnóstico?

### H-05 — El daemon reutilizado degrada: sirve estado pero falla síntesis

- **Severidad**: 🟠 Alta · **Área**: daemon (`DaemonState::new` en `crates/avi-daemon/src/lib.rs:116-128`, `translate_handler` `:594-708`, `dub_handler` `:977-988`)
- **Síntoma**: con daemon residual reutilizado, el `dub` vía daemon sale exit 5 (timeout de cliente `/dub` 10s) en 12s; con arranque fresco, exit 0 en ~10s. El daemon responde `status`/`warm` pero no sirve la petición.
- **Causa**: no demostrada (requiere H-04 para investigar).
- **Impacto**: la fixture de sesión reutiliza el daemon por diseño, por lo que un residual degradado envenena toda la sesión de tests.
- **Corrección propuesta**: validar salud real (no solo `warm`) al reutilizar, o no reutilizar nunca un daemon ajeno a la sesión.
- **Relaciones**: alimentado por H-01 · diagnosticable solo tras H-04.
- **Decisión requerida**: sí — ¿revalidar al reutilizar o arranque fresco siempre?

### H-06 — `daemon start/serve` sin control de idioma ni STT

- **Severidad**: 🟠 Alta · **Área**: CLI/daemon (`DaemonCommands::{Start,Serve}` en `src/main.rs:265-286`)
- **Síntoma**: `daemon start/serve` solo aceptan `--auto-restart`/`--max-retries`; no hay `--language` (preload de modelos por idioma) ni `--with-stt` (precarga de transcripción), aunque el daemon tiene STT funcional. Falla con `unrecognized argument`.
- **Causa**: migración que descartó flags funcionales. Demostrada contra el oráculo.
- **Impacto**: sin control de preload; los consumidores que lo esperan no pueden usarlo.
- **Corrección propuesta**: reimplementar ambos flags mapeados al preload real, o purgarlos formalmente del contrato con motivo documentado.
- **Relaciones**: altera el arranque/warmup → implementar después de estabilizar H-02 · mismo dilema implementar-vs-documentar que H-07 (resolver ambas decisiones en una sola sesión de diseño).
- **Decisión requerida**: sí — ¿reimplementar o purgar?

### H-07 — `voice clone --daemon` promete precarga inexistente

- **Severidad**: 🟠 Alta · **Área**: CLI/contrato (`docs/CLI/CONTRACT.md:238,241`, handler `voices_clone_handler`, `crates/avi-daemon/src/lib.rs:738` devuelve siempre `"precomputed": false`)
- **Síntoma**: el contrato promete que `--daemon` precarga los embeddings antes de clonar, pero el endpoint `POST /voices/precompute` no existe (purgado); el flag existe y el enrutado funciona, la feature no.
- **Causa**: purga del endpoint sin actualizar contrato. Demostrada (el router no lo registra).
- **Impacto**: clonado vía daemon sin la aceleración contratada; especificación falsa para integradores `--json`.
- **Corrección propuesta**: implementar el endpoint o corregir el contrato a lo que realmente hace (`/voices/clone` sin precompute).
- **Relaciones**: mismo router y documento que H-06; mismo dilema implementar-vs-documentar.
- **Decisión requerida**: sí — ¿endpoint o contrato?

### H-08 — Panic con `--mic` sin `--duration` en terminal (validado 2026-09-10)

- **Severidad**: 🟠 Alta · **Área**: CLI (`src/main.rs`, validación `:792-798` y `:1086-1094` frente a `duration.expect("validado arriba")` en `:846`, `:1157`, `:2576`, `:3014`)
- **Síntoma**: en TTY, `speech transcribe --mic` o `dub --mic` sin `--duration` (el caso push-to-talk que `USAGE.md` documenta como funcional) no pide Enter ni usa default: atraviesa la validación —que exime expresamente el TTY— y revienta en `Option::expect` con panic, fuera de toda disciplina de exit codes del contrato.
- **Causa**: demostrada por lectura (2026-09-10): la exención TTY existe en la validación pero su implementación no existe en ningún path —no hay espera de Enter ni duración medida en `src/main.rs` (búsqueda de `Enter|push_to_talk` vacía salvo el comentario)—. Sin TTY el mismo caso sale limpio con exit 2; en TTY es panic en las 4 vías (transcribe/dub × directo/daemon). Nota: `capture_16k_mono_pcm` en sí (`crates/avi-audio/src/lib.rs:179-246`) no tiene panics alcanzables con dispositivos reales (solo `channels == 0` o mutex envenenado, teóricos); el defecto está aguas arriba, en el despacho.
- **Impacto**: crash con stack trace en el flujo interactivo documentado; rompe el contrato de exit codes.
- **Corrección propuesta**: implementar el push-to-talk prometido (espera de Enter + duración medida) o exigir `--duration` también en TTY y corregir `USAGE.md`; convertir los 4 `expect` en error exit 2 como defensa.
- **Relaciones**: roza H-12 (ambos tocan UX interactiva de audio); independiente del resto.
- **Decisión requerida**: sí — ¿push-to-talk real o `--duration` obligatorio?

## 4. Medios

### H-09 — `setup` sin reinstalación forzada ni confirmación

- **Severidad**: 🟡 Media · **Área**: CLI/setup (`src/main.rs:122-131`)
- **Síntoma**: sin `--force-update` no hay forma de re-descargar modelos sin purga manual (~14 GB); sin `--yes` no hay modo no interactivo; `--language` es texto libre sin choices (`es-latam|en|all`).
- **Impacto**: la palanca operativa que faltó ante provisiones rotas; fricción en CI.
- **Corrección propuesta**: restaurar `--force-update` (o equivalente), `--yes` y `value_parser` de `--language`.
- **Relaciones**: cierra el loop operativo de provisión; independiente del resto.
- **Decisión requerida**: sí — política de confirmación y equivalencia exacta de `--force-update`.

### H-10 — `speech list` sin filtro por voz

- **Severidad**: 🟡 Media · **Área**: CLI (`SpeechCommands::List` unitaria en `src/main.rs:188`, contrato `CONTRACT.md:170`)
- **Síntoma**: `speech list --voice/-v` prometido en contrato es rechazado; imposible distinguir "voz mal escrita" de "sin resultados" (contrato §278).
- **Impacto**: guion E2E y UX de filtrado rotos a nivel menor.
- **Corrección propuesta**: restaurar `--voice/-v` con validación exit 3, o corregir el contrato.
- **Relaciones**: ninguna (aislado, una línea + validación).
- **Decisión requerida**: no — implementar el flag.

## 5. Bajos

### H-11 — `translate --from/--to` acepta cualquier texto

- **Severidad**: ⚪ Baja · **Área**: CLI (`src/main.rs:102-105`, contrato `:596`)
- **Síntoma**: valores libres donde el contrato promete `es|en`; inválidos entran sin error temprano.
- **Impacto**: menor; errores de tipeo llegan lejos sin diagnóstico.
- **Corrección propuesta**: `value_parser = ["es", "en"]` o documentar texto libre.
- **Relaciones**: ninguna (aislado).
- **Decisión requerida**: no.

### H-12 — `speech synthesize --play` sin flujo interactivo

- **Severidad**: ⚪ Baja · **Área**: CLI/UX (contrato §4 `:185-203`, `src/main.rs:857-865` reproduce y guarda incondicionalmente)
- **Síntoma**: el contrato promete bucle de 4 opciones (reproducir, aceptar, regenerar, descartar); el binario reproduce y guarda sin preguntar.
- **Impacto**: UX documentada inexistente; cualquier cambio roza el humo de audio de los tests.
- **Corrección propuesta**: implementar el loop o actualizar §4 a `play→save→done` con decisión explícita.
- **Relaciones**: roza los tests de audio (el humo de `say`) y H-08 (UX interactiva) · requiere la decisión de H-08 antes: ambos definen la UX interactiva de audio y no deben diseñarse por separado · hacerlo tras estabilizar ciclo de vida.
- **Decisión requerida**: sí — ¿loop o desdocumentar?

### H-13 — Rama alternativa del gate de traducción sin evidencia

- **Severidad**: ⚪ Baja · **Área**: motor (gate `is_ct2_provisioned`, orden de `auto::Tokenizer` en `ct2rs`)
- **Síntoma**: el gate acepta `tokenizer.json` o `source.spm`+`target.spm` pero rechaza `vocab.json`+`merges.txt` por falta de evidencia de que algún snapshot la satisfaga.
- **Impacto**: ninguno mientras ningún snapshot la requiera; riesgo de falso negativo futuro.
- **Corrección propuesta**: demostrarla contra modelos reales o mantener el rechazo.
- **Relaciones**: pertenece a provisión (zona ya estable); cabe en cualquier orquestación de medición.
- **Decisión requerida**: no — medir cuando se toque traducción.

### H-14 — Techos de guards de tests sin re-medir

- **Severidad**: ⚪ Baja · **Área**: tests (`tests/cli_golden.rs`, constantes 180s resto / 360s dub)
- **Síntoma**: techos derivados de constantes del producto, sin baseline medido con modelos reales.
- **Impacto**: techos mal calibrados (falsos positivos o esperas largas).
- **Corrección propuesta**: re-medir en la próxima corrida pesada instrumentada y ajustar.
- **Relaciones**: cuelga del cluster ciclo de vida (necesita warmup estable para medir bien).
- **Decisión requerida**: no.

## 6. Grafo de relaciones y orden de ataque

```
H-04 (traza del residente)
 ├─ desbloquea ─> H-02 (deadline de warmup)
 ├─ desbloquea ─> H-05 (degradación por reutilización)
 └─ comparte zona ─> H-03 (streams del lanzamiento) ── H-01 (huérfanos)
H-01 ── alimenta ──> H-05 (residual degradado)
H-02 ── abortos ──> H-01 · H-01 ── contamina ──> H-05
H-02 estable ── permite ──> H-06 (flags de preload) · H-14 (re-medir techos)
H-09 · H-13 ── independientes (provisión/medición)
H-10 · H-11 ── triviales aislados (relleno)
H-07 + H-06 ── mismo dilema implementar-vs-documentar (superficie daemon)
H-08 ⇆ H-12 (UX interactiva de audio: decidir H-08 primero, diseñar H-12 después)
H-12 ── última (toca UX de audio + humo de tests)
```

**Orden recomendado (con fundamento)**:

1. **H-04 → H-03 + H-01** — base observable e higiénica (misma zona: lanzamiento del daemon); **luego H-02 + H-05** con traza ya visible, **re-midiendo H-14**. Fundamento: sin traza no hay diagnóstico posible y todo lo que toca el daemon depende de un arranque estable.
2. **H-09 + H-13** — independientes, pequeños, sin decisiones; rellenan mientras se mide el warmup.
3. **Sesión única de decisiones H-06 + H-07 + H-08** — las tres son implementar-vs-documentar/purgar; decidirlas juntas evita tres rondas. Luego implementar lo decidido.
4. **H-10 + H-11** — triviales aislados.
5. **H-12 última** — requiere la decisión de H-08 ya resuelta y ciclo de vida estable.
