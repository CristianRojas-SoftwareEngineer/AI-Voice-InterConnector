# Integración con el plugin de narración de Claude Code

Este documento describe la integración de TTS-Sidecar con el plugin de narración
por voz **tts-sidecar-narrator**, desde la perspectiva del **motor (el
proveedor)**.

La contraparte, escrita desde la perspectiva del plugin, está en su repositorio:
[docs/INTEGRATION.md](https://github.com/CristianRojas-SoftwareEngineer/tts-sidecar-narrator/blob/main/docs/INTEGRATION.md).
El documento de diseño original y puntero al repo es
[CLAUDE-CODE-PLUGIN.md](CLAUDE-CODE-PLUGIN.md).

## Tabla de contenidos

- [Rol en el sistema de narración](#rol-en-el-sistema-de-narración)
- [El contrato de integración](#el-contrato-de-integración)
- [Qué NO comparten los dos proyectos](#qué-no-comparten-los-dos-proyectos)
- [Punto de entrada para el usuario](#punto-de-entrada-para-el-usuario)

## Rol en el sistema de narración

El sistema de narración por voz tiene dos componentes con repositorios y ciclos
de vida independientes:

| Componente | Repositorio | Rol |
|------------|-------------|-----|
| **TTS-Sidecar** (este) | `TTS-Sidecar` | **Motor**: sintetiza voz 100 % offline y expone una CLI pública estable. |
| **tts-sidecar-narrator** | [`tts-sidecar-narrator`](https://github.com/CristianRojas-SoftwareEngineer/tts-sidecar-narrator) | **Cliente**: plugin de Claude Code que narra la actividad de la sesión pidiendo síntesis a este motor. |

La dependencia es **unidireccional**: el plugin consume a TTS-Sidecar. Este repo
**no** conoce, importa ni depende del plugin — no hay ningún código, test ni
build de TTS-Sidecar que sepa de su existencia. El plugin es, a efectos del
motor, un consumidor externo más de la CLI, como un script de usuario.

## El contrato de integración

El único acoplamiento es la **CLI pública** (`tts-sidecar` en `PATH`). El plugin
depende de estas superficies y de la estabilidad de sus flags y de su esquema
JSON:

| Superficie | Qué consume el plugin | Compromiso de estabilidad |
|------------|-----------------------|----------------------------|
| `speech say --text "<msg>" --daemon` | Síntesis y reproducción de cada locución. Usa el daemon y falla si no está levantado (no lo arranca solo). | Mantener el flag `--daemon` y su semántica (usar el daemon, no auto-arrancarlo). |
| `doctor --json` | Verifica el entorno; busca en `checks[]` el elemento con `name == "Chatterbox model"` y lee su `status`. | Mantener `checks[].name == "Chatterbox model"` y los valores `PASS`/`FAIL`. |
| `daemon status --json` | Lee `running` para saber si el daemon corre. | Mantener el campo booleano `running`. |
| `daemon start` | Levanta el daemon para dejar el modelo en memoria. | Mantener el subcomando y su arranque desanclable. |

Cambiar cualquiera de estos nombres, flags o campos **rompe la narración** sin
que este repo tenga tests que lo detecten (el plugin vive fuera). Por eso esta
tabla es el contrato a preservar; al tocar `cli.py` en `speech say`, `doctor` o
`daemon`, revísala.

## Qué NO comparten los dos proyectos

- **Código**: el plugin es TypeScript sobre el Node.js que trae Claude Code; no
  importa el paquete `tts_sidecar`.
- **Versionado**: TTS-Sidecar versiona el motor (binarios por SO, PyPI); el
  plugin versiona con el campo `version` de `plugin.json`, al ritmo de Claude
  Code. Un fix en uno no obliga a un release del otro.
- **CI e infraestructura**: PyInstaller + pytest + gates de cobertura aquí;
  toolchain TypeScript + `claude plugin validate` allá.

## Punto de entrada para el usuario

Desde el lado del motor no hay nada que instalar para el plugin: basta con que
`tts-sidecar` esté en el `PATH` y el modelo esté en caché (`tts-sidecar setup`).
El plugin y su comando `/tts-sidecar-narrator:install` se encargan del resto. El
detalle de cómo el plugin orquesta hooks y degradación vive en su
[documento de integración](https://github.com/CristianRojas-SoftwareEngineer/tts-sidecar-narrator/blob/main/docs/INTEGRATION.md).
