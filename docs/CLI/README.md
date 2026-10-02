# Documentación de la CLI de AI-Voice-InterConnector

Referencia completa de la interfaz de línea de comandos de AI-Voice-InterConnector. Este directorio contiene el contrato normativo de la CLI y documentación de investigación detallada por comando.

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
    ├── SELF.md
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
| [commands/DOCTOR.md](commands/DOCTOR.md) | Investigación de `doctor`: diagnósticos del sistema, sección de ciclo de vida y patrón de veredicto |
| [commands/SETUP.md](commands/SETUP.md) | Investigación de `setup`: provisión del runtime, modelos y el papel de `setup_failed` |
| [commands/CLEANUP.md](commands/CLEANUP.md) | Investigación de `cleanup`: borrado por categorías, reglas de propiedad y confirmación |
| [commands/SELF.md](commands/SELF.md) | Investigación del grupo `self`: `self install` (con reparación) y `self uninstall`, sobre el motor `avi-lifecycle` |
| [commands/DAEMON.md](commands/DAEMON.md) | Investigación de `daemon`: ciclo de vida, endpoints Axum, protocolo IPC |
| [commands/VERSION.md](commands/VERSION.md) | Investigación de `version`: fuente de versión y payload |
| [commands/TRANSLATE.md](commands/TRANSLATE.md) | Investigación de `translate`: pipeline de traducción, divergencia ISO vs CLI |

## Resumen de comandos

La CLI Rust (clap) expone **10 comandos** de nivel superior. Punto de entrada: `src/main.rs` (binario `ai-voice-interconnector`); en desarrollo, `cargo run -- <comando>`.

### Grupos nominales (con subcomandos)

| Comando | Subcomandos | Propósito |
|---|---|---|
| `speech` | `synthesize`, `say`, `dub`, `play`, `list`, `remove`, `transcribe` | Síntesis de habla, gestión del almacén, transcripción, composición voz→voz |
| `voice` | `list`, `clone`, `remove` | Gestión del registro de voces |
| `daemon` | `start`, `stop`, `restart`, `status`, `serve` | Ciclo de vida del daemon nativo (Axum) |
| `self` | `install`, `update`, `uninstall` | Ciclo de vida de la instalación del usuario: instalar/reparar, actualizar y desinstalar |

### Comandos standalone

| Comando | Propósito |
|---|---|
| `devices` | Lista dispositivos de audio del sistema |
| `doctor` | Diagnóstico del sistema y del estado del ciclo de vida (recibo, canal, `PATH`, pendientes, modelos) |
| `setup` | Descarga los modelos pinneados vía HuggingFace Hub (sin índice: solo presencia de snapshot) |
| `cleanup` | Borra el estado por categorías (`--voices`/`--synthetic-speech`/`--model`/`--all`, `--dry-run`, `--yes/-y`; sin categoría → exit 2) sin tocar el programa ni el `PATH` |
| `version` | Muestra la versión |
| `translate` | Traduce texto es↔en sin síntesis de audio |

**`uninstall` ya no es un comando de nivel superior.** Desaparece sin alias, sin flag deprecado y sin periodo de transición: su papel lo cumplen `self uninstall` —que además borra la raíz de datos entera salvo `--keep-data`— y `cleanup` para la limpieza granular. El proyecto es pre-1.0 y no está distribuido, así que no hay instalaciones previas que migrar.

### Códigos de salida

| Código | Constante | Significado |
|---|---|---|
| 0 | `ExitCode::Ok` | Éxito |
| 1 | `ExitCode::Error` | Error genérico |
| 2 | `ExitCode::InvalidInput` | Entrada inválida |
| 3 | `ExitCode::NotFound` | Recurso no encontrado |
| 4 | `ExitCode::ModelMissing` | Modelo no provisionado |
| 5 | `ExitCode::DaemonUnreachable` | Daemon inalcanzable |
| 6 | `ExitCode::StateConflict` | Conflicto de estado |
| 7 | `ExitCode::NotApplicable` | Operación no aplicable |
| 8 | `ExitCode::PreconditionFailed` | Precondición incumplida |
| 9 | `ExitCode::TranslationFailed` | Fallo de traducción |
| 10 | `ExitCode::TranscriptionFailed` | Fallo de transcripción |
| 11 | `ExitCode::SetupFailed` | Provisión de modelos fallida; el programa **queda instalado** (éxito parcial, reintentable con `setup`) |
| 12 | `ExitCode::ExternallyManaged` | La copia la gestiona otra herramienta |
| 13 | `ExitCode::RolledBack` | Fallo en el reemplazo; versión anterior restaurada |
| 14 | `ExitCode::PathConflict` | Conflicto en la ruta del enlace del `PATH` |
| 15 | `ExitCode::BundleInvalid` | Falta un archivo obligatorio del bundle alrededor del ejecutable |
| 16 | `ExitCode::DaemonStopFailed` | No se pudo detener el daemon; nada modificado |
| 17 | `ExitCode::LifecycleLocked` | Hay otra operación de ciclo de vida en curso |
| 130 | `ExitCode::Interrupted` | Interrupción por usuario (Ctrl+C, con limpieza acotada de 2 s y salida preservada, con reclamo sin pidfile vía PID en memoria) |

Los códigos 0–10 y el 130 son los del contrato de la CLI y no cambian. Los siete del 11 al 17 son la tabla cerrada del ciclo de vida, **uno por `reason` de §8.1** (`crates/avi-core/src/exit_codes.rs`).

Todos los comandos soportan `--json` para salida machine-readable (excepto `daemon serve`). El sobre de la CLI lleva `schema_version` `"4"`; el **protocolo del daemon va por `"4"`**, porque son contratos independientes. `CliError` vive en `crates/avi-core/src/exit_codes.rs` y se traduce en `src/main.rs` (`ExitCode` + `reason`), sin herencia Python.
