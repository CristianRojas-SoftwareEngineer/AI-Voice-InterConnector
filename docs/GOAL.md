# Goal: Sistema TTS local con clonación de voz (Qwen3-TTS)

## Introducción

Este documento es la **especificación ideal** de `ai-voice-interconnector`: describe el producto meta que guía todo el desarrollo — un sistema de síntesis de voz **100% local**, con clonación de voz en **español latinoamericano**, consumible por línea de comandos desde cualquier lenguaje, y con una experiencia de usuario **equivalente en Windows, Linux y macOS**.

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
  - [Provisión de modelos en los canales sin `self install`](#provisión-de-modelos-en-los-canales-sin-self-install)

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

### Objetivo

Obtener un sistema TTS **100% local** con audio nativo por sistema operativo, para transformar texto a audio en **español latino** de la mejor calidad disponible, distribuido bajo **licencia MPL-2.0** (con dependencias y modelo bajo licencias compatibles).

Motor TTS: **Qwen3-TTS 0.6B CustomVoice** - multilingüe, clonación de voz; licencia MIT/Apache-2.0.

Un subsistema de **traducción cross-lingual local `es<->en`** (`opus-mt` sobre CTranslate2, modelos ya convertidos que `setup` descarga siempre) cierra el bucle de la clonación de voz: el usuario escribe en su idioma nativo y obtiene audio en el idioma destino con su propia voz clonada, en un solo comando (`speech say`/`synthesize --source-language ... --target-language ...`) o vía el comando `translate` cuando solo necesita el texto traducido. El eslabón de entrada de ese bucle (audio→texto) lo cubre `speech transcribe` (Parakeet TDT v3 int8 vía `ort` `load-dynamic`, incluido por defecto en `setup` base).

**El sistema debe ser consumible via línea de comandos** para que cualquier aplicación en cualquier lenguaje de programación pueda invocarlo (JavaScript/Node, Rust, Go, Java, C#, etc.)

**La experiencia del usuario final debe ser equivalente en Windows, Linux y macOS**: instalar, usar, actualizar y desinstalar con la misma cantidad de fricción, privilegios y residuo en los tres SO. Las diferencias de empaquetado idiomáticas por SO (formatos `tar.gz`/`zip`/Cask) son aceptables; las diferencias de experiencia no. La matriz de equivalencia y las brechas pendientes se registran en [§10 de la especificación del ciclo de vida](specs/LIFECYCLE.md#10-matriz-de-paridad-por-target).

### Alcance

Implementar y validar la síntesis en español latinoamericano con voz propia del usuario usando Qwen3-TTS 0.6B, distribuida con **equivalencia funcional completa** entre Windows, Linux y macOS: el cierre de las brechas de **código** del [registro de brechas](specs/LIFECYCLE.md#brechas-conocidas) es parte del alcance del goal inmediato; las brechas de **experiencia** que dependen de un tercero o del canal de instalación quedan en el [goal a largo plazo](#goal-a-largo-plazo) con su condición de entrada.

### Restricciones

- **100% local**: Sin APIs externas ni conexiones a internet para síntesis
- **Instalador único por SO (canal nativo)**: Un archivo comprimido por plataforma (`tar.gz`/`.zip` con binario Rust); ver [docs/DISTRIBUTION.md](DISTRIBUTION.md)
- **Ninguna instalación adicional (canal nativo)**: solo el sistema base de cada plataforma; el archivo aporta todo lo demás. Los mínimos por plataforma están en [Requisitos del README](../README.md#requisitos)
- **Licencia**: El código propio se distribuye bajo MPL-2.0; todas las dependencias y los modelos usados deben tener licencias compatibles con MPL-2.0 (permisivas — MIT/BSD/Apache/ISC/PSF — o copyleft compatible a nivel de archivo o librería, como MPL-2.0/LGPL-2.1+). El par de traducción `opus-mt` (derivados int8 de Helsinki-NLP/opus-mt, ya convertidos a CTranslate2 y publicados) se distribuye bajo CC-BY-4.0, con atribución registrada en [THIRD-PARTY-LICENSES.md](../THIRD-PARTY-LICENSES.md)

### Especificación

#### Instalador (canal nativo)

Estos requisitos aplican al **canal nativo** (binario Rust autocontenido por SO), que es el único canal de distribución:

- **Un solo artefacto por SO**: Windows (`.zip`), Linux (`tar.gz` x64/arm64), macOS (`tar.gz` arm64) — binario autocontenido + docs MPL-2.0, instala vía one-liner `curl|sh`/`irm|iex` o Cask de Homebrew
- **Ninguna instalación adicional: solo el sistema base de cada plataforma**: El usuario final no instala ningún runtime, librería ni toolchain
- **Descarga + instalación + configuración** en un solo paso (one-liner verifica checksum, extrae, integra PATH y encadena `setup`)
- **Audio nativo**: playback usando APIs nativas del SO (cpal)
- **Paridad de ciclo de vida entre SO**: instalación de una línea sin privilegios de administrador, modelo provisionado al terminar, actualización sin residuo y desinstalación con residuo cero (`self uninstall` borra programa+PATH+estado; `cleanup --all` es la limpieza granular del estado sin tocar el programa — ver `docs/CLI/CONTRACT.md §11`), en los tres sistemas operativos por igual (ver [§10 de la especificación](specs/LIFECYCLE.md#10-matriz-de-paridad-por-target))

La distribución es 100% Rust por archivos comprimidos (ver [docs/DISTRIBUTION.md](DISTRIBUTION.md)).

#### Paridad de experiencia

El ideal de paridad que persigue el goal inmediato, por fase del ciclo de vida (la matriz real y el registro de brechas viven en [§10 de la especificación](specs/LIFECYCLE.md#10-matriz-de-paridad-por-target)):

| Fase | Ideal en los 3 SO |
|---|---|
| Instalación | Una línea, sin prerrequisitos de terceros, sin privilegios de admin, checksum verificado |
| Primer arranque | Sin advertencias de reputación en la vía de una línea |
| Provisión | Modelo descargado al terminar la instalación (`setup` encadenado u ofrecido: en los canales que invocan `self install` se encadena; en los que no, la instalación ofrece el paso explícito) |
| Uso | CLI, daemon, voces y contratos `--json` idénticos |
| Actualización | Reemplaza la versión anterior sin residuo ni pasos-trampa |
| Desinstalación | Estado con `cleanup --all` ( granular, sin tocar el programa) y programa+PATH+estado con `self uninstall`, con residuo cero |

#### Comandos CLI

Los comandos están ordenados en secuencia de dependencia: cada paso solo requiere que los anteriores hayan funcionado. El daemon es el camino principal de uso: carga el modelo una sola vez y lo mantiene en memoria, eliminando el overhead de carga en cada invocación. Por eso su ciclo de vida envuelve toda la sesión: se arranca antes de sintetizar y se detiene al final.

```bash
# 1. Provisión (primera vez - chequeos + descarga el modelo si falta)
./ai-voice-interconnector setup

# 2. Diagnóstico del sistema (no depende de nada)
./ai-voice-interconnector version              # Versión instalada
./ai-voice-interconnector doctor               # Chequeo de entorno y modelo
./ai-voice-interconnector doctor --repair      # Recoge restos pendientes y vuelve a evaluar
./ai-voice-interconnector devices              # Dispositivos de audio disponibles

# 3. Arrancar el daemon (camino principal: carga el modelo en memoria una vez)
./ai-voice-interconnector daemon start         # Iniciar daemon
./ai-voice-interconnector daemon status        # Verificar que está activo
./ai-voice-interconnector daemon restart       # Reiniciar (reemplaza al activo)

# 4. Clonación de voz (requiere --speech-reference: un único audio, ~10 s recomendados, máximo 30 s)
./ai-voice-interconnector voice clone --name mi_voz --speech-reference condicion.wav

# 5. Listar voces registradas (verifica que la voz quedó registrada)
./ai-voice-interconnector voice list

# 6. Síntesis a través del daemon (añade -v mi_voz para usar la voz clonada)
./ai-voice-interconnector speech say --text "Hola mundo" [-v mi_voz]                    # Reproducir
./ai-voice-interconnector speech synthesize --text "Hola mundo" [-v mi_voz] --label LOCUCION  # Sintetiza y guarda la locución en el almacén
./ai-voice-interconnector speech synthesize --text "Hola mundo" --label LOCUCION --play  # Sintetiza y reproduce; solo guarda al elegir 2 (4 descarta)

# 6b. Síntesis cross-lingual opcional: el usuario escribe en su idioma y obtiene
# audio en el idioma destino con su propia voz clonada, en un solo comando
./ai-voice-interconnector speech say --text "Hola mundo" -v mi_voz --source-language es --target-language en

# 6c. O solo el texto traducido, sin síntesis
./ai-voice-interconnector translate --text "Hola mundo" --source-language es --target-language en

# 6d. Entrada de audio a texto: transcribe un WAV o el micrófono
./ai-voice-interconnector speech transcribe --audio condicion.wav --source-language es
./ai-voice-interconnector speech transcribe --mic --source-language es          # Push-to-talk (exige TTY)

# 6e. Doblaje voz→voz: transcribe, traduce si difiere, sintetiza y reproduce
./ai-voice-interconnector speech dub --audio condicion.wav --source-language es --target-language es -v mi_voz

# 7. Locuciones guardadas: listar, reproducir y eliminar
./ai-voice-interconnector speech list [--voice mi_voz]   # Filtrable por voz
./ai-voice-interconnector speech play --label LOCUCION [-v mi_voz]
./ai-voice-interconnector speech remove --label LOCUCION [-v mi_voz]

# 8. Eliminar voz clonada (limpieza; se lleva sus locuciones guardadas)
./ai-voice-interconnector voice remove --name mi_voz

# 9. Limpiar el estado provisionado, por categoría o completo
./ai-voice-interconnector cleanup --voices --synthetic-speech --model   # Sin tocar el programa ni el PATH
./ai-voice-interconnector cleanup --all                                # Unión de las tres categorías

# 10. Operaciones sobre la instalación del usuario
./ai-voice-interconnector self update            # Actualizar a la última estable
./ai-voice-interconnector self uninstall         # Programa + PATH + estado, con residuo cero

# 11. Detener el daemon (cierre de la sesión, libera el modelo de memoria)
./ai-voice-interconnector daemon stop
```

Los subcomandos `speech transcribe`, `speech synthesize`, `speech say`, `speech dub` y `translate` aceptan `--daemon`/`--no-daemon` para fijar el despacho (`Auto` por defecto). `speech list`, `speech play`, `speech remove`, `voice list` y `voice remove` son locales: el daemon no los expone y el parser rechaza esos flags.

#### Desinstalación en un comando

La desinstalación es **equivalente en esfuerzo a la instalación de una línea**: un único comando elimina programa, PATH integrado y estado (modelo y voces), con residuo cero, en los tres SO. `ai-voice-interconnector self uninstall` es el único que borra programa y PATH (`cleanup` es la limpieza granular del estado, sin tocar el programa — `crates/avi-lifecycle/src/cleanup.rs` y `uninstall.rs`, `docs/CLI/CONTRACT.md §11`). Ambas son multiplataforma y espejan la instalación one-line de cada plataforma. La desinstalación es atómica de cara al usuario: cancelar la confirmación del borrado aborta el proceso sin eliminar nada. Cada SO elimina el mismo conjunto de componentes; la secuencia interna de borrado y su mecánica son detalle de implementación:

- **Linux**: el enlace `~/.local/bin/ai-voice-interconnector`, el directorio de instalación `~/.local/opt/ai-voice-interconnector/` y el estado. Sin `sudo`. (`--yes` omite la confirmación; sin terminal es obligatorio)
- **macOS**: análogo a Linux (`self uninstall` retira el enlace + `~/.local/opt` + el estado) en la vía one-liner; con **Homebrew Cask**, `brew uninstall --cask --zap ai-voice-interconnector` sigue siendo la vía idiomática (cubre también el estado). Sin `sudo`.
- **Windows**: el estado, el directorio `%LOCALAPPDATA%\Programs\ai-voice-interconnector` y la entrada del PATH de usuario (`HKCU\Environment` + `WM_SETTINGCHANGE`). Sin UAC; `--yes` omite la confirmación.

Las vías idiomáticas por SO (`brew uninstall --cask --zap` en macOS vía Homebrew) se conservan en paralelo como alternativas; `self uninstall` es la vía equivalente de un comando en las tres plataformas. La matriz real de esta paridad vive en [§10 de la especificación](specs/LIFECYCLE.md#10-matriz-de-paridad-por-target). El binario gestiona PATH/dir directamente.

#### Estructura del proyecto

Ver [Estructura del proyecto en DESIGN.md](DESIGN.md#estructura-del-proyecto).

### Criterios de aceptación

<!-- Los criterios 2, 3 y 9 son claims de ejecución por SO: el pipeline de build (CI) produce los instaladores y un smoke test automatizado (versión contra el tag, `voice list`, `--self-test` del motor y `self install` en sandbox; ver docs/BUILD.md «Verificación post-build»), más una verificación en sala limpia del artefacto de Linux (los cuatro jobs `verify-portability`), pero la validación end-to-end sobre cada SO es por diseño externa al pipeline (ver "Validación E2E" más abajo). -->

1. [x] El artefacto de Windows (`ai-voice-interconnector-X.Y.Z-x86_64-windows.zip`) funciona en el mínimo de Windows declarado en los [requisitos](../README.md#requisitos) sin dependencias (validación E2E por SO, ver "Validación E2E" más abajo) — validado sobre `v0.36.0` en Windows 11 x64 (`10.0.26200`): instalación por el one-liner `irm | iex` con checksum verificado, provisión de los cinco modelos, ciclo de vida del daemon, clonación de voz, síntesis con `WER ≤ 0.25` y superficie `store`/`transcribe`/`translate`/`dub`, sin instalar nada más allá del sistema base. La versión mínima declarada (Windows 10 1903) no se ejercitó en sí misma: el binario es el mismo, pero el suelo declarado queda sin probar en esa máquina concreta
2. [ ] El artefacto de Linux (`ai-voice-interconnector-X.Y.Z-x86_64-linux.tar.gz`) funciona en distribuciones principales (validación E2E por SO, ver "Validación E2E" más abajo)
3. [ ] El artefacto de macOS (`ai-voice-interconnector-X.Y.Z-arm64-macos.tar.gz`) funciona en el mínimo declarado en el manifiesto del bundle (`min_os`, ver [versión mínima de macOS](BUILD.md#versión-mínima-de-macos) y los [requisitos](../README.md#requisitos)) (Apple Silicon; Mac Intel no soportado) (validación E2E por SO, ver "Validación E2E" más abajo)
4. [x] `ai-voice-interconnector speech say --text "Hola mundo"` reproduce audio en español
5. [x] `ai-voice-interconnector voice clone --name test --speech-reference speech.wav` clona la voz
6. [x] El audio generado suena en español con las características de la voz de referencia
7. [x] El español latinoamericano suena natural y con buena prosodia
8. [x] La síntesis funciona sin conexión a internet (modelo en local)
9. [ ] El instalador incluye todo lo necesario (no requiere instalaciones adicionales) (validación E2E por SO, ver "Validación E2E" más abajo)
10. [ ] **Equivalencia funcional completa entre los 3 SO**: las brechas de **código** del [registro de brechas](specs/LIFECYCLE.md#brechas-conocidas) están cerradas a nivel de código/scripts/tests (one-liner con el bootstrap `install.sh` POSIX compartido más `install.ps1` para Windows, `zap` del Cask completo, README con las tres plataformas y desinstalación en un comando con `self uninstall`). Quedan dos brechas de **experiencia**, ambas diferidas por diseño al [goal a largo plazo](#goal-a-largo-plazo) y ninguna bloqueante por sí sola: la *firma de código* (SmartScreen/Gatekeeper, binarios sin firmar, cross-SO) y la *provisión de modelos en los canales que no invocan `self install`* (el Cask de Homebrew y la descarga manual, donde `setup` se ofrece en vez de encadenarse). La marca de este criterio queda pendiente de la validación por feedback de usuarios reales en Linux y macOS (ver "Validación E2E" más abajo)

#### Validación E2E

La validación end-to-end de los instaladores (instalar → `setup` → `speech synthesize` real → desinstalar) **no se ejecuta dentro del pipeline de CI** por una decisión consciente de diseño: requiere cuota de runner significativa (carga de Qwen3-TTS + Parakeet + descarga de los modelos que fija [`setup`](CLI/commands/SETUP.md) + síntesis real con audio) y reproducirla en cada push no aporta señal proporcional a su coste. El pipeline sí ejecuta un **smoke test automatizado** en los cuatro jobs de build, que detecta empaquetados rotos sin pagar el coste del modelo: la versión del binario contra el tag (`--version`; en Windows además `version --json`), `voice list` con la voz `default`, el `--self-test` de los kernels del motor TTS (sin pesos), y la instalación del archivo empaquetado con `self install --no-setup --no-modify-path` en un sandbox, seguida de `--version` del binario instalado.

Además, los cuatro jobs **`verify-portability`** abren el artefacto de Linux ya publicado en una **sala limpia** —AlmaLinux 8 (el suelo de glibc 2.28) y Debian stable-slim, en x64 y arm64, sin paquetes de desarrollo, sin ALSA, sin libgomp y sin procps— y comprueban que el binario arranca, que `voice list` responde, que `self install --no-setup --yes` deja el comando disponible a través del enlace instalado y que la comprobación `onnx_runtime` de `doctor` carga la librería ONNX Runtime empaquetada. Es una puerta E2E parcial automatizada, y parcial en dos sentidos: solo corre sobre Linux, y no paga el coste del modelo (no hay `setup`, ni síntesis, ni descarga).

Fuera del pipeline, la validación se reparte así:

- **Windows**: la realiza el propietario manualmente sobre su equipo local, instalando el artefacto de cada release, ejecutando el recorrido `setup` → `speech synthesize` → desinstalar, y registrando el resultado. El resultado de la validación de `v0.36.0` está en el [criterio 1](#criterios-de-aceptación).
- **Linux y macOS**: dependen de **feedback de usuarios reales** que prueben la instalación y ejecución en sus equipos. Ese feedback (positivo o negativo) es la entrada de issues que cierra el circuito y guía correcciones específicas por plataforma.

El recorrido concreto —la secuencia ordenada de comandos que ejercita toda la superficie de la CLI en este proceso manual— está en [docs/MANUAL-VALIDATION.md](MANUAL-VALIDATION.md).

Por tanto, los criterios 2, 3 y 9 no son "pendientes" en el sentido de tareas olvidadas: son el **borde externo** del proceso de calidad, donde el propietario más el feedback de la comunidad reemplazan a un runner de CI que no podría ejercitar la matriz de hardware/SO real. Cualquier issue reportado en estos criterios se incorpora al ciclo de desarrollo como bug prioritario y motiva fixes versionados.

### Condición de finalización

La implementación está completa únicamente cuando:

- [x] El motor TTS está implementado y funcional (Qwen3-TTS)
- [x] La clonación de voz funciona con una muestra de ~10 segundos
- [x] El audio generado preserva las características de la voz original
- [x] El español latinoamericano suena natural
- [x] El pipeline de CI produce el artefacto de cada SO (Windows, Linux, macOS) y publica los dos bootstrap del one-liner (`install.sh` e `install.ps1`) como assets del release; en la raíz del repositorio no queda ningún script de ciclo de vida, y los que se ejecutan antes del binario viven en `packaging/bootstrap/`
- [ ] Los instaladores funcionan sin ninguna dependencia externa (validación E2E por SO, ver "Validación E2E" arriba: smoke test automatizado en CI + validación manual Windows del propietario + feedback de usuarios reales en Linux y macOS)
- [ ] La experiencia de instalación, uso, actualización y desinstalación es equivalente en los 3 SO: las [brechas conocidas](specs/LIFECYCLE.md#brechas-conocidas) no incluyen ninguna **de código** abierta (las de *firma de código* y *provisión en los canales sin `self install`* permanecen registradas como diferidas al goal a largo plazo y no bloquean esta condición)
- [x] **README.md** resume la arquitectura en un párrafo y remite a docs/DESIGN.md, su único dueño
- [x] **docs/DESIGN.md** corresponde al estado implementado
- [x] El daemon mode está implementado y funciona correctamente
- [x] Los logs están normalizados con estructura consistente
- [x] Los tests `cargo test --all` pasan en los 3 SO (suite Rust)

---

## Goal a largo plazo

Especificaciones **no comprometidas** para el goal inmediato. No se trabajan ahora — cada una registra por qué se difiere (según el [criterio de clasificación](#clasificación-de-specs)) y qué condición la promueve al goal inmediato.

### Firma de código y notarización

**Motivación**: los binarios del canal nativo no están firmados, por lo que Windows SmartScreen y macOS Gatekeeper bloquean el primer arranque cuando el artefacto se descarga por navegador. El mecanismo y la mitigación ya vigente (instaladores de una línea) están explicados en [SECURITY.md](../SECURITY.md#artefactos-sin-firmar); no elimina el bloqueo para la descarga directa desde el navegador.

**Justificación del diferimiento**: la firma es un gate que solo vale la pena cuando el proyecto/producto esté **cristalizado y completo** — idealmente sin bugs y con funcionalidad completa y equivalente entre los 3 sistemas operativos (sin [brechas conocidas](specs/LIFECYCLE.md#brechas-conocidas) abiertas). El producto aún está en desarrollo: firmar ahora significaría re-tramitar la confianza externa (aprobación de SignPath OSS, cuenta Apple Developer de pago) sobre artefactos que siguen cambiando de forma. Solo entonces se iniciará el proceso de firma.

**Especificación diferida**:

- **Windows**: firma Authenticode vía [SignPath Foundation](https://signpath.org/) (firma gratuita para proyectos open source), integrada al job de CI que produce el artefacto `ai-voice-interconnector-X.Y.Z-x86_64-windows.zip` (binario `ai-voice-interconnector.exe` dentro del zip).
- **macOS**: notarización con una cuenta Apple Developer ID, integrada al job de CI que produce el artefacto `ai-voice-interconnector-X.Y.Z-arm64-macos.tar.gz`.

**Condiciones de entrada** (promueven esta spec al goal inmediato):

- El goal inmediato está cumplido: producto cristalizado, con equivalencia funcional completa entre los 3 SO (sin [brechas conocidas](specs/LIFECYCLE.md#brechas-conocidas) de **código** abiertas; las de **experiencia** restantes serían esta misma, de *firma de código*, y la de *provisión en los canales sin `self install`*).
- Windows: aprobación del proyecto por el programa SignPath OSS.
- macOS: alta de una cuenta Apple Developer (de pago).

**Criterio de cierre**: los instaladores de Windows y macOS generados por CI arrancan sin disparar SmartScreen ni Gatekeeper en una instalación limpia, incluso descargados por navegador.

### Provisión de modelos en los canales sin `self install`

**Motivación**: la provisión de modelos la encadena `self install` (su paso 10), que es lo que invocan los dos one-liners (`install.sh` para Linux y macOS, `install.ps1` para Windows). Los otros dos canales documentados —el Cask de Homebrew y la descarga manual del archivo comprimido— no invocan `self install`: instalan el programa y dejan la provisión como un paso explícito que el canal ofrece. En el Cask, ese paso es el `caveat` que remite a `ai-voice-interconnector setup`; en la descarga manual, la línea final del README.

**Justificación del diferimiento**: criterio de clasificación 2 —condición de madurez del producto. La provisión es hoy la fase más larga y más pesada de toda la experiencia (5,85 GB), y encadenarla dentro de `brew install` congelaría el instalador de Homebrew durante la descarga: Homebrew no da a un hook de post-instalación barra de progreso propia, ni timeout, ni estado reanudable, ni aviso de «esto tardará», y una interrupción dejaría el Cask a medias. La diferencia es de **mecanismo**, no de experiencia: la paridad ideal admite `setup` «ofrecido» ([tabla de paridad](#paridad-de-experiencia)), y el `caveat` es exactamente eso — un paso explícito que el usuario ve y puede lanzar cuando quiera.

**Especificación diferida**:

- Los canales que no invocan `self install` ofrecen la provisión con la misma garantía que da `self install`: progreso visible, timeout, reanudación tras una interrupción y estado final verificable con `doctor --json`.
- El mecanismo concreto (un hook de post-instalación, una descarga con progreso gestionada por el canal o una reanudación del propio gestor de modelos) se decide cuando la condición de entrada se cumpla; hoy la elección está abierta y no se prejuzga.

**Condiciones de entrada** (promueven esta spec al goal inmediato):

- El canal ofrece descarga de modelos con progreso visible y estado reanudable: hoy el indicador se silencia con `AVI_NO_PROGRESS=1` y la descarga no es reanudable.
- Existe feedback de usuarios reales de esos canales que justifique el trabajo: el tap de Homebrew no tiene todavía ninguna instalación registrada.

**Criterio de cierre**: tras instalar por cualquiera de los canales, `doctor --json` informa los cinco modelos provisionados sin un paso manual adicional.
