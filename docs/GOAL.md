# Goal: Sistema TTS con Chatterbox Multilingual V3

## Introducción

Este documento es la **especificación ideal** de `tts-sidecar`: describe el producto meta que guía todo el desarrollo — un sistema de síntesis de voz **100% local**, con clonación de voz en **español latinoamericano**, consumible por línea de comandos desde cualquier lenguaje, y con una experiencia de usuario **equivalente en Windows, Linux y macOS**.

Es la fuente de verdad contra la que se evalúa cualquier cambio: si una propuesta acerca el proyecto a lo aquí especificado, avanza el goal; si no, queda fuera de alcance. Para mantener esa función sin frenar el desarrollo, el documento se declara en **dos partes**:

- **[Goal inmediato](#goal-inmediato)** — la especificación y el diseño del objetivo completo, redactados como el ideal que se sigue **como si ya estuviese implementado**. Todo cambio sin impedimentos se trabaja contra esta parte, de forma inmediata.
- **[Goal a largo plazo](#goal-a-largo-plazo)** — las especificaciones **no comprometidas** para el goal actual, separadas para no frenar el desarrollo que no tiene impedimentos. Cada spec diferida lleva su justificación y su condición de entrada; al cumplirse la condición, se promueve al goal inmediato.

El [criterio de clasificación](#clasificación-de-specs) decide a cuál de las dos partes va cada spec nueva.

## Tabla de contenidos

- [Clasificación de specs](#clasificación-de-specs)
- [Goal inmediato](#goal-inmediato)
  - [Objetivo](#objetivo)
  - [Alcance](#alcance)
  - [Restricciones](#restricciones)
  - [Especificación](#especificación)
    - [Instalador (canal nativo)](#instalador-canal-nativo)
    - [Paridad de experiencia](#paridad-de-experiencia)
    - [Comandos CLI](#comandos-cli)
    - [Desinstalación en un comando](#desinstalación-en-un-comando)
    - [Estructura del proyecto](#estructura-del-proyecto)
  - [Criterios de aceptación](#criterios-de-aceptación)
    - [Validación E2E](#validación-e2e)
  - [Condición de finalización](#condición-de-finalización)
- [Goal a largo plazo](#goal-a-largo-plazo)
  - [Firma de código y notarización](#firma-de-código-y-notarización)

---

## Clasificación de specs

Toda spec nueva se clasifica corriendo este test **antes** de ubicarla, sin heredar la clasificación de specs vecinas. Una spec va al **goal a largo plazo** únicamente si cumple **al menos uno** de estos tres impedimentos objetivos:

1. **Gate externo / dependencia de un tercero**: no puede completarse sin la aprobación o el alta de un tercero (p. ej. aprobación del programa SignPath OSS, cuenta Apple Developer de pago).
2. **Condición de madurez / cristalización del producto**: solo tiene sentido sobre un producto ya estabilizado, y hacerla antes obligaría a rehacerla mientras el producto sigue cambiando de forma (p. ej. firmar/notarizar artefactos que aún mutan).
3. **Impedimento activo que bloquea o contamina el desarrollo inmediato**: trabajarla ahora frenaría o ensuciaría la spec que guía el trabajo diario.

Si la spec **no cumple ninguno** de los tres, va al **goal inmediato** y se trabaja ya — **aunque no sea prioritaria**. Reglas para evitar el diferimiento injustificado:

- Una **preferencia de priorización** («prefiero cerrar X antes que Y», «invertir en esto cuando haya tiempo») **no es** un impedimento y **no justifica** diferir. Priorizar es elegir el orden dentro del goal inmediato, no expulsar la spec de él.
- Una condición de entrada válida es **objetiva y externa** (una aprobación, un alta, un release de un tercero). Si la condición de entrada es «cuando el dueño decida priorizarla», la spec **no** tenía impedimento y pertenece al goal inmediato.
- Cuando se pida **justificar** un diferimiento ya hecho, re-evaluar si la spec era diferible según este test — **no** rellenar el formato con una racionalización que confirme la decisión previa.

---

## Goal inmediato

## Objetivo

Obtener un sistema TTS **100% local** con audio nativo por sistema operativo, para transformar texto a audio en **español latino** de la mejor calidad disponible, distribuido bajo **licencia GPL-3.0-or-later** (con dependencias y modelo bajo licencias permisivas compatibles).

Motor TTS: **Chatterbox Multilingual V3** (ResembleAI) — 23+ idiomas, clonación de voz, licencia MIT.

**El sistema debe ser consumible via línea de comandos** para que cualquier aplicación en cualquier lenguaje de programación pueda invocarlo (Python, JavaScript/Node, Rust, Go, Java, C#, etc.)

**La experiencia del usuario final debe ser equivalente en Windows, Linux y macOS**: instalar, usar, actualizar y desinstalar con la misma cantidad de fricción, privilegios y residuo en los tres SO. Las diferencias tecnológicas idiomáticas por SO (Inno Setup, AppImage, `.dmg`/Cask) son aceptables; las diferencias de experiencia no. El estado de esta equivalencia y las brechas pendientes se registran en [docs/PARITY.md](PARITY.md).

## Alcance

Implementar y validar la síntesis en español latinoamericano con voz propia del usuario usando Chatterbox Multilingual V3, distribuida con **equivalencia funcional completa** entre Windows, Linux y macOS: el cierre de las brechas registradas en [docs/PARITY.md](PARITY.md) es parte del alcance del goal inmediato.

## Restricciones

- **100% local**: Sin APIs externas ni conexiones a internet para síntesis
- **Instalador único por SO (canal nativo)**: Un archivo ejecutable por plataforma; el canal PyPI complementario (ver [docs/DISTRIBUTION.md](DISTRIBUTION.md)) no está sujeto a esta restricción
- **Sin dependencias externas (canal nativo)**: El usuario final no necesita instalar nada más; el canal PyPI requiere Python 3.13+ y, en Linux, `libportaudio2` del sistema
- **Licencia**: El código propio se distribuye bajo GPL-3.0-or-later; todas las dependencias y el modelo usados deben tener licencias compatibles con GPLv3 (permisivas — MIT/BSD/Apache/ISC/PSF — o copyleft compatible, como LGPL-2.1+/MPL-2.0)

## Especificación

### Instalador (canal nativo)

Estos requisitos aplican al **canal nativo** (binarios PyInstaller por SO), que sigue siendo el canal recomendado para usuarios sin Python instalado:

- **Un solo instalador por SO**: Windows (.exe), Linux (.AppImage/.bin), macOS (.app)
- **Cero dependencias externas**: El usuario final no instala Python, Node, Rust ni nada más
- **Descarga + instalación + configuración** en un solo paso
- **Audio nativo**: playback usando APIs nativas de cada SO
- **Paridad de ciclo de vida entre SO**: instalación de una línea sin privilegios de administrador, modelo provisionado al terminar, actualización sin residuo y desinstalación con residuo cero, en los tres sistemas operativos por igual (ver [docs/PARITY.md](PARITY.md))

El proyecto distribuye además un **canal PyPI** complementario (`uv tool install tts-sidecar` / `pipx install tts-sidecar`) para audiencia técnica con Python 3.13+ ya instalado, que no está sujeto a estos requisitos (sí requiere Python, y en Linux la librería del sistema `libportaudio2`). Ver [docs/DISTRIBUTION.md](DISTRIBUTION.md) para la matriz de trade-offs completa entre ambos canales.

### Paridad de experiencia

El ideal de paridad que persigue el goal inmediato, por fase del ciclo de vida (el estado real y el registro de brechas viven en [docs/PARITY.md](PARITY.md)):

| Fase | Ideal en los 3 SO |
|---|---|
| Instalación | Una línea, sin prerequisitos de terceros, sin privilegios de admin, checksum verificado |
| Primer arranque | Sin advertencias de reputación en la vía de una línea |
| Provisión | Modelo descargado al terminar la instalación (`setup` encadenado u ofrecido) |
| Uso | CLI, daemon, voces y contratos `--json` idénticos |
| Actualización | Reemplaza la versión anterior sin residuo ni pasos-trampa |
| Desinstalación | Datos (`cleanup --all`) + binario, con residuo cero |

### Comandos CLI

Los comandos están ordenados en secuencia de dependencia: cada paso solo requiere que los anteriores hayan funcionado. El daemon es el camino principal de uso: carga el modelo una sola vez y lo mantiene en memoria, eliminando el overhead de carga en cada invocación. Por eso su ciclo de vida envuelve toda la sesión: se arranca antes de sintetizar y se detiene al final.

```bash
# 1. Provisión (primera vez - chequeos + descarga el modelo si falta)
./tts-sidecar setup

# 2. Diagnóstico del sistema (no depende de nada)
./tts-sidecar version              # Versión instalada
./tts-sidecar doctor               # Chequeo de entorno y modelo
./tts-sidecar devices              # Dispositivos de audio disponibles

# 3. Arrancar el daemon (camino principal: carga el modelo en memoria una vez)
./tts-sidecar daemon start         # Iniciar daemon
./tts-sidecar daemon status        # Verificar que está activo

# 4. Clonación de voz (requiere dos archivos de audio)
./tts-sidecar voice clone --name mi_voz --timbre-reference timbre.wav --speech-reference condicion.wav

# 5. Listar voces registradas (verifica que la voz quedó registrada)
./tts-sidecar voice list

# 6. Síntesis a través del daemon (añade -v mi_voz para usar la voz clonada)
./tts-sidecar speech say --text "Hola mundo" [-v mi_voz]                    # Reproducir
./tts-sidecar speech synthesize --text "Hola mundo" [-v mi_voz] --label LOCUCION  # Sintetiza y guarda la locución en el almacén

# 7. Eliminar voz clonada (limpieza)
./tts-sidecar voice remove --name mi_voz

# 8. Detener el daemon (cierre de la sesión, libera el modelo de memoria)
./tts-sidecar daemon stop
```

### Desinstalación en un comando

La desinstalación es **equivalente en esfuerzo a la instalación de una línea**: un único comando elimina binario, PATH integrado y datos (modelo y voces), con residuo cero, en los tres SO. `tts-sidecar setup --uninstall` es multiplataforma y espeja la instalación one-line de cada plataforma. La desinstalación es atómica de cara al usuario: cancelar la confirmación del borrado aborta el proceso sin eliminar nada. Cada SO elimina el mismo conjunto de componentes; la secuencia interna de borrado y su mecánica son detalle de implementación (ver el plan técnico en [docs/ROADMAP.md](ROADMAP.md)):

- **Linux**: el symlink `~/.local/bin/tts-sidecar`, el directorio de instalación `~/.local/opt/tts-sidecar/` y los datos (`cleanup --all`). Sin `sudo`.
- **macOS**: los datos (`cleanup --all`), el symlink `~/.local/bin/tts-sidecar` y el `.app` (`rm -rf` seguro sobre el bundle en ejecución). Si la instalación proviene de Homebrew, la desinstalación completa se remite a `brew uninstall --cask --zap` (que cubre también los datos) en lugar de proceder. Sin `sudo`.
- **Windows**: los datos (`cleanup --all`) en proceso; el binario y el PATH se delegan al desinstalador de Inno Setup (per-user, sin admin), leído desde `HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\{AppId}_is1`.

Las vías idiomáticas por SO (desinstalador de Inno en Configuración → Aplicaciones, `brew uninstall --cask --zap` en macOS vía Homebrew) se conservan en paralelo como alternativas; `setup --uninstall` es la vía equivalente de un comando en las tres plataformas. El estado real de esta paridad vive en [docs/PARITY.md](PARITY.md).

### Estructura del proyecto

```
TTS-Sidecar/
├── src/
│   └── tts_sidecar/       # Paquete Python (publicable en PyPI)
│       ├── __init__.py       # Fuente única de versión (__version__)
│       ├── __main__.py       # Entry point de `python -m tts_sidecar`
│       ├── bootstrap.py      # apply() idempotente: warnings, env vars, mock pkg_resources
│       ├── engine.py         # Wrapper de Chatterbox
│       ├── audio.py          # Reproducción de audio nativa
│       ├── cli.py            # Interfaz CLI (invoca bootstrap.apply())
│       ├── timing.py         # Instrumentación y timing
│       ├── voices.py         # Resolución de voces usuario→fábrica
│       ├── paths.py          # Rutas: fábrica relativa al paquete, datos en user-data-dir por SO
│       ├── model_cache.py    # Detección del modelo en la caché de HF
│       ├── voices/           # Voces de FÁBRICA (empaquetadas en wheel y bundle)
│       │   └── default/      # timbre-reference.wav + speech-reference.wav
│       └── daemon/           # Daemon mode (FastAPI + IPC)
│           ├── daemon.py    # Gestor del ciclo de vida
│           ├── server.py    # Endpoints FastAPI
│           ├── ipc.py       # Cliente HTTP
│           ├── protocol.py  # Modelos Pydantic
│           └── run.py       # Entry point (usa bootstrap.apply())
├── bin/
│   └── tts-sidecar          # Script de entry point (modo fuente)
├── scripts/                  # Scripts de build por SO
├── tests/                    # Suite de tests pytest
└── docs/
```

> El modelo `es-mx-latam` no se almacena en el repo: reside en la caché de
> HuggingFace del usuario (`~/.cache/huggingface/hub`) tras `tts-sidecar setup`.

## Criterios de aceptación

<!-- Los criterios 1-3 y 9 son claims de ejecución por SO: el pipeline de build (CI + scripts/build_*.py) produce los instaladores y un smoke test automatizado del binario congelado (`tts-sidecar version`), pero la validación end-to-end sobre cada SO es por diseño externa al pipeline (ver "Validación E2E" más abajo). -->

1. [ ] El instalador de Windows (.exe) funciona en Windows 10/11 sin dependencias (validación E2E por SO, ver "Validación E2E" más abajo)
2. [ ] El instalador de Linux funciona en distribuciones principales (validación E2E por SO, ver "Validación E2E" más abajo)
3. [ ] El instalador de macOS funciona en el mínimo declarado por `LSMinimumSystemVersion` (Apple Silicon; Mac Intel no soportado) — derivado dinámicamente del `MACOSX_DEPLOYMENT_TARGET` del toolchain de build, no un número fijo (validación E2E por SO, ver "Validación E2E" más abajo)
4. [x] `tts-sidecar speech say --text "Hola mundo"` reproduce audio en español
5. [x] `tts-sidecar voice clone --name test --timbre-reference ref.wav --speech-reference speech.wav` clona la voz
6. [x] El audio generado suena en español con las características de la voz de referencia
7. [x] El español latinoamericano suena natural y con buena prosodia
8. [x] La síntesis funciona sin conexión a internet (modelo en local)
9. [ ] El instalador incluye todo lo necesario (no requiere instalaciones adicionales) (validación E2E por SO, ver "Validación E2E" más abajo)
10. [ ] **Equivalencia funcional completa entre los 3 SO**: todas las brechas accionables del registro de [docs/PARITY.md](PARITY.md) están cerradas a nivel de código/scripts/tests (one-liner macOS `install-macos.sh`, `.command` sin `sudo`, limpieza de AppImages en `install-linux.sh`, `zap` del Cask completo, README con las tres plataformas — cerradas en v0.5.0 — y `setup --uninstall` multiplataforma — brecha de *desinstalación en un comando*, cerrada a nivel de código/scripts/tests en v0.6.0). Solo la brecha de *firma de código* (SmartScreen/Gatekeeper, binarios sin firmar, cross-SO) permanece diferida por diseño al goal a largo plazo. Con ello **todas las brechas accionables están cerradas en código**; la marca de este criterio queda pendiente solo de la validación por feedback de usuarios reales en Linux y macOS (ver "Validación E2E" más abajo)

### Validación E2E

La validación end-to-end de los instaladores (instalar → `setup` → `speech synthesize` real → desinstalar) **no se ejecuta dentro del pipeline de CI** por una decisión consciente de diseño: requiere cuota de runner significativa (carga del modelo Chatterbox + descarga de ~4 GB de pesos + síntesis real con audio) y reproducirla en cada push no aporta señal proporcional a su coste. El pipeline sí ejecuta un **smoke test automatizado** del binario congelado (`tts-sidecar version`, exit 0) en los cuatro jobs de build, que detecta empaquetados rotos (metadata faltante, `--collect-all` incompleto) sin pagar el coste del modelo.

Fuera del pipeline, la validación se reparte así:

- **Windows**: la realiza el propietario manualmente sobre su equipo local, instalando el artefacto de cada release, ejecutando el recorrido `setup` → `speech synthesize` → desinstalar, y registrando el resultado.
- **Linux y macOS**: dependen de **feedback de usuarios reales** que prueben la instalación y ejecución en sus equipos. Ese feedback (positivo o negativo) es la entrada de issues que cierra el circuito y guía correcciones específicas por plataforma.

Por tanto, los criterios 1-3 y 9 no son "pendientes" en el sentido de tareas olvidadas: son el **borde externo** del proceso de calidad, donde el propietario más el feedback de la comunidad reemplazan a un runner de CI que no podría ejercitar la matriz de hardware/SO real. Cualquier issue reportado en estos criterios se incorpora al ciclo de desarrollo como bug prioritario y motiva fixes versionados.

## Condición de finalización

La implementación está completa únicamente cuando:

- [x] El motor Chatterbox Multilingual V3 está implementado y funcional
- [x] La clonación de voz funciona con una muestra de ~10 segundos
- [x] El audio generado preserva las características de la voz original
- [x] El español latinoamericano suena natural
- [x] Hay scripts de build e instalador por cada SO (Windows, Linux, macOS) en el pipeline de CI
- [ ] Los instaladores funcionan sin ninguna dependencia externa (validación E2E por SO, ver "Validación E2E" arriba: smoke test automatizado en CI + validación manual Windows del propietario + feedback de usuarios reales en Linux y macOS)
- [ ] La experiencia de instalación, uso, actualización y desinstalación es equivalente en los 3 SO: [docs/PARITY.md](PARITY.md) sin brechas **accionables** abiertas (la de *firma de código* permanece registrada como diferida al goal a largo plazo y no bloquea esta condición)
- [x] **README.md** refleja la nueva arquitectura con Chatterbox
- [x] **docs/DESIGN.md** corresponde al estado implementado
- [x] El daemon mode está implementado y funciona correctamente
- [x] Los logs están normalizados con estructura consistente
- [x] Los tests pytest pasan (572/572)

---

## Goal a largo plazo

Especificaciones **no comprometidas** para el goal inmediato. No se trabajan ahora — cada una registra por qué se difiere (según el [criterio de clasificación](#clasificación-de-specs)) y qué condición la promueve al goal inmediato.

## Firma de código y notarización

**Motivación**: los binarios del canal nativo no están firmados, por lo que Windows SmartScreen y macOS Gatekeeper bloquean el primer arranque cuando el artefacto se descarga por navegador (ver `SECURITY.md` §"Artefactos sin firmar" y `docs/BUILD.md` §"Limitación conocida: firma de código y notarización"). Los instaladores de una línea y el canal PyPI ya mitigan esta fricción (descarga por CLI sin Mark-of-the-Web / launcher generado localmente), pero no la eliminan para la descarga directa desde el navegador.

**Justificación del diferimiento**: la firma es un gate que solo vale la pena cuando el proyecto/producto esté **cristalizado y completo** — idealmente sin bugs y con funcionalidad completa y equivalente entre los 3 sistemas operativos ([docs/PARITY.md](PARITY.md) sin brechas abiertas). El producto aún está en desarrollo: firmar ahora significaría re-tramitar la confianza externa (aprobación de SignPath OSS, cuenta Apple Developer de pago) sobre artefactos que siguen cambiando de forma. Solo entonces se iniciará el proceso de firma.

**Especificación diferida**:

- **Windows**: firma Authenticode vía [SignPath Foundation](https://signpath.org/) (firma gratuita para proyectos open source), integrada al job de CI que produce el instalador `.exe`.
- **macOS**: notarización con una cuenta Apple Developer ID, integrada al job de CI que produce el `.dmg`.

**Condiciones de entrada** (promueven esta spec al goal inmediato):

- El goal inmediato está cumplido: producto cristalizado, con equivalencia funcional completa entre los 3 SO ([docs/PARITY.md](PARITY.md) sin brechas accionables abiertas; la única brecha restante sería esta misma, de *firma de código*).
- Windows: aprobación del proyecto por el programa SignPath OSS.
- macOS: alta de una cuenta Apple Developer (de pago).

**Criterio de cierre**: los instaladores de Windows y macOS generados por CI arrancan sin disparar SmartScreen ni Gatekeeper en una instalación limpia, incluso descargados por navegador.
