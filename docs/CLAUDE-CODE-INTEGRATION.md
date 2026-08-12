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

Es un **consumidor** del CLI público (`ai-voice-interconnector` en PATH): no importa el
paquete `ai_voice_interconnector`, no comparte código ni requiere el árbol fuente. Sus
propiedades relevantes para esta integración:

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
| `speech say --text "<msg>" --daemon` | Síntesis y reproducción de cada locución. Usa el daemon y falla si no está levantado (no lo arranca solo). | Mantener el flag `--daemon` y su semántica (usar el daemon, no auto-arrancarlo). |
| `speech transcribe --audio <wav> --source-language <lang> --daemon` | No lo consume el plugin (registro de impacto de la Fase 5): transcripción con despacho al daemon de tres modos, forma nueva. | Mantener los tres modos (`--daemon`/`--no-daemon`/autodetección) y el shape `--json` `{"text", "source"}`. |
| `speech dub --mic --source-language <lang> --target-language <lang> -v <voz>` | No lo consume el plugin (registro de impacto de la Fase 5): composición voz→voz (transcribe → traduce → sintetiza → reproduce), forma nueva. | Mantener `--audio`/`--mic` mutuamente excluyentes (exactamente una) y `--source-language` requerido. |
| `doctor --json` | Verifica el entorno; busca en `checks[]` los elementos cuyo `name` empieza con `"Chatterbox model"` (uno por idioma: `"Chatterbox model (es-latam)"` y `"Chatterbox model (en)"`) y lee su `status`. | Mantener el prefijo `checks[].name == "Chatterbox model"` (con sufijo ` (<idioma>)` por modelo) y los valores `PASS`/`FAIL`. |
| `daemon status --json` | Lee `running` para saber si el daemon corre. | Mantener el campo booleano `running`. |
| `daemon start` | Levanta el daemon para dejar los modelos en memoria. | Mantener el subcomando y su arranque desanclable. |

Cambiar cualquiera de estos nombres, flags o campos **rompe la narración** sin
que este repo tenga tests que lo detecten (el plugin vive fuera). Por eso esta
tabla es el contrato a preservar; al tocar `cli.py` en `speech say`,
`speech transcribe`, `speech dub`, `doctor` o `daemon`, revísala.

## Qué NO comparten los dos proyectos

El acoplamiento real es solo el contrato público del CLI; todo lo demás es
disjunto, y por eso el plugin vive en su propio repositorio:

- **Código**: el plugin es TypeScript sobre el Node.js que trae Claude Code; no
  importa el paquete `ai_voice_interconnector`.
- **Versionado**: AI-Voice-InterConnector versiona el motor (binarios por SO, PyPI); el
  plugin versiona con el campo `version` de `plugin.json`, al ritmo de Claude
  Code. Un fix en uno no obliga a un release del otro.
- **CI e infraestructura**: PyInstaller + pytest + gates de cobertura aquí;
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
