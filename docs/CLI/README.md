# Documentación de la CLI de AI Voice-InterConnector

Referencia completa de la interfaz de línea de comandos de AI Voice-InterConnector. Este directorio contiene el contrato normativo de la CLI y la documentación por comando.

## Estructura

```
docs/CLI/
├── README.md            ← este archivo (índice)
├── CONTRACT.md          ← contrato público: comandos, flags, códigos de salida, payloads --json
└── commands/            ← documentación por comando
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
| [commands/SPEECH.md](commands/SPEECH.md) | Comportamiento e implementación del grupo `speech`: síntesis, reproducción, dubbing, transcripción, limpieza de archivos de audio/vídeo y gestión del almacén |
| [commands/VOICE.md](commands/VOICE.md) | Comportamiento e implementación de `voice`: clonación, listado y eliminación de voces |
| [commands/DEVICES.md](commands/DEVICES.md) | Comportamiento e implementación de `devices`: enumeración de dispositivos de audio |
| [commands/DOCTOR.md](commands/DOCTOR.md) | Comportamiento e implementación de `doctor`: diagnósticos del sistema, sección de ciclo de vida y patrón de veredicto |
| [commands/SETUP.md](commands/SETUP.md) | Comportamiento e implementación de `setup`: provisión del runtime, modelos y el papel de `setup_failed` |
| [commands/CLEANUP.md](commands/CLEANUP.md) | Comportamiento e implementación de `cleanup`: borrado por categorías, reglas de propiedad y confirmación |
| [commands/SELF.md](commands/SELF.md) | Comportamiento e implementación del grupo `self`: `self install` (con reparación) y `self uninstall`, sobre el motor `avi-lifecycle` |
| [commands/DAEMON.md](commands/DAEMON.md) | Comportamiento e implementación de `daemon`: ciclo de vida, endpoints Axum, protocolo IPC |
| [commands/VERSION.md](commands/VERSION.md) | Comportamiento e implementación de `version`: fuente de versión y payload |
| [commands/TRANSLATE.md](commands/TRANSLATE.md) | Comportamiento e implementación de `translate`: validación en dos carriles (parser y handler), passthrough, motor CT2 local y despacho al daemon |
| [../denoise-migration-walkthrough.md](../denoise-migration-walkthrough.md) | Evidencia de la migración Denoise y sus límites declarados; el contrato de uso vigente lo fija [CONTRACT.md](CONTRACT.md) |

## Resumen de comandos

La CLI Rust (clap) expone los comandos de nivel superior que listan las dos tablas siguientes. Punto de entrada: `src/main.rs` (binario `ai-voice-interconnector`); en desarrollo, `cargo run -- <comando>`.

### Grupos nominales (con subcomandos)

| Comando | Subcomandos | Propósito |
|---|---|---|
| `speech` | `synthesize`, `say`, `dub`, `play`, `list`, `remove`, `transcribe`, `denoise` | Síntesis de habla, gestión del almacén, transcripción, composición voz→voz y limpieza local de archivos de audio/vídeo |
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
| 9 | `ExitCode::TranslationFailed` | Fallo de traducción |
| 10 | `ExitCode::TranscriptionFailed` | Fallo de transcripción |
| 11 | `ExitCode::SetupFailed` | Provisión de modelos fallida; el programa **queda instalado** (éxito parcial, reintentable con `setup`) |
| 12 | `ExitCode::ExternallyManaged` | La copia la gestiona otra herramienta |
| 13 | `ExitCode::RolledBack` | Fallo en el reemplazo; versión anterior restaurada |
| 14 | `ExitCode::PathConflict` | Conflicto en la ruta del enlace del `PATH` |
| 15 | `ExitCode::BundleInvalid` | Falta un archivo obligatorio del bundle alrededor del ejecutable |
| 16 | `ExitCode::DaemonStopFailed` | No se pudo detener el daemon; nada modificado |
| 17 | `ExitCode::LifecycleLocked` | Hay otra operación de ciclo de vida en curso |
| 18 | `ExitCode::UnsupportedPlatform` | Plataforma no soportada (triple del host fuera de la tabla); compilar desde el código fuente |
| 19 | `ExitCode::BinaryIncompatible` | El binario descargado no arranca en este sistema o no informa de su versión |
| 20 | `ExitCode::NetworkError` | Fallo de red al resolver o descargar la versión objetivo |
| 21 | `ExitCode::ChecksumMismatch` | El archivo descargado no coincide con el hash de `SHA256SUMS.txt` |
| 22 | `ExitCode::ProgramDirKept` | El resto se desinstaló pero el directorio de programa no se pudo borrar |
| 130 | `ExitCode::Interrupted` | Interrupción por usuario (Ctrl+C, con limpieza acotada de 2 s y salida preservada, con reclamo sin pidfile vía PID en memoria) |

Los códigos 0–6, el 9, el 10 y el 130 son los del contrato de la CLI y no cambian; el 7 y el 8 no existen. Los códigos del 11 al 17 son la tabla cerrada del ciclo de vida, **uno por `reason` de las [reglas transversales](../specs/LIFECYCLE.md#81-reglas-transversales)** (`crates/avi-core/src/exit_codes.rs`). Los del 18 al 22 son la tabla cerrada de `self update` (18–21: plataforma, compatibilidad del binario, red e integridad) más `program_dir_kept` (22) de `self uninstall`; todos los del 11 al 22 salen por `ExitCode::from_reason`, que es la traducción única de `reason` a código.

Todos los comandos soportan `--json` para salida machine-readable (excepto `daemon serve`). `speech denoise` usa ese flag global para emitir JSONL, una línea por evento o resultado. El envelope de la CLI lleva `schema_version`; el **protocolo del daemon lleva su propia versión** ([las dos versiones de esquema](CONTRACT.md#las-dos-versiones-de-esquema)), porque son contratos independientes.

Los esquemas `--json` se mantienen a propósito tanto en [`USAGE.md`](../../USAGE.md) como en cada documento por comando, de modo que cada uno sea autosuficiente: es la única excepción a la regla de dueño único de cada afirmación. `CliError` vive en `crates/avi-core/src/exit_codes.rs` y se traduce en `src/main.rs` (`ExitCode` + `reason`).
