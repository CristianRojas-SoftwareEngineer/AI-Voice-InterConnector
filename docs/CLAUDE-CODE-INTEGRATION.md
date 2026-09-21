# Integración con Claude Code

Este documento describe la integración de AI-Voice-InterConnector con **tts-sidecar-narrator**,
un plugin de [Claude Code](https://code.claude.com) que narra por voz la actividad
de la sesión, desde la perspectiva del **motor (el proveedor)**.

La contraparte, escrita desde la perspectiva del plugin, vive en su repositorio:
[docs/INTEGRATION.md](https://github.com/CristianRojas-SoftwareEngineer/tts-sidecar-narrator/blob/main/docs/INTEGRATION.md).
El diseño completo, la arquitectura de sus componentes y sus decisiones detalladas
también viven allá; este repo solo documenta el contrato que debe preservar.

## Tabla de contenidos

- [Rol en el sistema de narración](#rol-en-el-sistema-de-narración)
- [Qué es el plugin](#qué-es-el-plugin)
- [El contrato de integración](#el-contrato-de-integración)
- [Qué NO comparten los dos proyectos](#qué-no-comparten-los-dos-proyectos)
- [Punto de entrada para el usuario](#punto-de-entrada-para-el-usuario)

## Rol en el sistema de narración

El sistema de narración por voz tiene dos componentes con repositorios y ciclos
de vida independientes:

| Componente | Repositorio | Rol |
|------------|-------------|-----|
| **AI-Voice-InterConnector** (este) | `AI-Voice-InterConnector` | **Motor**: sintetiza voz 100 % offline y expone una CLI pública estable. |
| **tts-sidecar-narrator** | [`tts-sidecar-narrator`](https://github.com/CristianRojas-SoftwareEngineer/tts-sidecar-narrator) | **Cliente**: plugin de Claude Code que narra la actividad de la sesión pidiendo síntesis a este motor. |

La dependencia es **unidireccional**: el plugin consume a AI-Voice-InterConnector. Este repo
**no** conoce, importa ni depende del plugin — no hay ningún código, test ni
build de AI-Voice-InterConnector que sepa de su existencia. El plugin es, a efectos del
motor, un consumidor externo más de la CLI, como un script de usuario.

## Qué es el plugin

`tts-sidecar-narrator` **narra por voz** la actividad de la sesión de Claude
Code. Al final de cada turno (y en avisos relevantes) el usuario escucha un
mensaje conversacional corto en español —no el texto en bruto del asistente,
sino una locución procesada.

Es un **consumidor** del CLI público (`ai-voice-interconnector` en PATH): no
comparte código ni requiere el árbol fuente. Sus propiedades relevantes para
esta integración:

- **Automático**: disparado por hooks (`Stop`, `Notification`), sin intervención
  del modelo ni del usuario. `SessionStart` verifica el entorno y deja el daemon
  caliente.
- **No intrusivo**: nunca bloquea ni retrasa el turno; falla en silencio si
  AI-Voice-InterConnector no está disponible.

El resto de sus propiedades de diseño (costo cero, sin runtime extra,
multiplataforma, activación/desactivación) vive en el repositorio del plugin,
que es su fuente de verdad.

## El contrato de integración

El único acoplamiento es la **CLI pública** (`ai-voice-interconnector` en `PATH`). El plugin
depende de estas superficies y de la estabilidad de sus flags y de su esquema
JSON:

| Superficie | Qué consume el plugin | Compromiso de estabilidad |
|------------|-----------------------|----------------------------|
| `speech say --text "<msg>" --daemon` | Síntesis dinámica y reproducción de cada locución. Usa el daemon y falla si no está levantado (no lo arranca solo). | Mantener el flag `--daemon` y su semántica (usar el daemon, no auto-arrancarlo). |
| `speech synthesize --text "<aviso>" --label <label>` | Pre-síntesis de avisos; se guarda en el SpeechStore bajo un `label`. | Mantener el flag `--label` y la colisión de label (exit `6`, ver abajo). |
| `speech play --label <label>` | Reproducción de una locución pre-cacheada por `label`. | Mantener el subcomando `play` y el flag `--label`; exit `3`/`NotFound` si el label no existe. |
| `doctor --json` | Verifica el entorno; lee `status` (`ok`/`failed`) y `issues[]`. | Mantener el campo `status` con esos valores y la lista de `issues`. |
| `daemon status --json` | Lee `running` (booleano) para saber si el daemon corre. | Mantener el campo booleano `running`. |
| `daemon start` | Levanta el daemon para dejar los modelos en memoria. | Mantener el subcomando y su arranque desanclable. |

Además de estas seis superficies, el plugin interpreta dos **exit codes
semánticos** (definidos en `crates/avi-core/src/exit_codes.rs`) para tomar
decisiones sin parsear stderr:

- exit `5` = `DaemonUnreachable`: el daemon está caído o inalcanzable; el
  plugin lo usa para saber que no responde.
- exit `6` = `StateConflict`, emitido con reason `"label_exists"` cuando un
  `--label` ya existe en `speech synthesize`; el plugin lo usa para no
  re-sintetizar un aviso ya cacheado.

Cambiar cualquiera de estos nombres, flags, campos o exit codes **rompe la
narración** sin que este repo tenga tests que lo detecten (el plugin vive
fuera). Por eso esta tabla es el contrato a preservar; al tocar `src/main.rs`
en `speech say`, `speech synthesize`, `speech play`, `doctor`, `daemon` o
`crates/avi-core/src/exit_codes.rs`, revísala.

`speech transcribe` y `speech dub` existen en la CLI pero **no** los consume
el plugin; no forman parte de este contrato.

## Qué NO comparten los dos proyectos

El acoplamiento real es solo el contrato público del CLI; todo lo demás es
disjunto, y por eso el plugin vive en su propio repositorio:

- **Código**: el plugin es TypeScript sobre el Node.js que trae Claude Code;
  no depende del árbol Rust.
- **Versionado**: AI-Voice-InterConnector versiona el motor (binarios por SO);
  el plugin versiona con el campo `version` de `plugin.json`, al ritmo de Claude
  Code. Un fix en uno no obliga a un release del otro.
- **CI e infraestructura**: cargo test/build + publicaciones por tag aquí;
  toolchain TypeScript + `claude plugin validate` allá.

Además, el modelo de distribución de plugins (marketplaces) asume un repo git
propio.

## Punto de entrada para el usuario

Desde el lado del motor no hay nada que instalar para el plugin: basta con que
`ai-voice-interconnector` esté en el `PATH` y los modelos estén en caché (`ai-voice-interconnector setup`).

El repositorio del plugin dobla como su propio marketplace:

```
/plugin marketplace add CristianRojas-SoftwareEngineer/tts-sidecar-narrator
/plugin install tts-sidecar-narrator@tts-sidecar-narrator
/tts-sidecar-narrator:install
```

El comando `/tts-sidecar-narrator:install` guía la instalación del binario
AI-Voice-InterConnector, la descarga de los modelos y la activación de la narración. El detalle
de cómo el plugin orquesta hooks y degradación vive en su
[documento de integración](https://github.com/CristianRojas-SoftwareEngineer/tts-sidecar-narrator/blob/main/docs/INTEGRATION.md).
