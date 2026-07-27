# Rediseño de la CLI: el grupo `speech` y el contrato de salida

**Estado**: propuesta, sin implementar. Ninguna decisión de este documento está en el código.
**Alcance**: contrato público de la CLI (comandos, flags, códigos de salida, payloads `--json`) y el almacén de habla sintética.
**Base**: commit `26735cd`, el último que toca `src/`. Todo lo descrito en la sección 1 está verificado contra el árbol de trabajo.
**Sustituye a**: `generate-speech-redesign.md`, que se conserva solo por trazabilidad. Este documento es la fuente única; el anterior no debe consultarse para saber qué va a existir.

---

## Tabla de contenidos

- [1. Estado actual](#1-estado-actual)
  - [1.1. Superficie de comandos](#11-superficie-de-comandos)
  - [1.2. `speak` en detalle](#12-speak-en-detalle)
  - [1.3. El contrato de salida](#13-el-contrato-de-salida)
  - [1.4. Los canales legibles por máquina](#14-los-canales-legibles-por-máquina)
  - [1.5. El despacho al daemon](#15-el-despacho-al-daemon)
  - [1.6. El almacén de voces, el sandbox y el vocabulario](#16-el-almacén-de-voces-el-sandbox-y-el-vocabulario)
- [2. Estado objetivo](#2-estado-objetivo)
  - [2.1. Invariantes y criterios generadores](#21-invariantes-y-criterios-generadores)
  - [2.2. Superficie de comandos y vocabulario](#22-superficie-de-comandos-y-vocabulario)
  - [2.3. El grupo `speech`: cinco sub-acciones](#23-el-grupo-speech-cinco-sub-acciones)
  - [2.4. `speech synthesize` y el bucle de `--play`](#24-speech-synthesize-y-el-bucle-de---play)
  - [2.5. El despacho al daemon](#25-el-despacho-al-daemon)
  - [2.6. Reglas de validación](#26-reglas-de-validación)
  - [2.7. Matrices de comportamiento](#27-matrices-de-comportamiento)
  - [2.8. El almacén de habla sintética](#28-el-almacén-de-habla-sintética)
  - [2.9. El contrato de salida](#29-el-contrato-de-salida)
  - [2.10. El canal de error y los payloads `--json`](#210-el-canal-de-error-y-los-payloads---json)
  - [2.11. Cambios en `cleanup`, `setup` y `voice`](#211-cambios-en-cleanup-setup-y-voice)
- [3. El puente](#3-el-puente)
  - [3.1. El orden y por qué](#31-el-orden-y-por-qué)
  - [3.2. Movimiento 1 — limpieza](#32-movimiento-1--limpieza)
  - [3.3. Movimiento 2 — el contrato de salida](#33-movimiento-2--el-contrato-de-salida)
  - [3.4. Movimiento 3 — la feature](#34-movimiento-3--la-feature)
  - [3.5. Puertas de verificación](#35-puertas-de-verificación)
  - [3.6. Documentación pública](#36-documentación-pública)

---

## 1. Estado actual

> ⏳ **Pendiente de redacción.** Descripción del contrato que existe hoy, verificada contra el árbol de trabajo. Donde el estado actual contiene un defecto de hecho —no una preferencia de diseño— se enuncia en una línea, sin análisis.

### 1.1. Superficie de comandos

> ⏳ **Pendiente de redacción.** Tabla de comandos y sub-acciones con su propósito.

### 1.2. `speak` en detalle

> ⏳ **Pendiente de redacción.** Flags, reglas de validación, matriz de comportamiento y forma del payload.

### 1.3. El contrato de salida

> ⏳ **Pendiente de redacción.** Los códigos de salida declarados, el código que vive fuera del bloque principal y sin documentar, la colisión del 2 con argparse, y el recuento de llamadas a `sys.exit()` del paquete.

### 1.4. Los canales legibles por máquina

> ⏳ **Pendiente de redacción.** Reparto de `emit_json()` entre rutas de éxito y de error, el caso en que un fallo deja stdout vacío, y los payloads con clave de estado improvisada.

### 1.5. El despacho al daemon

> ⏳ **Pendiente de redacción.** Las tres ramas de despacho y qué decide cada una.

### 1.6. El almacén de voces, el sandbox y el vocabulario

> ⏳ **Pendiente de redacción.** Estructura del registro de voces, límites del sandbox y la tabla de homonimia del término `speech` en las capas donde aparece.

---

## 2. Estado objetivo

> ⏳ **Pendiente de redacción.** Lo que va a existir. Se lee sin conocer el estado actual ni el camino: esta sección es autosuficiente.

### 2.1. Invariantes y criterios generadores

> ⏳ **Pendiente de redacción.** Ninguna superficie acepta rutas del llamador; una responsabilidad por sub-acción; el eje de dos preguntas que ordena los códigos de salida; la asimetría de reversibilidad.

### 2.2. Superficie de comandos y vocabulario

> ⏳ **Pendiente de redacción.** Comandos y sub-acciones resultantes, más la tabla de resolución del vocabulario.

### 2.3. El grupo `speech`: cinco sub-acciones

> ⏳ **Pendiente de redacción.** Las cinco sub-acciones y los parámetros de cada una.

### 2.4. `speech synthesize` y el bucle de `--play`

> ⏳ **Pendiente de redacción.** Comportamiento de la sub-acción de síntesis y el bucle de aceptación.

### 2.5. El despacho al daemon

> ⏳ **Pendiente de redacción.** Los tres modos de despacho y qué superficies los reciben.

### 2.6. Reglas de validación

> ⏳ **Pendiente de redacción.** Las cinco reglas, con el código de salida de cada una.

### 2.7. Matrices de comportamiento

> ⏳ **Pendiente de redacción.** Las dos matrices, con sus filas de salida legible por máquina.

### 2.8. El almacén de habla sintética

> ⏳ **Pendiente de redacción.** Qué archivo es el recurso de registro, la forma del sidecar y cómo se cierra la ventana entre comprobación y escritura.

### 2.9. El contrato de salida

> ⏳ **Pendiente de redacción.** La tabla de códigos, con el criterio generador que decide a cuál pertenece un fallo nuevo.

### 2.10. El canal de error y los payloads `--json`

> ⏳ **Pendiente de redacción.** Forma de los payloads, reglas de compatibilidad e invariante del canal.

### 2.11. Cambios en `cleanup`, `setup` y `voice`

> ⏳ **Pendiente de redacción.** Qué arrastra cada bandera de limpieza y qué cambia en los otros dos comandos.

---

## 3. El puente

> ⏳ **Pendiente de redacción.** Cómo se llega del estado actual al objetivo, en tres movimientos.

### 3.1. El orden y por qué

> ⏳ **Pendiente de redacción.** El argumento de no-invertibilidad que fija el orden de los tres cortes.

### 3.2. Movimiento 1 — limpieza

> ⏳ **Pendiente de redacción.** Pasos del movimiento, con su verificación.

### 3.3. Movimiento 2 — el contrato de salida

> ⏳ **Pendiente de redacción.** Pasos del movimiento, con la tabla de reclasificación de códigos.

### 3.4. Movimiento 3 — la feature

> ⏳ **Pendiente de redacción.** Pasos del movimiento, con su verificación.

### 3.5. Puertas de verificación

> ⏳ **Pendiente de redacción.** Una puerta por movimiento, con sus comprobaciones ejecutables.

### 3.6. Documentación pública

> ⏳ **Pendiente de redacción.** Qué cambia en `USAGE.md`, `docs/DAEMON-MODE.md`, `docs/NARRATION-INTEGRATION.md` y `CHANGELOG.md`.
