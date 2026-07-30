# Plugin de Claude Code: narración por voz con TTS-Sidecar

> **Estado**: extraído a su repositorio propio. Este documento fue la
> especificación de diseño inicial y hoy es un **puntero**: la fuente de verdad
> del plugin es su repositorio independiente.
>
> **Repositorio**: <https://github.com/CristianRojas-SoftwareEngineer/tts-sidecar-narrator>
>
> **Contrato de integración** (superficies del CLI que el plugin consume, desde
> la perspectiva de este motor): [NARRATION-INTEGRATION.md](NARRATION-INTEGRATION.md).

## Tabla de contenidos

- [Qué es](#qué-es)
- [Propiedades de diseño](#propiedades-de-diseño)
- [Por qué un repositorio independiente](#por-qué-un-repositorio-independiente)
- [Instalación (resumen)](#instalación-resumen)

## Qué es

`tts-sidecar-narrator` es un plugin de [Claude Code](https://code.claude.com)
que **narra por voz** la actividad de la sesión usando TTS-Sidecar. Al final de
cada turno (y en avisos relevantes), el usuario escucha un mensaje conversacional
corto —no el texto en bruto del asistente, sino una locución procesada, en
español.

El plugin es un **consumidor** del CLI público (`tts-sidecar` en PATH:
`speech say`, `doctor`, `daemon`): no importa el paquete `tts_sidecar`, no
comparte código ni requiere el árbol fuente. Por eso vive en su propio
repositorio, con ciclo de vida y versionado disjuntos de los del motor.

## Propiedades de diseño

- **Automático**: disparado por hooks (`Stop`, `Notification`), sin intervención
  del modelo ni del usuario. `SessionStart` verifica el entorno y deja el daemon
  caliente.
- **No intrusivo**: nunca bloquea ni retrasa el turno; falla en silencio si
  TTS-Sidecar no está disponible.
- **Multiplataforma**: paridad Windows / Linux / macOS.
- **Sin runtime extra**: los scripts corren sobre el Node.js que Claude Code ya
  trae (TypeScript compilado a `dist/`); no exige Python.
- **Costo cero**: mensajes generados con niveles gratuitos de LLM (Gemini free →
  OpenRouter `:free`) con degradación a un constructor local determinista.
- **Controlable**: activa/desactiva la narración sin desinstalar.

## Por qué un repositorio independiente

El acoplamiento real es solo el contrato público del CLI; los ciclos de vida y
versionado son disjuntos; el modelo de distribución de plugins (marketplaces)
asume un repo git propio; y las infraestructuras de CI no comparten nada
(PyInstaller + pytest aquí, toolchain TypeScript + `claude plugin validate`
allá). El diseño completo, la arquitectura de los componentes y las decisiones
detalladas viven en el repositorio del plugin.

## Instalación (resumen)

El repositorio del plugin dobla como su propio marketplace:

```
/plugin marketplace add CristianRojas-SoftwareEngineer/tts-sidecar-narrator
/plugin install tts-sidecar-narrator@tts-sidecar-narrator
/tts-sidecar-narrator:install
```

El comando `/tts-sidecar-narrator:install` guía la instalación del binario
TTS-Sidecar, la descarga del modelo y la activación de la narración. Consulta el
README y la documentación del repositorio del plugin para el detalle.
