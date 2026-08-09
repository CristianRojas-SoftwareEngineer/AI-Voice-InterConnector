# Documentación de la CLI de TTS-Sidecar

Referencia completa de la interfaz de línea de comandos de TTS-Sidecar. Este directorio contiene el contrato normativo de la CLI y documentación de investigación detallada por comando.

## Estructura

```
docs/CLI/
├── README.md            ← este archivo (índice)
├── CONTRACT.md          ← contrato público: comandos, flags, códigos de salida, payloads --json
└── commands/            ← documentación de investigación por comando
    ├── SPEECH.md
    ├── VOICE.md
    ├── DEVICES.md
    ├── DOCTOR.md
    ├── SETUP.md
    ├── CLEANUP.md
    ├── DAEMON.md
    ├── VERSION.md
    └── TRANSLATE.md
```

## Documentos

| Documento | Contenido |
|---|---|
| [CONTRACT.md](CONTRACT.md) | Contrato normativo: invariantes de diseño, vocabulario, códigos de salida, payloads `--json`, reglas de validación, matrices de comportamiento |
| [commands/SPEECH.md](commands/SPEECH.md) | Investigación del grupo `speech`: síntesis, reproducción, dubbing, transcripción, gestión del almacén |
| [commands/VOICE.md](commands/VOICE.md) | Investigación de `voice`: clonación, listado y eliminación de voces |
| [commands/DEVICES.md](commands/DEVICES.md) | Investigación de `devices`: enumeración de dispositivos de audio |
| [commands/DOCTOR.md](commands/DOCTOR.md) | Investigación de `doctor`: diagnósticos del sistema y patrón de veredicto |
| [commands/SETUP.md](commands/SETUP.md) | Investigación de `setup`: provisión del runtime, modelos y PATH |
| [commands/CLEANUP.md](commands/CLEANUP.md) | Investigación de `cleanup`: borrado de modelos, voces y habla sintética |
| [commands/DAEMON.md](commands/DAEMON.md) | Investigación de `daemon`: ciclo de vida, endpoints FastAPI, protocolo IPC |
| [commands/VERSION.md](commands/VERSION.md) | Investigación de `version`: fuente de versión y payload |
| [commands/TRANSLATE.md](commands/TRANSLATE.md) | Investigación de `translate`: pipeline de traducción, divergencia ISO vs CLI |

## Resumen de comandos

La CLI expone **9 comandos** de nivel superior con **15 subcomandos** en total. Framework: `argparse` (stdlib). Punto de entrada: `tts-sidecar` (vía `pyproject.toml`), `bin/tts-sidecar` (launcher), o `python -m tts_sidecar`.

### Grupos nominales (con subcomandos)

| Comando | Subcomandos | Propósito |
|---|---|---|
| `speech` | `synthesize`, `say`, `dub`, `play`, `list`, `remove`, `transcribe` | Síntesis de habla, gestión del almacén, transcripción, composición voz→voz |
| `voice` | `list`, `clone`, `remove` | Gestión del registro de voces |
| `daemon` | `start`, `stop`, `restart`, `status`, `serve` | Ciclo de vida del daemon FastAPI |

### Comandos standalone

| Comando | Propósito |
|---|---|
| `devices` | Lista dispositivos de audio del sistema |
| `doctor` | Ejecuta diagnósticos del sistema |
| `setup` | Provisión del runtime (modelos, PATH) |
| `cleanup` | Borrado de modelo, voces y/o habla sintética |
| `version` | Muestra la versión |
| `translate` | Traduce texto es↔en sin síntesis de audio |

### Códigos de salida

| Código | Constante | Significado |
|---|---|---|
| 0 | `EXIT_OK` | Éxito |
| 1 | `EXIT_ERROR` | Error genérico |
| 2 | `EXIT_INVALID_INPUT` | Entrada inválida |
| 3 | `EXIT_NOT_FOUND` | Recurso no encontrado |
| 4 | `EXIT_MODEL_MISSING` | Modelo no provisionado |
| 5 | `EXIT_DAEMON_UNREACHABLE` | Daemon inalcanzable |
| 6 | `EXIT_STATE_CONFLICT` | Conflicto de estado |
| 7 | `EXIT_NOT_APPLICABLE` | Operación no aplicable |
| 8 | `EXIT_PRECONDITION_FAILED` | Precondición incumplida |
| 9 | `EXIT_TRANSLATION_FAILED` | Fallo de traducción |
| 10 | `EXIT_TRANSCRIPTION_FAILED` | Fallo de transcripción |
| 130 | `EXIT_INTERRUPTED` | Interrupción por usuario (Ctrl+C) |

Todos los comandos soportan `--json` para salida machine-readable (excepto `daemon serve`). La clase `CliError` hereda de `BaseException` para evitar captura por `except Exception` genérico.
