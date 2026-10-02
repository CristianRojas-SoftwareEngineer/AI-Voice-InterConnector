# Especificación funcional: ciclo de vida de la aplicación y del entorno de desarrollo

> **Estado**: implementada · **Versión base**: v0.23.1 · **Fecha**: 2026-09-25

> **Directiva de no retrocompatibilidad.** El proyecto es pre-1.0 y no requiere ningún tipo de retrocompatibilidad con el ciclo de vida anterior. No hay aliases ni flags deprecados, ni adopción de instalaciones sin recibo, ni migración de datos cuando cambien las rutas, ni scripts puente en las URLs antiguas. El proyecto no está distribuido: no hay instalaciones previas, de modo que no se concede ningún mecanismo de transición.

Este documento especifica cómo se **instala, actualiza, desinstala y limpia** `ai-voice-interconnector` en el entorno del usuario final, y cómo se **prepara, actualiza y limpia** el entorno del desarrollador, con el mismo comportamiento en los cuatro targets de compilación. Evalúa las alternativas de implementación y fija una arquitectura única que reemplaza a los scripts de ciclo de vida de la raíz del repositorio.

Es la fuente de verdad funcional del ciclo de vida: la guía de usuario, el contrato de la CLI y la guía de build se derivan de ella y no deben contradecirla.

## Tabla de contenidos

- [1. Alcance](#1-alcance)
- [2. Glosario](#2-glosario)
- [3. Targets soportados](#3-targets-soportados)
- [4. Evaluación de alternativas](#4-evaluación-de-alternativas)
- [5. Arquitectura](#5-arquitectura)
- [6. Modelo de rutas y propiedad](#6-modelo-de-rutas-y-propiedad)
- [7. Recibo de instalación y canales](#7-recibo-de-instalación-y-canales)
- [8. Entorno del usuario](#8-entorno-del-usuario)
- [9. Entorno del desarrollador](#9-entorno-del-desarrollador)
- [10. Matriz de paridad por target](#10-matriz-de-paridad-por-target)
- [11. Seguridad](#11-seguridad)
- [12. Estrategia de pruebas](#12-estrategia-de-pruebas)
- [13. Papel de la documentación](#13-papel-de-la-documentación)
- [14. Criterios de aceptación](#14-criterios-de-aceptación)
- [15. Decisiones cerradas](#15-decisiones-cerradas)

---

## 1. Alcance

**Dentro del alcance**

- **Entorno del usuario**: primera instalación, reparación, actualización (incluida la consulta de versión disponible), desinstalación y limpieza selectiva del estado de la aplicación.
- **Entorno del desarrollador**: preparación y actualización del entorno de compilación, empaquetado local idéntico al de release, instalación del build local y limpieza de artefactos.
- Los cuatro targets de compilación ([§3](#3-targets-soportados)) y las reglas transversales: confirmación, idempotencia, privilegios, seguridad, salida y códigos de resultado.

**Fuera del alcance**

- Firma de código y notarización (goal a largo plazo; solo se referencia en [§11](#11-seguridad)).
- Actualización automática en segundo plano o comprobación periódica de versiones: la actualización es siempre una acción explícita del usuario.
- Instalación para todos los usuarios del sistema (per-machine) y convivencia de varias versiones instaladas a la vez.
- Canales nuevos de gestores de paquetes (winget, Scoop, apt, AUR). El Cask de Homebrew existente se conserva como canal gestionado externamente ([§7.2](#72-canales)).
- El corte de releases (`xtask release`, `xtask cask`), salvo el empaquetado, que comparte con el entorno de desarrollo.

## 2. Glosario

| Término | Definición |
|---|---|
| **Bundle** | Contenido completo de un release para un target: ejecutable, motor TTS (`qwen_tts`), librería de ONNX Runtime (más las DLL del runtime de VC++ en Windows) y documentos de licencia. Se instala siempre entero. |
| **Archivo de release** | El bundle comprimido (`.tar.gz` o `.zip`) publicado en GitHub Releases y listado en `SHA256SUMS.txt`. |
| **Bootstrap** | Script mínimo de primer contacto (`install.sh`, `install.ps1`) que obtiene y verifica el bundle y delega la instalación en el propio binario. |
| **Directorio de programa** | Directorio donde vive el bundle instalado. Propiedad exclusiva de la aplicación; nunca contiene datos del usuario. |
| **Recibo de instalación** | Archivo JSON dentro del directorio de programa que registra qué se instaló, dónde, por qué canal y qué integración de PATH se aplicó. |
| **Canal** | Vía por la que se instaló la copia: `script`, `dev`, `homebrew` o `unmanaged` ([§7.2](#72-canales)). |
| **Estado de la aplicación** | Todo lo que la aplicación crea en el perfil del usuario fuera del directorio de programa: modelos, voces de usuario, habla sintetizada, configuración, `daemon.pid`, logs y temporales. |
| **Staging** | Directorio de trabajo, hermano del directorio de programa, donde se descarga, verifica y extrae un bundle antes de instalarlo. |
| **Aparcar** | Renombrar un archivo instalado a un subdirectorio `.old-<txid>` en lugar de borrarlo, para poder revertir o para sortear un archivo en uso. |
| **Operación de ciclo de vida** | `self install`, `self update`, `self uninstall`, `cleanup` y las tareas equivalentes de `xtask`. |

## 3. Targets soportados

| Target | Triple de Rust | Archivo de release | Mínimo declarado |
|---|---|---|---|
| Windows x86_64 | `x86_64-pc-windows-msvc` | `ai-voice-interconnector-<ver>-x86_64-windows.zip` | Windows 10 x64 |
| Linux x86_64 | `x86_64-unknown-linux-gnu` | `ai-voice-interconnector-<ver>-x86_64-linux.tar.gz` | glibc ≥ 2.35 |
| Linux arm64 | `aarch64-unknown-linux-gnu` | `ai-voice-interconnector-<ver>-arm64-linux.tar.gz` | glibc ≥ 2.35, userland de 64 bits |
| macOS arm64 | `aarch64-apple-darwin` | `ai-voice-interconnector-<ver>-arm64-macos.tar.gz` | macOS 13 |

**Detección del target.** El bootstrap y el binario aplican la misma tabla:

- **Windows**: se usa la arquitectura nativa del SO, no la del proceso de PowerShell (que puede ser x86 de 32 bits, o x64 emulado en un equipo ARM64). `AMD64` → Windows x86_64; `ARM64` → no soportado.
- **Linux**: `uname -m` ∈ {`x86_64`, `amd64`} → x86_64; ∈ {`aarch64`, `arm64`} → arm64; cualquier otro valor → no soportado.
- **macOS**: `sysctl -n hw.optional.arm64` = `1` → arm64, aunque la terminal corra bajo Rosetta (en ese caso `uname -m` devuelve `x86_64`); en otro caso (Mac Intel) → no soportado.

**Plataformas no soportadas**: Windows ARM64, macOS Intel, Linux con musl (Alpine), Linux de 32 bits y userland de 32 bits sobre kernel de 64 bits. Terminan con `unsupported_platform` (o con `binary_incompatible` si la incompatibilidad solo se detecta al ejecutar el binario) y un mensaje que remite a compilar desde el código fuente ([BUILD.md](../BUILD.md)).

**Compatibilidad comprobada, no inferida.** No se parsean versiones de glibc para decidir si se instala: se ejecuta el binario descargado (`--version`) y, si no arranca, se diagnostica la causa probable (glibc insuficiente vía `getconf GNU_LIBC_VERSION`, musl, userland de 32 bits). Una sola comprobación cubre todas las incompatibilidades de ABI, incluidas las que un parseo no detecta.

## 4. Evaluación de alternativas

### 4.1 Criterios

| Id | Criterio |
|---|---|
| C1 | Una sola implementación de cada regla (sin duplicados por lenguaje ni rutas espejadas). |
| C2 | Comportamiento idéntico en los cuatro targets. |
| C3 | Actualizar y desinstalar sin depender de una copia del repositorio. |
| C4 | Testeable en las tres puertas de test de la CI actual (CircleCI). |
| C5 | Superficie mínima en el repositorio y ninguna pieza de ciclo de vida en la raíz. |
| C6 | Encaje con el bundle multiarchivo y con el pipeline existente. |
| C7 | Coste de adopción. |

### 4.2 Alternativas para el entorno del usuario

| Criterio | A. Estado actual | B. Scripts unificados por shell | **C. Binario autogestionado + bootstrap mínimo** | D. Instalador separado | E. `cargo-dist` | F. Gestores de paquetes |
|---|---|---|---|---|---|---|
| C1 | ❌ 3 lenguajes | ⚠️ 2 lenguajes + Rust | ✅ Rust (bootstrap trivial) | ✅ Rust | ⚠️ scripts generados + Rust | ❌ un manifiesto por gestor |
| C2 | ⚠️ divergencias | ⚠️ | ✅ | ✅ | ⚠️ | ❌ cobertura desigual |
| C3 | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ |
| C4 | ⚠️ bats + Pester | ⚠️ | ✅ | ✅ | ⚠️ | ❌ |
| C5 | ❌ 5 archivos en la raíz | ⚠️ 2 scripts largos | ✅ 2 scripts mínimos fuera de la raíz | ⚠️ crate y artefacto extra | ⚠️ | ❌ |
| C6 | ✅ | ✅ | ✅ | ⚠️ | ❌ | ⚠️ |
| C7 | — | Bajo | Medio | Alto | Alto | Alto |

- **A. Estado actual.** Funciona en el caso feliz, pero incumple C1, C3 y C5: el mismo flujo de instalación, actualización y desinstalación está duplicado en tres lenguajes, con divergencias no intencionadas; las rutas de instalación y de estado están replicadas a mano y exigen tests de paridad para no desincronizarse; la actualización solo funciona desde una copia del repositorio, ejecuta la copia local del instalador y reinstala aunque ya se tenga la última versión; ninguna vía detiene el daemon antes de reemplazar el programa, de modo que en Windows un ejecutable en uso aborta la instalación con el directorio a medio borrar; en Windows, reescribir el PATH de usuario aplana las entradas `%VAR%`, e `irm | iex` deja preferencias y funciones del instalador en la sesión del usuario; la confirmación es inconsistente, porque `xtask clean` exige `--yes` cuando no hay terminal y los comandos de desinstalación y limpieza del producto no; y conviven cinco piezas de ciclo de vida en la raíz del repositorio, tres frameworks de test y siete documentos que describen partes solapadas del mismo ciclo.
- **B. Scripts unificados por familia de shell** (un `install.sh` para Linux y macOS y un `install.ps1` con instalar, actualizar y desinstalar). Reduce de cinco a dos archivos, pero la lógica sigue en dos lenguajes y duplica lo que el binario ya hace en Rust (parada del daemon, rutas, PATH en el registro). Hereda además las limitaciones de `irm | iex` (no admite parámetros y comparte ámbito con la sesión) y un testeo débil.
- **C. Binario autogestionado + bootstrap mínimo** (patrón de `rustup` y `uv`). El producto gestiona su propio ciclo de vida con subcomandos `self`. Para la primera instalación basta un bootstrap por familia de shell, que no puede eliminarse porque en ese momento el binario todavía no existe en la máquina. El conocimiento necesario ya vive en el binario: rutas (`avi-store`), parada unificada del daemon, acceso al registro de Windows y difusión de `WM_SETTINGCHANGE`, y un stack TLS (rustls) por la descarga de modelos. Además, la lógica que instala una versión es la de esa misma versión, así que las correcciones del instalador se aplican en cuanto se publican.
- **D. Instalador o gestor separado** (estilo `rustup-init`). Añade un segundo ejecutable sin firmar por target, lo que duplica el problema de reputación ante SmartScreen y Defender, exige sincronizar versiones entre instalador y aplicación, y no aporta nada que el binario del producto no pueda hacer.
- **E. `cargo-dist` (con `axoupdater`).** Genera instaladores, recibos y un actualizador, pero asume GitHub Actions como CI, y el pipeline de CircleCI tiene puertas, `sccache`, build del motor con MSYS2 y bundle de ONNX Runtime. Su modelo está orientado a binarios sueltos en un directorio `bin`, así que el bundle multiarchivo y los ganchos propios (parar el daemon, `setup`) quedarían fuera o requerirían personalización. Su actualizador reejecuta el instalador generado, con lo que la lógica vuelve a los scripts. Se adoptan sus ideas (recibo, URLs `releases/latest/download`, `irm | iex` sobre assets del release), no la herramienta.
- **F. Gestores de paquetes nativos como vía principal.** Son complementarios, no sustitutos: exigen prerrequisitos (Homebrew, Scoop) que la audiencia sin toolchain no tiene y multiplican el mantenimiento (un manifiesto y una publicación por gestor). Se conserva el Cask; futuros canales entrarían como gestionados externamente.

### 4.3 Alternativas para el entorno del desarrollador

| Opción | Prerrequisito extra | Multiplataforma | Testeable | Valoración |
|---|---|---|---|---|
| **`xtask` (Rust)** | Ninguno (solo `cargo`) | ✅ Mismo código en los 4 targets | ✅ Tests de Rust | **Elegida**: ya adoptada (`release`, `cask`, `clean`, `build-engine`) |
| `just` | Instalar `just` | ⚠️ Recetas en shell, divergen por SO | ⚠️ | Descartada |
| `cargo-make` | Instalar `cargo-make` | ⚠️ DSL en TOML + scripts | ⚠️ | Descartada |
| Makefile / scripts PowerShell | `make` en Windows | ❌ | ❌ | Descartada |

### 4.4 Decisión

**Un programa por audiencia y un único motor de ciclo de vida, escrito en Rust:**

- **Usuario final** → el binario del producto (`ai-voice-interconnector self install | update | uninstall`, `cleanup`, `setup`, `doctor`). El motor vive en un crate propio.
- **Desarrollador** → `cargo xtask` (`doctor`, `bootstrap`, `build-engine`, `package`, `install`, `clean`). Delega en el binario del producto todo lo que toca la instalación o el estado del usuario, y no replica rutas.
- **Primer contacto** → dos bootstrap mínimos (POSIX sh para Linux y macOS, PowerShell para Windows), fuera de la raíz, publicados como assets del release.

No se funde todo en un único ejecutable para ambas audiencias porque sus contextos son disjuntos. El usuario final no tiene el repositorio ni `cargo`, y las tareas de desarrollo (compilar, empaquetar, cortar releases) solo tienen sentido con el código fuente: incluirlas en el binario distribuido lo inflaría y expondría comandos irrelevantes. La coherencia no la da un único ejecutable, sino un único motor y una misma semántica de verbos y flags ([§8.1](#81-reglas-transversales)).

## 5. Arquitectura

### 5.1 Principios

| Id | Principio |
|---|---|
| P1 | **Fuente única.** Cada regla (targets, rutas, formato del bundle, integración de PATH) se implementa una sola vez, en Rust. El bootstrap y `xtask` no conocen rutas de instalación ni de estado. |
| P2 | **El binario se gestiona a sí mismo.** Instalar, actualizar y desinstalar son subcomandos del producto. |
| P3 | **La versión que se instala ejecuta su propia lógica.** El bootstrap y la actualización delegan en el binario nuevo. |
| P4 | **Propiedad exclusiva.** Solo se borra lo que la aplicación posee en exclusiva; los recursos compartidos nunca se borran por defecto ([§6](#6-modelo-de-rutas-y-propiedad)). |
| P5 | **Per-user, sin privilegios.** Ninguna operación del usuario pide `sudo` ni UAC. |
| P6 | **Idempotencia y convergencia.** Repetir una operación deja el mismo estado final; ninguna falla por encontrar el trabajo ya hecho. |
| P7 | **Transaccional.** Una instalación o actualización fallida deja intacta la versión anterior. |
| P8 | **Sin efectos colaterales ocultos.** Lo que se modifica fuera del directorio de programa (PATH, perfiles de shell) se anuncia antes, se registra en el recibo y se revierte exactamente al desinstalar. |
| P9 | **Misma semántica en todas partes.** Mismos verbos, flags, confirmaciones y códigos de resultado en los cuatro targets y en ambas audiencias. |
| P10 | **Seguro por defecto.** HTTPS, verificación de integridad antes de ejecutar nada descargado y confirmación en operaciones destructivas. |
| P11 | **Idioma por capa.** Identificadores en inglés, incluidos los nombres de fichero y los targets de Cargo; comentarios, documentación, mensajes al usuario, textos de ayuda y descripciones de prueba en español; los contratos de máquina —claves JSON, `reason`, flags, variables de entorno y líneas de protocolo— no se traducen. La fuente canónica de esta política es `AGENTS.md` §0. |
| P12 | **Comentario autocontenido.** Un comentario de código, prueba, configuración o script explica la regla en lugar de remitir a una sección de esta especificación o a otro fichero para explicarse, con dos excepciones: los mensajes de error al usuario final sí remiten a la documentación, porque es su único punto de contacto, y el nombre del fichero que una función escribe o lee sí se nombra, porque ahí el nombre es parte de la operación. La fuente canónica de esta política es `AGENTS.md` §0. |

### 5.2 Componentes

```text
Usuario final                                       Desarrollador
─────────────                                       ─────────────
curl -fsSL …/install.sh | sh                        cargo xtask <tarea>
irm …/install.ps1 | iex                                     │
        │                                                   ▼
        ▼                                           crates/xtask
packaging/bootstrap/install.{sh,ps1}                  doctor · bootstrap · build-engine
  detecta el target, descarga, verifica,              package · install · clean
  extrae en staging y delega                                │
        │                                                   │ invocación por proceso
        └──────────────────┐        ┌───────────────────────┘ (sin enlazar el motor)
                           ▼        ▼
          ai-voice-interconnector  self install · self update · self uninstall
                                   cleanup · setup · doctor
                                        │
                                        ▼
                            crates/avi-lifecycle ──► crates/avi-store (rutas canónicas)
```

- **`crates/avi-lifecycle`** concentra el motor: detección de target, resolución y descarga de releases, verificación SHA-256, extracción, reemplazo transaccional, integración de PATH por SO, recibo, bloqueo, recuperación, plan de desinstalación y de limpieza. La lógica de `uninstall` y `cleanup` que hoy vive en el binario principal se traslada aquí.
- **`crates/avi-shared`** es la fuente única de rutas; **`crates/avi-store`** la reexporta para conservar la API de sus llamadores, y `avi-lifecycle` la consume a través de `avi-store`.
- **`crates/xtask`** delega en el binario del producto por invocación de proceso. Así no arrastra dependencias de red ni TLS (el tiempo de compilación de `xtask` precede a toda tarea de desarrollo) y no replica rutas.

### 5.3 Organización del repositorio

```text
AI-Voice-InterConnector/
├── packaging/
│   └── bootstrap/
│       ├── install.sh            # Linux + macOS (POSIX sh)
│       └── install.ps1           # Windows (PowerShell 5.1+ y 7+)
├── crates/
│   ├── avi-lifecycle/            # motor de ciclo de vida del usuario
│   ├── avi-process/              # flags de creación de procesos y borrado diferido (Windows)
│   ├── avi-shared/                # rutas canonicas (fuente unica; reexportada por avi-store)
│   └── avi-store/                # VoiceStore, SpeechStore, ModelStore (reexporta las rutas de avi-shared)
├── src/main.rs                   # cablea `self …`, `cleanup`, `setup`, `doctor`
├── tests/bootstrap/              # pruebas de los dos bootstrap (bats + Pester)
├── rust-toolchain.toml           # versión de Rust fijada para desarrollo y CI
└── .cargo/config.toml            # alias `cargo xtask` = `run -p xtask --`
```

**Desaparecido tras la implementación:** los cinco scripts de la raíz (`install-linux.sh`, `install-macos.sh`, `install-windows.ps1`, `upgrade-ai-voice-interconnector.sh` y `.ps1`), los pasos de staging repetidos por job en la CI (sustituidos por `cargo xtask package`) y las réplicas de rutas en `xtask clean` y en el instalador de Windows. `docs/SELF-HOSTED-INSTALL.md` queda retirado, con su contenido vigente absorbido según [§13](#13-papel-de-la-documentación).

**Único archivo nuevo en la raíz:** `rust-toolchain.toml`. Es la convención estándar de Rust y rustup solo la reconoce en la raíz; a cambio, instala y actualiza automáticamente la versión de Rust fijada para cualquier desarrollador.

### 5.4 Superficie de comandos

**Usuario final** (binario del producto):

| Comando | Propósito | Flags |
|---|---|---|
| `self install` | Instala el bundle del que forma parte el ejecutable, o repara la instalación si se ejecuta desde ella | `--no-setup`, `--no-modify-path`, `--force`, `--yes`, `--json` |
| `self update` | Actualiza a la última versión estable o a una concreta | `--check`, `--version X.Y.Z`, `--force`/`-f`, `--no-setup`, `--yes`, `--json` |
| `self uninstall` | Elimina el programa, la integración de PATH y (por defecto) el estado | `--keep-data`, `--dry-run`, `--yes`, `--json` |
| `setup` | Provisiona los modelos fijados por la versión | `--with-voice-cloning`, `--force-update`, `--yes`, `--json` |
| `cleanup` | Borra el estado por categorías, sin tocar el programa | `--model`, `--voices`, `--synthetic-speech`, `--all`, `--dry-run`, `--yes`, `--json` |
| `doctor` | Diagnóstico, incluido el estado del ciclo de vida | `--json` |

Uso típico:

```sh
curl -fsSL https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/releases/latest/download/install.sh | sh
ai-voice-interconnector self update --check     # ¿hay versión nueva?
ai-voice-interconnector self update             # actualizar
ai-voice-interconnector cleanup --model         # liberar espacio (reintentable con `setup`)
ai-voice-interconnector self uninstall          # desinstalar
```

```powershell
irm https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/releases/latest/download/install.ps1 | iex
```

**Desarrollador** (`cargo xtask`):

| Comando | Propósito | Flags |
|---|---|---|
| `doctor` | Verifica requisitos del host y deriva del entorno, sin modificar nada | `--json` |
| `bootstrap` | Instala o actualiza (convergente) el entorno de desarrollo | `--system`, `--models`, `--yes` |
| `build-engine` | Compila el motor TTS (existente) | `--self-test`, `--simd`, `-j` |
| `package` | Produce el bundle y el archivo idénticos a los de release | `--out <dir>`, `--no-compress`, `--expect-version X.Y.Z` |
| `install` | Instala el build local por el mismo camino que un release (canal `dev`) | `--no-setup` |
| `clean` | Limpia por capas | `--repo` (por defecto), `--app`, `--all`, `--dry-run`, `--yes` |

`release`, `cask`, `licenses`, `source-offer` y `changelog` se mantienen sin cambios (fuera de alcance).

## 6. Modelo de rutas y propiedad

Las rutas se resuelven con las convenciones de cada SO (XDG en Linux, `~/Library` en macOS, Known Folders en Windows) y se definen unicamente en `avi-shared`, que `avi-store` reexporta.

| Recurso | Propiedad | Linux (x86_64, arm64) | macOS arm64 | Windows x86_64 |
|---|---|---|---|---|
| Directorio de programa (bundle + recibo) | Exclusiva | `~/.local/opt/ai-voice-interconnector/` | `~/.local/opt/ai-voice-interconnector/` | `%LOCALAPPDATA%\Programs\ai-voice-interconnector\` |
| Comando en el PATH | Exclusiva (solo el enlace o la entrada) | Enlace `~/.local/bin/ai-voice-interconnector` | Ídem | Entrada en el valor `Path` de `HKCU\Environment` |
| Bloque en perfiles de shell | Exclusiva (solo el bloque delimitado) | [§8.3.1](#831-integración-de-path) | Ídem | — |
| Datos de usuario y estado (voces, habla sintetizada, configuración, `daemon.pid`, logs) | Exclusiva | `$XDG_DATA_HOME/ai-voice-interconnector/` | `~/Library/Application Support/ai-voice-interconnector/` | `%LOCALAPPDATA%\ai-voice-interconnector\data\` |
| Modelos (caché regenerable) | Exclusiva | `$XDG_CACHE_HOME/ai-voice-interconnector/models/` | `~/Library/Caches/ai-voice-interconnector/models/` | `%LOCALAPPDATA%\ai-voice-interconnector\cache\models\` |
| Staging y aparcados | Exclusiva | `~/.local/opt/.ai-voice-interconnector-staging-*` y `<programa>/.old-*` | Ídem | `%LOCALAPPDATA%\Programs\.ai-voice-interconnector-staging-*` y `<programa>\.old-*` |
| Bloqueo de ciclo de vida | Exclusiva | `~/.local/opt/.ai-voice-interconnector.lock` | Ídem | `%LOCALAPPDATA%\Programs\.ai-voice-interconnector.lock` |
| Temporales de ejecución | Exclusiva por prefijo | `$TMPDIR` con prefijo `avi-`/`avi_` | Ídem | `%TEMP%` con los mismos prefijos |
| Caché HF elegida por el usuario (`HF_HUB_CACHE` o `HF_HOME`) | **Compartida** | La indicada | Ídem | Ídem |

**Reubicación.** Cada raíz admite una variable de entorno de reubicación. Además de permitir ubicaciones personalizadas, es lo que hace posibles las pruebas aisladas en CI, porque en Windows las Known Folders ignoran `LOCALAPPDATA`. Los valores efectivos se registran en el recibo, así que la actualización y la desinstalación operan sobre las mismas ubicaciones aunque la variable ya no esté definida.

| Variable | Raíz |
|---|---|
| `AVI_INSTALL_DIR` | Directorio de programa |
| `AVI_BIN_DIR` | Directorio del enlace (Unix) |
| `AVI_DATA_DIR` | Datos de usuario y estado |
| `AVI_CACHE_DIR` | Modelos |
| `HF_HUB_CACHE` / `HF_HOME` | Usa una caché HF compartida elegida por el usuario; su propiedad pasa a ser compartida |
| `AVI_DOWNLOAD_BASE_URL` | Base de descarga de releases (espejos, redes aisladas, pruebas) |

**Reglas de propiedad:**

- **R1.** Las operaciones destructivas solo actúan dentro de raíces de propiedad exclusiva. Una ruta fuera de ellas es un error interno y nunca se borra.
- **R2.** El directorio de programa solo se borra si contiene el recibo o el ejecutable de la aplicación. Nunca se borra `$HOME`, la raíz de una unidad ni un directorio del sistema, aunque una variable de reubicación o un recibo manipulado apunten allí.
- **R3.** En una raíz compartida solo se borran entradas atribuibles a la aplicación (los repos fijados, `models--<org>--<nombre>`, y sus bloqueos). Nunca se borran subdirectorios globales de la caché (`xet`, `.locks` completo).
- **R4.** Todo recurso nuevo que la aplicación empiece a crear se añade a esta tabla y a los planes de limpieza y desinstalación en el mismo cambio.

## 7. Recibo de instalación y canales

### 7.1 Recibo

El recibo `install-receipt.json` vive en el directorio de programa y se escribe de forma atómica (archivo temporal + renombrado).

```json
{
  "schema_version": 1,
  "app": "ai-voice-interconnector",
  "version": "0.24.0",
  "target": "x86_64-unknown-linux-gnu",
  "channel": "script",
  "install_dir": "/home/ana/.local/opt/ai-voice-interconnector",
  "files": [
    "ai-voice-interconnector",
    "libonnxruntime.so",
    "vendor/qwen3-tts/qwen_tts",
    "LICENSE",
    "THIRD-PARTY-LICENSES.md",
    "SOURCE-OFFER.md",
    "README.md"
  ],
  "path_integration": {
    "modify_path": true,
    "symlink": "/home/ana/.local/bin/ai-voice-interconnector",
    "profile_blocks": ["/home/ana/.bashrc", "/home/ana/.profile"],
    "registry_entry": null
  },
  "roots": {
    "data_dir": "/home/ana/.local/share/ai-voice-interconnector",
    "cache_dir": "/home/ana/.cache/ai-voice-interconnector"
  },
  "installed_at": "2026-09-25T18:00:00Z",
  "source": "https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/releases/download/v0.24.0/ai-voice-interconnector-0.24.0-x86_64-linux.tar.gz"
}
```

- En Windows, `path_integration.registry_entry` guarda la entrada añadida al PATH de usuario, y `symlink`/`profile_blocks` son `null`.
- Los campos desconocidos se ignoran, para que un binario antiguo pueda leer recibos nuevos. Un `schema_version` mayor que el soportado aborta la operación con un mensaje que pide actualizar.

### 7.2 Canales

| Canal | Cómo se origina | `self install` | `self update` | `self uninstall` | `cleanup` |
|---|---|---|---|---|---|
| `script` | Bootstrap, o `self install` desde un archivo extraído a mano | Instala o repara | ✅ | ✅ | ✅ |
| `dev` | `cargo xtask install` | Repara | `externally_managed` → `cargo xtask install` (`--force` pasa al canal `script` con el release publicado) | ✅ | ✅ |
| `homebrew` | Cask de Homebrew (detectado, sin recibo) | `externally_managed` | `externally_managed` → `brew upgrade --cask ai-voice-interconnector` | `externally_managed` → `brew uninstall --cask --zap ai-voice-interconnector` | ✅ |
| `unmanaged` | Binario ejecutado fuera de una instalación (bundle sin instalar, `target/`) | Instala si el bundle es válido; si no, `bundle_invalid` | Actúa sobre la instalación registrada, si existe; si no, `not_installed` | Actúa sobre la instalación registrada, si existe | ✅ |

**Detección del canal:** `homebrew` si el ejecutable resuelto está bajo el prefijo de Homebrew (`Caskroom`); `script` o `dev` según el recibo; `unmanaged` en cualquier otro caso. `self update` y `self uninstall` siempre actúan sobre la **instalación registrada** (el recibo en su ubicación), independientemente de qué copia del binario ejecute el comando. Si conviven dos instalaciones (Cask y `script`), `doctor` lo informa junto con cuál tiene precedencia en el PATH.

## 8. Entorno del usuario

### 8.1 Reglas transversales

**Confirmación.** "Hay terminal" significa que stdin es una TTY. El bootstrap redirige stdin a la terminal de control cuando existe ([§8.2](#82-bootstrap-primera-instalación)), de modo que `curl | sh` sigue siendo interactivo.

| Tipo de operación | Con terminal | Sin terminal |
|---|---|---|
| No destructiva (`self install`, `self update`, `setup`) | Resumen de cambios y confirmación `[S/n]`; `--yes` la omite | Procede sin preguntar |
| Destructiva (`self uninstall`, `cleanup`, `setup --force-update`, degradar de versión, `xtask clean`) | Lista de rutas con tamaños y confirmación `[s/N]`; `--yes` la omite | Exige `--yes`; sin él termina con `confirmation_required` y no borra nada |

**Simulación.** Toda operación destructiva acepta `--dry-run`: imprime el plan (rutas, tamaños, cambios de PATH) sin modificar el disco.

**Salida.** El progreso y los avisos van a stderr en español; el resultado, a stdout. Con `--json`, stdout contiene únicamente el sobre estándar del contrato de la CLI, con `status`, `reason` y los campos de cada operación.

**Resultados.** Los `reason` son contrato de máquina y se mantienen en inglés. Los códigos de salida numéricos se asignan en el contrato de la CLI respetando las familias existentes (0 éxito, 2 error de uso).

| `reason` | Operaciones | Significado | Resultado |
|---|---|---|---|
| `already_up_to_date` | `self update` | Ya se está en la versión objetivo | Éxito |
| `not_installed` | `self update`, `self uninstall` | No hay instalación registrada | `uninstall`: éxito (idempotente); `update`: error, con el one-liner |
| `externally_managed` | `self *` | La copia la gestiona otra herramienta | Error, con el comando correcto |
| `unsupported_platform` | Bootstrap, `self install` | Target no soportado | Error |
| `binary_incompatible` | Bootstrap, `self update` | El binario descargado no arranca en este sistema | Error, con diagnóstico |
| `network_error` | Bootstrap, `self update`, `setup` | Fallo de descarga tras reintentos | Error |
| `checksum_mismatch` | Bootstrap, `self update` | El hash no coincide o falta en `SHA256SUMS.txt` | Error; nada modificado |
| `bundle_invalid` | `self install` | Falta un archivo obligatorio del bundle | Error; nada modificado |
| `daemon_stop_failed` | `self *`, `cleanup` | No se pudo detener el daemon | Error; nada modificado |
| `path_conflict` | `self install` | En la ruta del enlace hay un archivo ajeno | Error, salvo `--force` |
| `lifecycle_locked` | Todas | Hay otra operación de ciclo de vida en curso | Error |
| `sudo_not_supported` | `self install`, `self uninstall`, `cleanup` | Unix: la ejecución vino de `sudo` | Error genérico (1); nada modificado |
| `confirmation_required` | Destructivas | Sin terminal y sin `--yes` | Error de uso (2) |
| `usage_error` | `cleanup` | Sin categoría | Error de uso (2) |
| `setup_failed` | `self install`, `self update` | Programa instalado, pero la provisión de modelos falló | Éxito parcial (código propio), reintentable con `setup` |
| `rolled_back` | `self install`, `self update` | Fallo durante el reemplazo; versión anterior restaurada | Error |
| `removal_scheduled` | `self uninstall` | Windows: el directorio se borra al terminar el proceso | Éxito |
| `program_dir_kept` | `self uninstall` | Todas: el resto se completó, pero el directorio de programa no se pudo borrar ni programar su borrado | Error con código propio (22) |

**Bloqueo.** Las operaciones de ciclo de vida toman un bloqueo exclusivo de SO (`flock` o `LockFileEx`) sobre el archivo de bloqueo de [§6](#6-modelo-de-rutas-y-propiedad). El SO lo libera aunque el proceso muera. Mientras está tomado, los comandos que lanzan el daemon automáticamente no lo lanzan y terminan con `lifecycle_locked`, para que no arranque un daemon de la versión saliente en mitad de una actualización.

**Recuperación.** Al empezar, toda operación de ciclo de vida (y `doctor`, en modo informe):

1. Si existe un diario de transacción pendiente (`<programa>/.transaction.json`), completa el rollback restaurando lo aparcado, o el commit si la transacción ya estaba confirmada.
2. Borra aparcados `.old-*` que ya no estén en uso, stagings huérfanos y temporales propios sin proceso vivo.

**Privilegios.** Ninguna operación pide elevación. En Unix, si se detecta ejecución vía `sudo` (uid 0 con `SUDO_USER` definido), se aborta: la instalación es per-user y con `sudo` acabaría en el perfil de root. Root sin `sudo` (contenedores) sí se permite. En Windows, si el proceso está elevado, se avisa de que la instalación se hará en el perfil de la cuenta que ejecuta.

**Red.** Solo HTTPS con TLS ≥ 1.2, reintentos acotados con espera creciente, respeto de las variables de proxy estándar (`HTTPS_PROXY`, `NO_PROXY`) y del proxy del sistema en Windows.

### 8.2 Bootstrap (primera instalación)

El bootstrap es el único código que corre antes de que exista el binario. Su responsabilidad se limita a **obtener, verificar y ejecutar** el bundle correcto. No integra el PATH, no provisiona modelos, no consulta versiones instaladas ni conoce rutas de estado: todo eso lo hace `self install`. Cada script ocupa del orden de un centenar de líneas.

**Publicación.** `install.sh` e `install.ps1` se publican como **assets de cada release**, con la versión estampada por el pipeline al publicar, y se incluyen en `SHA256SUMS.txt`. Así el bootstrap y el bundle de un release son siempre coherentes y la instalación no depende del estado de `main`.

| Uso | URL |
|---|---|
| Última versión | `https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/releases/latest/download/install.sh` (o `install.ps1`) |
| Versión fijada | `https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/releases/download/vX.Y.Z/install.sh` |
| Alternativa inspeccionable | Descargar el script, leerlo y ejecutarlo como archivo |

**Entradas.** Bajo `irm | iex` no se pueden pasar parámetros, así que toda opción tiene una variable de entorno equivalente.

| `install.sh` | `install.ps1` (como archivo) | Variable de entorno | Efecto |
|---|---|---|---|
| `--version X.Y.Z` | `-Version X.Y.Z` | `AVI_VERSION` | Instala esa versión en lugar de la estampada |
| `--no-setup` | `-NoSetup` | `AVI_NO_SETUP=1` | No provisiona modelos |
| `--no-modify-path` | `-NoModifyPath` | `AVI_NO_MODIFY_PATH=1` | No modifica el PATH persistente |
| `--yes` | `-Yes` | `AVI_YES=1` | Acepta la confirmación (modo desatendido) |
| — | — | `AVI_DOWNLOAD_BASE_URL` | Base de descarga alternativa |

Ejemplos: `curl -fsSL <url> | sh -s -- --no-setup` y `$env:AVI_NO_SETUP = "1"; irm <url> | iex`.

**Flujo:**

1. **Detectar el target** según [§3](#3-targets-soportados). Si no está soportado → `unsupported_platform`, antes de descargar nada.
2. **Unix: rechazar `sudo`** ([§8.1](#81-reglas-transversales), privilegios).
3. **Resolver la versión**, por este orden: opción o variable, versión estampada, y (solo si el script se ejecuta sin estampar, desde el repositorio) la última estable, obtenida siguiendo la redirección de `https://github.com/<repo>/releases/latest`. No se usa la API REST de GitHub, así que no hay límite de peticiones ni hace falta parsear JSON.
4. **Crear el staging** como hermano del directorio de programa (mismo volumen), con permisos solo del usuario.
5. **Descargar** el archivo del target y `SHA256SUMS.txt` por HTTPS.
6. **Verificar**: localizar en `SHA256SUMS.txt` la línea cuyo nombre coincide exactamente con el del archivo (comparación de cadenas, no expresión regular) y comparar el hash en minúsculas. Si el hash no coincide o falta la línea → `checksum_mismatch` y se borra el staging.
7. **Extraer** en el staging.
8. **Comprobar el arranque** con `<staging>/ai-voice-interconnector --version`. Si falla → `binary_incompatible` con diagnóstico ([§3](#3-targets-soportados)); la instalación existente no se toca.
9. **Delegar**: `<staging>/ai-voice-interconnector self install`, pasando las opciones. En Unix, si stdin no es una terminal y `/dev/tty` está disponible, stdin se redirige desde `/dev/tty`.
10. **Terminar**: borrar el staging, tanto si hubo éxito como error, y propagar el código de salida de `self install`.

**Requisitos de `install.sh`:** POSIX sh (dash, bash, busybox sh, zsh en modo sh); `curl` o, en su defecto, `wget`; `sha256sum` o `shasum -a 256`; `tar` y `mktemp`.

**Requisitos de `install.ps1`:** compatible con PowerShell 5.1 y 7+.

- Todo el script se ejecuta dentro de un bloque con ámbito propio: `$ErrorActionPreference`, `$ProgressPreference` y las funciones no se filtran a la sesión del usuario.
- Bajo `irm | iex` nunca usa `exit` (un error no cierra la consola del usuario); ejecutado como archivo, termina con código ≠ 0 en caso de error.
- Desactiva la barra de progreso durante las descargas (en PowerShell 5.1 ralentiza mucho `Invoke-WebRequest`) y habilita TLS 1.2 si falta.
- Usa `Invoke-WebRequest -UseBasicParsing`, `Get-FileHash` y `Expand-Archive`.
- Su único efecto sobre la sesión es añadir el directorio de programa al PATH de esa sesión, para que el comando funcione sin abrir otra terminal.
- El archivo es ASCII puro y sin BOM. Bajo `irm | iex` el cuerpo llega como texto sin `charset` (ISO-8859-1 en PowerShell 5.1, UTF-8 que conserva `U+FEFF` en 7): un BOM impide el parseo y un byte no ASCII corrompe los mensajes. Las tildes de los mensajes se componen con variables `[char]` definidas al inicio del bloque de ámbito propio.

### 8.3 `self install`

**Modo, según dónde está el ejecutable que se invoca:**

- **Fuera del directorio de programa** (bundle en staging o extraído a mano): instalación, o reemplazo si ya hay una versión.
- **Dentro del directorio de programa**: reparación. Se vuelven a aplicar la integración de PATH, los permisos, la limpieza de cuarentena y el recibo, sin copiar archivos.
- **Ejecutable sin bundle alrededor** (por ejemplo, `target/release`): `bundle_invalid`, con la indicación de usar `cargo xtask install`.

**Flujo de instalación:**

1. **Recuperación y bloqueo** ([§8.1](#81-reglas-transversales)).
2. **Validar el bundle** contra la lista de archivos de su target, fijada en compilación. Es la misma lista que usa `cargo xtask package`, de modo que empaquetado e instalación no pueden divergir. Si falta un archivo → `bundle_invalid`.
3. **Detectar la instalación previa**: registrada (recibo) o ajena (Cask en el PATH, que solo genera un aviso de coexistencia y precedencia).
4. **Resumen y confirmación.** Ejemplo:

   ```text
   Se instalará ai-voice-interconnector 0.24.0 (x86_64-unknown-linux-gnu)
     Programa:  ~/.local/opt/ai-voice-interconnector   (reemplaza 0.23.1)
     Comando:   ~/.local/bin/ai-voice-interconnector
     PATH:      se añadirá ~/.local/bin en ~/.bashrc y ~/.profile
     Modelos:   se descargarán unos 3.3 GB en ~/.cache/ai-voice-interconnector/models
   ¿Continuar? [S/n]
   ```

   Con la integración del `PATH` ya hecha (entrada del registro en Windows, enlace del comando en Unix), la línea del `PATH` no anuncia cambios:

   ```text
     PATH:      ~/.local/bin ya está en el PATH; no se modifica
   ```

   En Windows dice «ya está en el PATH del usuario; no se modifica».

   Instalar una versión menor que la instalada es una degradación y se confirma como operación destructiva.
5. **Parar el daemon** si está activo, incluido el proceso residente del motor, con el protocolo estable entre versiones (`daemon.pid` + `POST /shutdown` + árbol de procesos). Si no se detiene → `daemon_stop_failed`, sin modificar nada.
6. **Reemplazo transaccional**, con el mismo algoritmo en los cuatro targets:
   1. **Aparcar** el contenido actual del directorio de programa en `<programa>/.old-<txid>/` por renombrado, registrándolo en el diario. En Windows, renombrar un ejecutable en uso está permitido; borrarlo no.
   2. **Colocar** el bundle nuevo. Los archivos se mueven desde el staging (renombrado en el mismo volumen), salvo el ejecutable que está corriendo, que se copia.
   3. **Ajustar permisos**: en Unix, 0755 para ejecutables y 0644 para el resto.
   4. **Confirmar**: marcar el diario como confirmado y borrar `.old-<txid>/`. Lo que no se pueda borrar por estar en uso queda para borrado diferido ([§8.4](#84-self-update)).
   5. **Revertir** si falla el paso 2 o el 3: retirar lo colocado, restaurar lo aparcado, borrar el diario → `rolled_back`.
7. **macOS**: eliminar `com.apple.quarantine` de forma recursiva en todo el directorio de programa, no solo en el ejecutable, porque el motor y la librería de ONNX Runtime también se ejecutan o cargan. Si el archivo se descargó por navegador y se extrajo con el Finder, todos los archivos heredan la cuarentena.
8. **Integración de PATH** ([§8.3.1](#831-integración-de-path)).
9. **Windows**: si el PATH de máquina contiene una entrada de una instalación per-machine antigua, se avisa y se muestra el comando exacto para quitarla desde una PowerShell de administrador. HKLM nunca se modifica.
10. **Escribir el recibo** (de forma atómica) y liberar el bloqueo.
11. **Provisionar como `setup`** en el mismo proceso (misma función de provisión: descarga según el estado real del almacén), salvo `--no-setup` ([§8.7](#87-setup-en-el-ciclo-de-vida)). Si falla → `setup_failed`: el programa queda instalado y basta reintentar con `setup`.
12. **Resumen final**: versión, rutas, estado del PATH (con "abre una terminal nueva" cuando corresponda) y estado de los modelos.

#### 8.3.1 Integración de PATH

**Linux y macOS**

- El enlace simbólico `~/.local/bin/ai-voice-interconnector` apunta al ejecutable del directorio de programa. Se crea de forma atómica (enlace temporal + renombrado). Si en esa ruta hay algo que no es un enlace propio → `path_conflict`, salvo `--force`.
- Si `~/.local/bin` no está en el PATH y no se pasó `--no-modify-path`, se añade un bloque delimitado e idempotente a los archivos de arranque de los shells detectados (el de `$SHELL` y los que ya tengan archivo de arranque):

  | Shell | Archivo |
  |---|---|
  | sh / bash | `~/.profile` y, si existe, `~/.bashrc` |
  | zsh | `${ZDOTDIR:-$HOME}/.zshrc` |
  | fish | `~/.config/fish/conf.d/ai-voice-interconnector.fish` (archivo propio) |

  ```sh
  # >>> ai-voice-interconnector >>>
  case ":${PATH}:" in *":$HOME/.local/bin:"*) ;; *) export PATH="$HOME/.local/bin:$PATH" ;; esac
  # <<< ai-voice-interconnector <<<
  ```

- El proceso que ejecutó `curl | sh` no puede cambiar el PATH de su shell padre. El resumen final indica la línea exacta para la sesión actual o pide abrir una terminal nueva.

**Windows**

- La entrada del directorio de programa se añade al valor `Path` de `HKCU\Environment`:
  - el valor se lee **sin expandir**;
  - la comparación es canónica: sin distinguir mayúsculas, ignorando separadores finales y considerando también la forma expandida de cada entrada;
  - la entrada se añade al final solo si falta;
  - se escribe **conservando el tipo** del valor (`REG_EXPAND_SZ` si no existía) y las entradas `%VAR%` intactas;
  - se difunde `WM_SETTINGCHANGE` ("Environment") con tiempo límite.
- Con `--no-modify-path` no se toca el registro, y el comando se invoca por su ruta completa.

Todo lo modificado queda en el recibo, y `self uninstall` lo revierte exactamente, sin reescribir nada más.

### 8.4 `self update`

```text
ai-voice-interconnector self update [--check] [--version X.Y.Z] [--force] [--no-setup] [--yes] [--json]
```

**Flujo:**

1. **Recuperación y bloqueo.**
2. **Leer el recibo y el canal** ([§7.2](#72-canales)). Si el canal es `homebrew` o `dev` → `externally_managed` con el comando correcto; si no hay instalación → `not_installed` con el one-liner.
3. **Resolver la versión objetivo**: `--version`, o la última estable obtenida por la redirección de `releases/latest`, sin API. La API REST solo se usa como respaldo. Se respeta `AVI_DOWNLOAD_BASE_URL`.
4. **Comparar las versiones** semánticamente:
   - iguales → `already_up_to_date`, sin descargar nada (`--force` reinstala la misma versión);
   - objetivo menor → solo con `--version` explícito, y se trata como operación destructiva por la compatibilidad de datos.
5. **`--check`**: informa la transición (`0.23.1 → 0.24.0`, o que ya se está en la última) y termina sin cambios. En JSON: `current`, `latest`, `update_available` y `channel`.
6. **Resumen y confirmación.**
7. **Preparar el bundle nuevo**: descargar el archivo y `SHA256SUMS.txt` en un staging hermano, verificar el SHA-256 (y la firma, cuando exista, [§11](#11-seguridad)), extraer, y comprobar que el binario nuevo arranca y que `--version` coincide con la versión objetivo.
8. **Parar el daemon con el binario actual**, que conoce su propio protocolo y la ruta de su `daemon.pid`. Se anota si estaba en ejecución.
9. **Traspaso**: ejecutar `<staging>/ai-voice-interconnector self install --yes` heredando la consola, con las preferencias registradas en el recibo (por ejemplo, `--no-modify-path` si se usó al instalar) y `--no-setup` si se pidió. Se espera a que termine y se propaga su resultado.
10. **Limpiar**: borrar el staging. Lo aparcado que siga en uso (en Windows, el ejecutable del proceso que actualiza) se elimina con un proceso auxiliar desacoplado que espera a que ese proceso termine y borra con reintentos acotados. Si el auxiliar no llega a hacerlo, la recuperación de la siguiente operación lo completa.
11. **Resultado**: `0.23.1 → 0.24.0`. Si el daemon estaba activo, el resumen indica cómo reiniciarlo.

**Garantías.** Un fallo antes del traspaso deja todo intacto. Un fallo durante el traspaso se revierte con la transacción del `self install` nuevo. Una interrupción en cualquier punto se recupera en la siguiente operación de ciclo de vida.

**Modelos.** El `setup` de la versión nueva provisiona los modelos cuyo pin cambió y poda las revisiones propias obsoletas ([§8.7](#87-setup-en-el-ciclo-de-vida)).

### 8.5 `self uninstall`

```text
ai-voice-interconnector self uninstall [--keep-data] [--dry-run] [--yes] [--json]
```

Actúa sobre la instalación registrada, sea cual sea la copia del binario que lo invoque.

1. **Recuperación y bloqueo.** Si el canal es `homebrew` → `externally_managed` con `brew uninstall --cask --zap ai-voice-interconnector`; para el estado, se sugiere `cleanup --all`.
2. **Plan**: rutas con tamaños del directorio de programa, la integración de PATH (enlace, bloques de perfil, entrada de registro) y el estado (salvo con `--keep-data`). También se listan, marcados como "no se tocará", los recursos compartidos.
3. **`--dry-run`**: imprime el plan y termina.
4. **Confirmación destructiva.**
5. **Parar el daemon.** Si falla → `daemon_stop_failed`, sin borrar nada.
6. **Borrar el estado** (salvo con `--keep-data`):
   - **Sin `--keep-data`: la raíz de datos entera**, y no el plan de `cleanup --all`. El motivo es que al desinstalar **desaparece el programa, y con él las voces de fábrica** (`cleanup --all` las protege porque van embebidas en el binario y el programa sigue instalado, así que `setup` las vuelve a materializar). Dejarlas sería residuo dentro de una raíz de propiedad exclusiva, que es lo que prohíbe el criterio 17, de modo que el destino del estado es la raíz completa —que R1 permite porque es exclusiva— en vez de la lista de categorías. Los modelos siguen el plan de `--model`, con sus reglas de propiedad.
   - **Con `--keep-data`: el plan de `cleanup --all` filtrado** —fuera modelos, voces y habla, dentro configuración, logs y estado del daemon—, que es lo coherente con lo que el usuario ha pedido. El directorio de programa se borra igual en los dos casos: la bandera conserva el estado, no el programa.
7. **Revertir el PATH** según el recibo:
   - el enlace, solo si apunta al directorio de programa;
   - los bloques delimitados de los perfiles;
    - la entrada del registro, con comparación canónica, conservando el tipo del valor y difundiendo `WM_SETTINGCHANGE`.
8. **Borrar el directorio de programa** (con la regla R2). En Unix, directamente. En Windows, si el ejecutable en uso está dentro, un proceso auxiliar desacoplado espera a que termine y borra el directorio con reintentos acotados → `removal_scheduled`; el auxiliar confirma su arranque con una marca y, si muere sin darla, el borrado no se da por programado. Si el directorio no se puede borrar ni programar → `program_dir_kept`, sin borrado parcial.
9. **Borrar el archivo de bloqueo.**

**Idempotencia:** sin instalación ni estado, termina con éxito y `not_installed`. **Residuo:** cero dentro de las raíces de propiedad exclusiva. Lo compartido que no se borra se informa explícitamente.

### 8.6 `cleanup`

```text
ai-voice-interconnector cleanup (--model | --voices | --synthetic-speech | --all) [--dry-run] [--yes] [--json]
```

| Flag | Borra | Nunca borra |
|---|---|---|
| `--model` | Modelos provisionados: todas las revisiones de los repos propios. Con la caché exclusiva de la aplicación, el directorio se borra entero | Modelos de otras herramientas en una caché compartida, si el usuario eligió `HF_HUB_CACHE` o `HF_HOME` (regla R3) |
| `--voices` | Voces de usuario (clonadas o importadas) y su registro | Voces de fábrica (van embebidas en el binario) |
| `--synthetic-speech` | Audio sintetizado guardado | — |
| `--all` | Todo lo anterior, más configuración, logs y `daemon.pid` | Programa e integración de PATH (eso es `self uninstall`) |

- Sin categoría → `usage_error`, sin borrar nada.
- Antes de borrar recursos que el daemon usa, se detiene el daemon.
- Cualquier invocación barre además los temporales propios huérfanos, los stagings huérfanos y los aparcados `.old-*`.
- Tras `--model`, la aplicación queda reintentable: `setup` descarga de nuevo lo necesario.

### 8.7 `setup` en el ciclo de vida

Solo se especifican los aspectos de `setup` que afectan al ciclo de vida:

- Provisiona el conjunto de modelos fijado por la versión (pines), según la **selección persistida**: el conjunto base más los opcionales, como `--with-voice-cloning`. La selección se guarda en la configuración, para que las actualizaciones provisionen el mismo conjunto.
- Antes de descargar, calcula el tamaño pendiente. Con terminal pide confirmación `[S/n]`; la omite con `--yes` o cuando la invoca `self install`/`self update` después de su propio resumen. Sin terminal, procede.
- Tras provisionar con éxito, **poda las revisiones obsoletas** de los repos propios, es decir, las que ya no corresponden al pin vigente.
- Es idempotente: si todo está provisionado, no descarga nada.
- Las **migraciones de estado** entre versiones son responsabilidad del `setup` de la versión nueva, idempotentes y solo hacia delante.

### 8.8 `doctor`

`doctor` añade una sección de ciclo de vida (claves JSON entre paréntesis):

| Dato | Contenido |
|---|---|
| Versión y target (`version`, `target`) | Los del binario en ejecución |
| Canal (`channel`) | `script`, `dev`, `homebrew` o `unmanaged` |
| Instalación (`install`) | Directorio de programa y estado del recibo: válido o ausente |
| PATH (`path`) | Si el comando resuelve a esta instalación, si hay duplicados en el PATH (Cask + `script`) y el estado del enlace o de la entrada de registro |
| Pendientes (`pending`) | Diario de transacción, aparcados y stagings huérfanos |
| Modelos (`models`) | Provisionados, faltantes (evaluados contra la selección persistida: el modelo Base opt-in no seleccionado no cuenta) y revisiones obsoletas, con tamaños |

## 9. Entorno del desarrollador

### 9.1 Requisitos por target

La tabla de requisitos y las versiones fijadas (Rust, ONNX Runtime, paquetes de MSYS2) se declaran **una sola vez, en código de `xtask`**, y de ahí las consumen `cargo xtask doctor`, `cargo xtask bootstrap` y la CI. La guía de build remite a `cargo xtask doctor` en lugar de duplicar la lista.

| Requisito | Windows x86_64 | Linux x86_64 / arm64 | macOS arm64 | Lo provee |
|---|---|---|---|---|
| Rust (versión fijada) | rustup | rustup | rustup | `rust-toolchain.toml` (rustup la instala sola) |
| Compilador C/C++ del workspace (CTranslate2) | Visual Studio Build Tools (C++) | gcc/g++ | Xcode Command Line Tools | Sistema |
| CMake ≥ 3.20 | winget | Gestor de la distribución | Homebrew | Sistema |
| libclang (bindgen de `ct2rs`) | LLVM | `libclang-dev` o equivalente | Xcode Command Line Tools | Sistema |
| pkg-config y cabeceras de ALSA | — | `pkg-config`, `libasound2-dev` o equivalentes | — | Sistema |
| Toolchain del motor TTS | MSYS2 UCRT64 (gcc, OpenBLAS y `mingw32-make` fijados) | make, gcc y OpenBLAS | make, clang y Accelerate (Xcode CLT) | Sistema (MSYS2: su `pacman`) |
| ONNX Runtime (runtime de STT, versión fijada) | Descarga verificada | Descarga verificada | Descarga verificada | `cargo xtask bootstrap` |
| sccache (opcional) | ✅ | ✅ | ✅ | `cargo install` o gestor |

### 9.2 `cargo xtask doctor`

Solo lectura. Para cada requisito informa si está correcto, falta o tiene una versión distinta de la fijada. Cuando algo falta, da el comando exacto de instalación para el gestor detectado (apt, dnf, pacman o zypper; `xcode-select` o `brew`; winget o el `pacman` de MSYS2). Informa además de la deriva del entorno: motor TTS desactualizado respecto de sus fuentes, versión de ONNX Runtime distinta de la fijada y modelos no provisionados. Termina con éxito solo si están todos los requisitos obligatorios. Admite `--json`.

### 9.3 `cargo xtask bootstrap` (instalar y actualizar el entorno)

Es **convergente**: la misma orden prepara el entorno la primera vez y lo pone al día después de un `git pull`. Si nada cambió, no hace trabajo.

1. Ejecuta `doctor`. Para los requisitos del sistema que falten, imprime los comandos de instalación. Con `--system`, los ejecuta, y es la única operación de toda la especificación que puede pedir `sudo` o UAC, y solo por petición explícita.
2. Asegura los componentes de Rust necesarios (clippy, rustfmt y, si se usa cobertura, las herramientas LLVM). La versión de Rust ya la aplica `rust-toolchain.toml`.
3. Descarga ONNX Runtime en la versión fijada para el target del host, verifica su checksum fijado, lo deja en `ort-bundle/` y hace que lo encuentren los binarios de `target/`.
4. Compila el motor TTS (`build-engine --self-test`) si falta o está desactualizado.
5. Con `--models`, provisiona los modelos ejecutando `setup` con el build local.
6. Imprime un resumen del estado alcanzado.

### 9.4 `cargo xtask package`

Produce el bundle y el archivo de release del target del host. No hay compilación cruzada: cada target se empaqueta en su runner nativo.

1. Compila `--release --features full`.
2. Monta el bundle con la lista canónica de archivos del target (la misma que valida `self install`): ejecutable, motor TTS, ONNX Runtime (más las DLL del runtime de VC++ en Windows) y documentos de licencia.
3. Valida la versión contra `--expect-version` (en CI, el tag).
4. Hace una prueba de humo (`--version` y `voice list`).
5. Comprime de forma determinista (`.tar.gz` o `.zip`) con el nombre convencional.

Los cuatro jobs de build de CI invocan este comando en lugar de repetir cada uno su propio staging. El job de publicación estampa la versión en los bootstrap, los añade como assets y calcula `SHA256SUMS.txt` sobre todos los assets.

### 9.5 `cargo xtask install`

Ejecuta `package --no-compress` en un staging y después `<staging>/ai-voice-interconnector self install --channel dev` (`--channel` es una opción oculta). El build local queda instalado **por el mismo camino que un release**, de modo que cada instalación de desarrollo ejercita el instalador real.

- **Volver al release publicado**: `ai-voice-interconnector self update --force`.
- **Desinstalar**: `ai-voice-interconnector self uninstall`, o `cargo run -- self uninstall` si la copia instalada está rota.

### 9.6 `cargo xtask clean`

| Capa | Flag | Contenido | Mecanismo |
|---|---|---|---|
| Repositorio | `--repo` (por defecto) | `target/`, `ort-bundle/`, salidas de `package`, binario y objetos del motor, cobertura y pesos locales heredados bajo `vendor/qwen3-tts` | `xtask` |
| Aplicación | `--app` | Instalación y estado del usuario | Delegado en `self uninstall --yes` (o `cleanup --all --yes` si el canal es `homebrew`), ejecutado con el binario del repositorio |
| Ambas | `--all` | Aplicación y después repositorio | Ídem |
| Global compartida | — | `~/.cargo`, caché de sccache, paquetes del sistema, MSYS2 | Nunca; solo se informa |

- Detiene primero los daemons lanzados desde `target/`.
- `--prune` modera la capa repo a la poda fina (cachés, cruces ajenos y PDBs viejos) y conserva la compilación vigente; el defecto sin flag no cambia. Ver `docs/BRANCHING.md`.
- Nunca borra código fuente versionado.
- Aplica las reglas de confirmación de [§8.1](#81-reglas-transversales): `--dry-run`, y `--yes` obligatorio sin terminal.
- En Windows, el `xtask.exe` en ejecución se borra con la misma implementación de borrado diferido que el producto (el crate `avi-process`).

### 9.7 Mapa de operaciones del desarrollador

| Necesidad | Comando |
|---|---|
| Preparar el entorno por primera vez | `cargo xtask bootstrap` |
| Actualizar el entorno tras `git pull` | `cargo xtask bootstrap` |
| Verificar el entorno | `cargo xtask doctor` |
| Reproducir el artefacto de release | `cargo xtask package` |
| Instalar el build local | `cargo xtask install` |
| Desinstalar el build local | `ai-voice-interconnector self uninstall` |
| Limpiar el repositorio / la aplicación / todo | `cargo xtask clean` / `clean --app` / `clean --all` |

## 10. Matriz de paridad por target

| Operación | Windows x86_64 | Linux x86_64 | Linux arm64 | macOS arm64 |
|---|---|---|---|---|
| Primera instalación | `irm …/install.ps1 \| iex` | `curl …/install.sh \| sh` | Ídem | Ídem (o Cask) |
| Directorio de programa | `%LOCALAPPDATA%\Programs\ai-voice-interconnector` | `~/.local/opt/ai-voice-interconnector` | Ídem | Ídem |
| Integración de PATH | `HKCU\Environment\Path` (tipo conservado) + `WM_SETTINGCHANGE` | Enlace en `~/.local/bin` + bloque en los perfiles | Ídem | Ídem |
| Particularidad | Ejecutable en uso: se aparca y se borra de forma diferida | glibc ≥ 2.35 comprobada al arrancar | Ídem + userland de 64 bits | Cuarentena limpiada en todo el bundle; detección correcta bajo Rosetta |
| Actualizar | `ai-voice-interconnector self update` | Ídem | Ídem | Ídem (Cask: `brew upgrade --cask`) |
| Desinstalar | `ai-voice-interconnector self uninstall` | Ídem | Ídem | Ídem (Cask: `brew uninstall --cask --zap`) |
| Limpiar estado | `ai-voice-interconnector cleanup …` | Ídem | Ídem | Ídem |
| Entorno de desarrollo | `cargo xtask bootstrap` (Build Tools + MSYS2) | `cargo xtask bootstrap` (apt/dnf/pacman/zypper) | Ídem | `cargo xtask bootstrap` (Xcode CLT, Homebrew) |

Las diferencias entre columnas son mecanismos idiomáticos de cada SO. La experiencia (comandos, confirmaciones, resultados y residuo) es la misma.

El uso de la aplicación (CLI, daemon, voces de fábrica y de usuario, esquemas `--json` y códigos de salida) tampoco varía entre SO; solo cambia el backend de audio, una tecnología equivalente y no una diferencia de experiencia.

### Brechas conocidas

Son las únicas asimetrías de experiencia entre targets que permanecen abiertas, por decisión y no por olvido:

| Brecha | Targets | Situación |
|---|---|---|
| **Firma de código** | Windows y macOS | Los binarios no van firmados ni notarizados. Los one-liners descargan por CLI (sin Mark-of-the-Web) y `self install` limpia la cuarentena de macOS, y el Cask hace lo propio; un archivo descargado por navegador sí dispara SmartScreen o Gatekeeper. Es cross-SO y está diferida al goal a largo plazo porque depende de terceros (SignPath OSS, Apple Developer). Ver [§11](#11-seguridad) y [SECURITY.md](../../SECURITY.md#artefactos-sin-firmar) |
| **Cobertura de arquitecturas** | Todos | Solo los cuatro targets de [§3](#3-targets-soportados); no hay Windows ARM64 ni macOS Intel. Es una limitación de toolchain aceptada |
| **Provisión de modelos en el Cask** | macOS (Homebrew) | Homebrew no admite un post-install arbitrario, así que el Cask no ejecuta `setup`: imprime un *caveat* que remite a `ai-voice-interconnector setup`. Además exige tener Homebrew, un prerrequisito que el one-liner no tiene |

Al abrir o cerrar una brecha se actualiza esta tabla. El historial de las ya cerradas queda en `CHANGELOG.md`.

## 11. Seguridad

- **Transporte**: solo HTTPS (en `curl`, `--proto '=https' --tlsv1.2`), sin vuelta atrás a HTTP.
- **Integridad**: el SHA-256 del archivo se verifica contra `SHA256SUMS.txt` antes de extraer o ejecutar nada, buscando el nombre exacto del archivo.
- **Autenticidad (limitación actual)**: `SHA256SUMS.txt` procede del mismo release que el archivo, así que detecta corrupción, pero no un release comprometido. **Mejora aprobada, diferida**:
  - firmar `SHA256SUMS.txt` con una clave ed25519 (formato minisign) en el job de publicación;
  - embeber la clave pública en el binario, para que `self update` verifique la firma sin herramientas externas;
  - en el bootstrap, verificar la firma solo si `minisign` está disponible. La primera instalación sigue confiando en HTTPS y en GitHub, como cualquier `curl | sh`.
- **Código remoto**: lo único que se ejecuta sin verificación previa es el propio bootstrap, algo inherente a `curl | sh` e `irm | iex`. Por eso es mínimo, se publica como asset versionado, figura en `SHA256SUMS.txt` y tiene una alternativa inspeccionable.
- **Privilegios**: nunca se eleva (salvo `cargo xtask bootstrap --system`, por petición explícita); se rechaza `sudo` en Unix y se avisa si el proceso está elevado en Windows ([§8.1](#81-reglas-transversales)).
- **Borrado**: rige por las reglas R1–R3 ([§6](#6-modelo-de-rutas-y-propiedad)). Ni una variable de reubicación ni un recibo manipulado pueden ampliar el alcance del borrado.
- **Staging y temporales**: se crean con permisos exclusivos del usuario.
- **Reputación de binarios sin firmar**: la descarga por CLI no aplica Mark-of-the-Web y `self install` limpia la cuarentena de macOS. Estas medidas mitigan el síntoma; la solución de fondo es la firma de código (goal a largo plazo, [SECURITY.md](../../SECURITY.md#artefactos-sin-firmar)).
- **Sin telemetría**: ninguna operación envía información. Las únicas peticiones de red son las descargas de releases y de modelos.

## 12. Estrategia de pruebas

| Nivel | Qué cubre | Dónde corre |
|---|---|---|
| Unitarias (Rust) | Detección de target, comparación de versiones, lectura y escritura del recibo, edición de PATH (bloques de perfil; lista de PATH de Windows con conservación del tipo), planificador de transacciones y rollback, plan de limpieza (reglas de propiedad) | Las 3 puertas de test |
| Integración (Rust, aislada) | Ciclos completos install → update → update sin cambios → uninstall; checksum inválido; interrupción y recuperación; daemon activo durante la actualización; canales `homebrew` y `dev`; confirmación sin terminal | Las 3 puertas, con las raíces reubicadas a temporales y un servidor HTTP local que sirve releases falsos (`AVI_DOWNLOAD_BASE_URL`). En Windows, la integración de PATH se prueba sobre una clave de registro de prueba |
| Bootstrap | Detección de target, resolución de versión, checksum inválido, binario incompatible, paso de opciones y ausencia de efectos en la sesión de PowerShell; ejecución real de `irm | iex` contra el servidor local con PowerShell 5.1 y 7; guarda de codificación ASCII de los `.ps1` | bats (Linux, macOS) y Pester (Windows), contra el mismo servidor local |
| Humo del empaquetado | `cargo xtask package`, luego `self install --no-setup --no-modify-path` en un sandbox, luego `--version` | Los 4 jobs de build (cubre Linux arm64, que no tiene puerta de test propia) |
| E2E real | One-liner contra el release publicado, en máquinas reales | Manual, fuera del pipeline, según la política actual de validación E2E |

Las interrupciones se simulan con un punto de inyección de fallos que solo existe en builds de prueba, nunca en el binario distribuido.

## 13. Papel de la documentación

| Documento | Papel tras la implementación |
|---|---|
| `docs/specs/sdlc-lifecycle.md` (este) | Fuente de verdad funcional del ciclo de vida |
| `README.md` | One-liners y los tres comandos esenciales (`self update`, `self uninstall`, `cleanup`) |
| `USAGE.md` | Guía de usuario del ciclo de vida |
| `docs/CLI/README.md` | **Índice** de `docs/CLI/`: el árbol de documentos, la tabla de comandos de nivel superior (con `self` y **sin** `uninstall`) y la tabla de códigos de salida con los siete enteros del ciclo de vida |
| `docs/CLI/CONTRACT.md` | Contrato de `self *`, `setup`, `cleanup` y `doctor`: flags, `reason` y códigos de salida, sobre `--json` y las dos versiones de esquema |
| `docs/CLI/commands/SELF.md` | Documento del grupo `self`, creado en el ciclo 1 y con 201 líneas: los tres modos de `self install`, sus doce pasos, `setup_failed` como éxito parcial y el alcance real de `self uninstall` sobre el estado |
| `docs/CLI/commands/CLEANUP.md` | Documento de `cleanup` **contra el módulo `cleanup` de `avi-lifecycle`**: el planificador único, las reglas R1–R3, el gate de categoría y la confirmación destructiva |
| `docs/CLI/commands/SETUP.md` | Documento de `setup` **contra el módulo `setup` de `avi-lifecycle`**: selección, idempotencia, caché exclusiva de modelos y lo que llega en el Ciclo 2 |
| `docs/CLI/commands/DOCTOR.md` | Documento de `doctor` **contra el módulo `doctor` de `avi-lifecycle`**: las nueve claves del sobre, las cuatro retiradas, los seis chequeos y el veredicto de un solo objeto |
| `docs/BUILD.md` y `CONTRIBUTING.md` | Comandos de `cargo xtask` para el entorno de desarrollo; los requisitos, vía `cargo xtask doctor` |
| `docs/DISTRIBUTION.md` | Canales (script, Cask), antivirus y runbook de reporte a Microsoft; absorbe lo vigente de `SELF-HOSTED-INSTALL.md` |
| `docs/SELF-HOSTED-INSTALL.md` | Retirado: no existe en el arbol. Su contenido vigente esta en `docs/DISTRIBUTION.md` |
| `docs/DESIGN.md` | Arquitectura, motor TTS, estructura del proyecto y comandos; su árbol describe las piezas vigentes, de modo que la ausencia de los cinco scripts de la raíz es un hecho comprobable y no un olvido |
| `docs/GOAL.md` | Especificación ideal del producto y clasificación de specs, con la firma de código en el goal a largo plazo; el criterio de equivalencia entre SO nombra la invocación vigente y conserva la de entonces como constancia |
| `docs/MANUAL-VALIDATION.md` | Procedimiento operativo de la validación end-to-end manual: la matriz de la CLI que CI no puede ejercitar, con la caché de modelos y la integración de `PATH` vigentes |
| `docs/RELEASING.md` | El corte de release: bump de las cinco versiones, promoción de `[No publicado]` por `cargo xtask release` y aborted list del gate de publicación |
| `docs/CLI/commands/VERSION.md` | Documento de `version`, el único comando de nivel superior sin motor en `avi-lifecycle`: lo resuelve `handle_version` en `src/main.rs`, y por eso el sobre `--json` es toda su superficie |
| `THIRD-PARTY-LICENSES.md` | Inventario de licencias de terceros, con la región generada por `cargo xtask licenses` y la región curada arriba; gobierna el aviso de atribución de los pesos de modelo |
| `SECURITY.md` | Política y runbook: Mark-of-the-Web, los dos bootstrap como única ejecución previa al binario, y el aviso de binarios sin firmar diferido al goal a largo plazo |
| `AGENTS.md` | Directrices de trabajo en este repositorio: idioma, surgicalidad, disciplina de versionado y prohibiciones; gobierna a quien modifica el proyecto, no al usuario final |
| `.claude/skills/test-windows-e2e-as-final-user/SKILL.md` | Receta reutilizable del recorrido E2E en Windows como usuario final. **Fuera de git** (`.gitignore:62`): su corrección no entra en ningún commit y su reversión es el texto anterior, no `git checkout` |
| `.claude/skills/release/SKILL.md` | Receta del corte de release en siete pasos, del gate de confirmación a la verificación del release. También **fuera de git** (`.gitignore:62`), con la misma consecuencia |
| Memorias del agente (`~/.claude/projects/…/memory/`) | Índice de lo aprendido entre sesiones, **fuera de git** y mantenido por el orquestador. No documenta el producto, así que no es parte de su documentación de primera parte |

## 14. Criterios de aceptación

**Instalación**

1. En una máquina limpia de cada target, el one-liner deja el comando disponible en una terminal nueva, `doctor --json` en estado correcto y un recibo válido.
2. Repetir el one-liner con la misma versión termina con éxito y deja el mismo estado, sin entradas de PATH ni bloques de perfil duplicados.
3. Un checksum incorrecto termina con `checksum_mismatch` sin modificar nada fuera del staging, y el staging se borra.
4. En un target no soportado, el bootstrap termina con `unsupported_platform` antes de descargar el archivo.
5. En Linux con glibc < 2.35 o con musl, el bootstrap termina con `binary_incompatible` y un diagnóstico, sin tocar la instalación existente.
6. Con `--no-setup` no se descarga ningún modelo. Sin esa opción, un fallo de `setup` deja el programa instalado y termina con `setup_failed`.
7. En Windows, el tipo del valor `Path` de HKCU y sus entradas `%VAR%` quedan intactos después de instalar y de desinstalar.
8. Bajo `irm | iex`, un error no cierra la consola, y la sesión no conserva variables de preferencia ni funciones del instalador.
9. En macOS, ningún archivo del directorio de programa conserva `com.apple.quarantine`, tampoco cuando el archivo se descargó por navegador.
10. Con `curl | sh` y una terminal disponible, las confirmaciones son interactivas.

**Actualización**

11. `self update` estando en la última versión termina con `already_up_to_date` sin descargar nada.
12. `self update --check` informa la transición y no modifica el disco.
13. Con el daemon activo, la actualización lo detiene antes del reemplazo y termina con éxito en los cuatro targets, incluido Windows con el ejecutable en uso.
14. Interrumpir la actualización en cualquier punto deja operativa la versión anterior, y la siguiente operación de ciclo de vida completa la recuperación.
15. En los canales `homebrew` y `dev`, `self update` termina con `externally_managed` e indica el comando correcto.
16. Después de actualizar, los modelos cuyo pin cambió quedan provisionados y las revisiones propias obsoletas quedan podadas.

**Desinstalación y limpieza**

17. `self uninstall --yes` elimina el programa, la integración de PATH y el estado, sin residuo dentro de las raíces de propiedad exclusiva. En Windows puede terminar con `removal_scheduled`, y el directorio desaparece cuando el proceso termina. Si el borrado no puede programarse, el comando termina con `program_dir_kept` y nunca con éxito.
18. `self uninstall --keep-data` conserva modelos, voces y habla sintetizada.
19. Sin terminal y sin `--yes`, toda operación destructiva termina con `confirmation_required` y no borra nada.
20. `--dry-run` en cualquier operación destructiva lista rutas y tamaños sin modificar el disco.
21. Repetir `self uninstall` en un sistema ya limpio termina con éxito (`not_installed`).
22. Cada categoría de `cleanup` borra solo su alcance, y `cleanup` sin categoría termina con `usage_error`.
23. Ninguna operación borra recursos compartidos: una caché HF configurada por el usuario (salvo los repos propios), `~/.cargo` o sccache.

**Entorno del desarrollador**

24. En una máquina con los requisitos del sistema, `cargo xtask bootstrap` deja el entorno listo para `cargo test` y para `cargo run --features full` con STT. Repetirlo sin cambios en el repositorio no hace trabajo.
25. `cargo xtask package` produce en el host un archivo con el mismo layout que el del release de ese target.
26. `cargo xtask install` instala el build local por el mismo camino que el one-liner (canal `dev`).
27. `cargo xtask clean` sin flags solo borra la capa del repositorio.

**Organización**

28. La raíz del repositorio no contiene scripts de ciclo de vida, y ninguna ruta de instalación o de estado se define fuera de `avi-shared` —la fuente única, reexportada por `avi-store` para conservar la API de los llamadores—. La fuente se movió a `avi-shared` en el ciclo 4, cuando las rutas pasaron a ser datos que también consume `xtask` sin red ni TLS; el listón no baja por el cambio de crate, porque la exigencia sigue siendo **una sola definición de rutas**, y la unicidad ahora se comprueba en el sitio donde vive la definición.

## 15. Decisiones cerradas

Cerradas en el gate de alcance de la orquestación. El texto de cada sección affected ya asume la opción elegida.

| Id | Decisión | Cerrada como | Motivo |
|---|---|---|---|
| D1 | URLs antiguas del one-liner (`raw…/main/install-*.sh`) | Retiradas en el corte, sin scripts puente | Proyecto pre-1.0, URLs nuevas estables y cero piezas en la raíz |
| D2 | PATH persistente en Linux y macOS | Modificado por defecto, anunciado en el resumen, con `--no-modify-path` y reversión exacta | Paridad con Windows, menos fricción en macOS y reversibilidad garantizada por el recibo |
| D3 | Ubicación de los modelos | Caché exclusiva de la aplicación, con `HF_HUB_CACHE`/`HF_HOME` como opción de quien quiera compartir | Limpieza y desinstalación seguras borrando directorios completos, sin tocar `xet` ni `.locks` compartidos; se pierde la deduplicación con otras herramientas HF, un caso marginal |
| D4 | Datos en Windows | `%LOCALAPPDATA%\ai-voice-interconnector\data` | Los datos pesados y el estado ligado a la máquina (`daemon.pid`) no deben sincronizarse con perfiles móviles |
| D5 | Capa por defecto de `cargo xtask clean` | Solo el repositorio | Menor sorpresa, como `cargo clean`: borrar la instalación del usuario y los modelos requiere pedirlo explícitamente |
| D6 | Daemon tras `self update` | El resumen indica cómo reiniciarlo | El reinicio automático exigiría persistir los parámetros de arranque; se puede añadir después sin romper el contrato |
| D7 | Firma de `SHA256SUMS.txt` | Fase diferida, fuera de esta implementación | Aporta autenticidad en `self update`, pero no bloquea la consolidación; requiere gestionar una clave en la CI |
