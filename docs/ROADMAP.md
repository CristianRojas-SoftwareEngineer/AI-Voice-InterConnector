# Roadmap: estado actual y camino al Goal inmediato

Este documento es el **registro vivo** del estado de implementación de
`tts-sidecar` y del trabajo pendiente para alcanzar el **Goal inmediato**, cuya
especificación, criterios de aceptación y condición de finalización viven en
[docs/GOAL.md](GOAL.md). El **Goal a largo plazo** (firma de código /
notarización) también se define en
[docs/GOAL.md](GOAL.md#goal-a-largo-plazo).

Mantener el estado y el roadmap aquí (separados de la especificación ideal)
permite que [docs/GOAL.md](GOAL.md) siga siendo la fuente de verdad del *qué* y
el *cuándo está hecho*, mientras este archivo responde al *dónde estamos* y *qué
falta*.

## Tabla de contenidos

- [Estado actual](#estado-actual)
- [Trabajo pendiente (roadmap al Goal inmediato)](#trabajo-pendiente-roadmap-al-goal-inmediato)
- [Plan técnico: brecha de *desinstalación en un comando* — EJECUTADO (v0.6.0)](#plan-técnico-brecha-de-desinstalación-en-un-comando--ejecutado-v060)
- [Hacia el Goal inmediato](#hacia-el-goal-inmediato)

## Estado actual

**Implementado y verificable en el repo** (la validación end-to-end de los
instaladores por SO es externa al pipeline por diseño; ver la «Decisión de
validación E2E» en
[docs/GOAL.md](GOAL.md#validación-e2e)):

- Motor Chatterbox Multilingual V3 implementado (Python)
- Sistema de audio playback nativo por SO (pycaw/winsound/sounddevice/afplay)
- Daemon mode con IPC HTTP (FastAPI, puerto 8765)
- Optimizaciones de síntesis (n_cfm=4, max_new_tokens=500)
- Bypass del watermark PerthNet: el audio generado no lleva marca de agua (ver «Uso ético y responsable» en README/USAGE)
- Scripts de build PyInstaller por SO (Windows/Linux/macOS)
- **Canal PyPI** (`uv tool install tts-sidecar` / `pipx install tts-sidecar`), publicado automáticamente en cada tag `v*` junto al canal nativo (ver [docs/DISTRIBUTION.md](DISTRIBUTION.md))
- Descarga automática del modelo Chatterbox desde HuggingFace
- CLI completa con todos los comandos
- **Rediseño del CLI cerrado** (v0.7.0–v0.9.0): `speak` se reemplazó por `speech say`/`speech synthesize`, los códigos de salida se centralizaron en `exit_codes.py` y los payloads `--json` incorporaron la clave `error`; el contrato normativo vive en [docs/CLI-CONTRACT.md](CLI-CONTRACT.md) y el detalle en el CHANGELOG
- **Instalación auto-hospedada de una línea por SO** (Linux y Cask de macOS en v0.3.0; Windows en v0.4.0; one-liner macOS `install-macos.sh` en v0.5.0): `install-linux.sh` (`curl | sh`) en Linux, `install-macos.sh` (`curl | sh`, sin Homebrew ni `sudo`) y el Cask de Homebrew propio en macOS, e `install-windows.ps1` (`irm | iex`) en Windows (instalador Inno Setup per-user, sin UAC; entró en alcance al refutarse la premisa de SmartScreen — la descarga por CLI no aplica el Mark-of-the-Web). Todos los canales publican de forma autónoma, sin aprobación ni pull request a terceros. Ver [docs/SELF-HOSTED-INSTALL.md](SELF-HOSTED-INSTALL.md)
- **Paridad de experiencia entre los 3 SO** (v0.6.0): cerradas a nivel de código/scripts/tests **las siete** brechas accionables de [docs/PARITY.md](PARITY.md) — las seis de v0.5.0 (one-liner macOS, `.command` sin `sudo`, limpieza de AppImages, `setup --uninstall` en Linux, `zap` del Cask completo, README con las tres plataformas) más la de *desinstalación en un comando* (`setup --uninstall` multiplataforma en macOS/Windows, cerrada en v0.6.0). Queda **una sola brecha abierta**, la de *firma de código* (SmartScreen/Gatekeeper, binarios sin firmar, cross-SO), diferida al goal a largo plazo
- Tests pytest (666 tests: timing, protocolo, daemon, CLI, voces, rutas, caché de modelo, audio, Cask y utilidades de build), más los smoke-tests de instaladores (bats Linux/macOS y Pester Windows) en CI
- Documentación sincronizada

## Trabajo pendiente (roadmap al Goal inmediato)

**Todas** las brechas accionables de [docs/PARITY.md](PARITY.md) están cerradas
a nivel de código/scripts/tests: las seis de v0.5.0 más la de *desinstalación en
un comando* (`setup --uninstall` multiplataforma en macOS/Windows), cerrada en
v0.6.0 (plan técnico ejecutado, ver la sección siguiente). Solo la brecha de
*firma de código* (cross-SO) sigue diferida al goal a largo plazo. **No queda
código pendiente del goal inmediato**: la **marca de los criterios de aceptación
10, 1-3 y 9** depende ahora solo de la validación por feedback de usuarios reales
en Linux y macOS (la validación E2E automatizable ya corre en CI; ver
«Validación E2E» en [docs/GOAL.md](GOAL.md#validación-e2e)).

## Plan técnico: brecha de *desinstalación en un comando* — EJECUTADO (v0.6.0)

La brecha de *desinstalación en un comando* (`setup --uninstall`
multiplataforma en macOS/Windows) quedó **cerrada en v0.6.0**: el despachador
`_uninstall` y las ramas `_uninstall_macos`/`_uninstall_windows` viven en
`src/tts_sidecar/cli.py`, la suite `TestSetupUninstall` cubre los tres SO en
verde y la documentación relacionada quedó sincronizada. Solo queda pendiente
la marca del criterio de aceptación 10 por validación E2E vía feedback de
usuarios reales en Linux y macOS. El plan de implementación detallado que guio
este trabajo es historia y vive en el control de versiones (ver el CHANGELOG y
el commit que cerró la brecha), no en este roadmap.

## Hacia el Goal inmediato

El objetivo, los [Criterios de aceptación](GOAL.md#criterios-de-aceptación) y la
[Condición de finalización](GOAL.md#condición-de-finalización) que definen la
meta están en [docs/GOAL.md](GOAL.md). La brecha de *desinstalación en un
comando* quedó **cerrada en código** (v0.6.0), así que **no queda trabajo de
implementación pendiente del Goal inmediato**: solo falta marcar los criterios
pendientes vía feedback de usuarios reales en Linux y macOS. Cuando eso ocurra,
el Goal inmediato se considera cumplido y solo la brecha de *firma de código*
(cross-SO) queda como pieza diferida al Goal a largo plazo.
