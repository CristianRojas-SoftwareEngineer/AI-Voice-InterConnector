# `doctor`

Diagnóstico integral del sistema y del estado del ciclo de vida, **sin efectos secundarios salvo `--repair`**: sin flags no descarga, no instala, no borra y no inicia procesos. No es un checker de recursos para bootstrap: incluye precondiciones de instalación, receipt, `PATH`, residuos y modelos, también en canal `unmanaged`. Por eso, en ese canal, los fallos de receipt/PATH son parte del veredicto y producen exit 1; la gate de bootstrap debe validar recursos por separado.

Emite un **veredicto** —texto o JSON— y termina con exit code **1** si algún chequeo falla.

**Implementación:** el motor es el módulo `doctor` de `avi-lifecycle` (`crates/avi-lifecycle/src/doctor.rs`), que devuelve el reporte **como dato serializable** y sin imprimir nada. `src/main.rs` (`handle_doctor`) solo lo estampa, decide el código de salida y compone el veredicto.

**Por qué el motor no imprime.** El contrato de la CLI exige que cada invocación emita **exactamente un objeto JSON**, y que la salida por veredicto de `doctor` —código ≠ 0 con el reporte ya emitido y **sin** objeto `error` detrás— no lleve un segundo objeto. Eso solo es posible si el veredicto es un **dato** que el binario compone, y no una impresión con un `exit` detrás. En el motor no hay `println!` ni `exit`: hay una función que devuelve el reporte con su veredicto dentro.

---

## Definición CLI (parser)

```
ai-voice-interconnector doctor [--repair] [--json]
```

`Doctor` admite un solo flag propio, `--repair`: ejecuta la recuperación tomando el bloqueo (completa o revierte la transacción pendiente y barre los residuos de dueño muerto: aparcados, stagings, scratch, temporales y registros de borrado resueltos) y vuelve a evaluar el informe sobre lo que quedó. Lo que la recuperación conserva a propósito —residuos de dueño no observable y borrados diferidos sin completar— sigue en `pending` tras `--repair` y mantiene fallando `pending_artifacts` hasta que se resuelva por otra vía. Sin flags no admite subcomandos. Hereda el flag global `--json`. `doctor` no declara `--daemon` ni `--no-daemon` (el parser los rechaza como argumento desconocido) porque nunca dialoga con el daemon.

---

## Las nueve claves del envelope

| Clave | Contenido |
|---|---|
| `version` | Versión del binario en ejecución |
| `target` | Tripla del target del host |
| `channel` | `script`, `homebrew` o `unmanaged` |
| `install` | `dir` (directorio de programa), `data_dir` (raíz de datos efectiva), `receipt` (`valid`/`absent`/`invalid`) y `version` (la del recibo, si lo hay) |
| `path` | `resolves_to_this_install`, `duplicate_entries`, `integration` (`present`/`absent`/`not_modified`) y `coexisting` (instalaciones ajenas, con su canal y cuál tiene precedencia) |
| `pending` | `transaction_journal` (booleano) y ocho listas de rutas: `parked`, `stagings`, `scratch`, `temporaries` y `failure_logs` (residuos de dueño muerto que el barrido borraría), `unknown_owner` (residuos de cualquier familia cuyo dueño no se puede observar: se conservan y se informan, sin poder afirmar que estén huérfanos), `pending_failures` (registros `*.removal-failed.log` cuyo destino sigue existiendo: el borrado diferido no se completó) y `unreadable` (directorios del barrido que existen y no se pueden listar; en texto salen una línea por ruta). Cualquier lista no vacía o el diario presente hace fallar `pending_artifacts` |
| `models` | `root`, `shared_root`, `provisioned`, `missing` (repos de la selección sin provisionar) y `size_bytes` |
| `checks` | Una entrada `{name, ok, detail}` por comprobación |
| `failed` | Los `name` de las comprobaciones que fallan — **es el veredicto** |

`doctor` a secas emite estas nueve claves y `schema_version`, sin `status`. Con `--repair` el envelope añade `status: "repaired"`, `context: {}` y las claves comunes `total_ms`, `stages` (una sola, `repairing`) y `warnings`.

`schema_version` lo inyecta `emit_raw_json`; ver [las dos versiones de esquema](../CONTRACT.md#las-dos-versiones-de-esquema).

---

## Los chequeos

`checks` y `failed` son el veredicto, y `failed` no está vacío si y solo si hay algún `ok: false`. `onnx_runtime` solo aparece en los builds con `native-stt`:

| `name` | Falla cuando | Qué mira |
|---|---|---|
| `install_receipt` | No hay recibo en el directorio de programa, o existe pero es ilegible (el detalle nombra el fichero) | Recibo válido |
| `path_resolves` | La **primera** entrada del `PATH` que apunta a una instalación no es la registrada | Orden de precedencia |
| `path_duplicates` | Más de una entrada del `PATH` apunta a una instalación | Coexistencia (Cask + `script`) |
| `pending_artifacts` | Hay diario de transacción, o alguna lista de `pending` no está vacía (residuos de dueño muerto, de dueño no observable, borrados diferidos sin completar o directorios sin poder listar) | Transacción interrumpida y residuos por identidad |
| `models_provisioned` | Falta algún repo de la selección, el modelo Base de clonado incluido | Integridad mínima definida por `avi_store::ModelStore`, incluidos los ficheros críticos del pin |
| `onnx_runtime` | La librería de ONNX Runtime del directorio real del programa (o la de `ORT_DYLIB_PATH`) no existe o no carga; el detalle nombra la ruta buscada | Solo en binarios con STT (`native-stt`): carga la librería, sin modelos |

**El modelo Base de clonado no tiene un chequeo propio.** `qwen3-tts-0.6b-base` es un modelo obligatorio como los demás: su ausencia aparece en `models.missing` y hace fallar `models_provisioned`.

**Nada de esto escribe en disco, salvo `--repair`.** La recuperación de las [reglas transversales](../../specs/LIFECYCLE.md#81-reglas-transversales) se ejecuta aquí en **modo informe**: `doctor` **calcula** lo que la recuperación haría —con la misma decisión que usa el barrido real— y lo publica en `pending`, sin tomar el bloqueo y sin modificar nada. Barrer de verdad desde un diagnóstico convertiría el comando más inocuo del producto en uno que borra temporales de la máquina que lo invoca. La excepción es `--repair`, que toma el bloqueo, ejecuta ese mismo barrido y reevalúa: su alcance es exactamente lo que el barrido recoge, ni más ni menos.

---

## Un solo objeto, también cuando falla

Este es el punto donde el contrato es más estricto y donde el comportamiento se decidió a propósito:

- Con `--json`, `doctor` emite **un único objeto**: el reporte, con el veredicto dentro (`checks` y `failed`).
- Si algún chequeo falla, el código de salida es **1** y **no se adjunta el objeto `error` detrás**. Es una **salida por veredicto** (`Outcome::Verdict`), no un error: el comando corrió sin error y su resultado es negativo.
- Devolver `Err(CliError)` desde aquí haría que `main` escribiera un segundo objeto `error` detrás del reporte y el envelope sería ilegible, que es exactamente el defecto que «cada invocación emite exactamente un objeto JSON» prohíbe.

Sin `--json`, todas las comprobaciones salen por stdout, bajo la cabecera `Diagnóstico:`, una por línea: `  ✓ <etiqueta>: <detalle>` si pasa y `  ✗ <etiqueta>: <detalle> → <indicación>` si falla. Las etiquetas están en español (`Recibo de instalación`, `Comando en el PATH`, `PATH sin duplicados`, `Restos pendientes`, `Modelos`, `ONNX Runtime`) y los identificadores internos (`install_receipt`, `path_resolves`…) solo salen en `--json`. La indicación de una comprobación fallida lleva la orden concreta: ``ejecuta `ai-voice-interconnector setup` `` para los modelos y ``ejecuta `ai-voice-interconnector doctor --repair` `` para los restos pendientes. No hay una línea «Pista» aparte.

El veredicto sale por **stderr** en los dos modos, y `--json` solo cambia stdout. Sin fallos escribe `✓ Diagnóstico sin problemas.` (solo sin `--repair`). Con fallos escribe `Error: N comprobaciones fallan.` seguido de `Solución:`, que reúne las órdenes de las comprobaciones fallidas que tienen indicación (sin repetirlas y en el orden del informe), por ejemplo ``Solución: Ejecuta `ai-voice-interconnector doctor --repair`; ejecuta `ai-voice-interconnector setup`.``; si ninguna trae orden, la solución es `Revisa las comprobaciones marcadas con ✗ en el diagnóstico.`. En `--json` stdout lleva solo el reporte, y en los dos modos el comando sale con 1.

`doctor --repair` escribe la cabecera `Reparación de los restos pendientes`, ejecuta su etapa única `repairing` («Reparando los restos…»), cierra con `✓ Reparación completada en <total>` y, en este orden, después imprime las comprobaciones por stdout y por último el separador y el `Error:` con su `Solución:` si algo sigue fallando. Con una sola etapa, la línea `✓` de la etapa no se escribe: queda solo el cierre.

---

## Contrato `--json` (ejemplo)

```json
{
  "schema_version": "7",
  "version": "X.Y.Z",
  "target": "x86_64-pc-windows-msvc",
  "channel": "script",
  "install": {
    "dir": "<directorio-de-programa>",
    "data_dir": "<raíz-de-datos>",
    "receipt": "valid",
    "version": "X.Y.Z"
  },
  "path": {
    "resolves_to_this_install": true,
    "duplicate_entries": [],
    "integration": "present",
    "coexisting": []
  },
  "pending": {
    "transaction_journal": false,
    "parked": [],
    "stagings": [],
    "scratch": [],
    "temporaries": [],
    "failure_logs": [],
    "unknown_owner": [],
    "pending_failures": [],
    "unreadable": []
  },
  "models": {
    "root": "<raíz-de-modelos>",
    "shared_root": false,
    "provisioned": ["opus-mt-es-en", "opus-mt-en-es", "parakeet-tdt-v3", "dpdfnet8", "qwen3-tts-0.6b-base"],
    "missing": [],
    "size_bytes": 0
  },
  "checks": [
    { "name": "install_receipt", "ok": true, "detail": "recibo válido en …" }
  ],
  "failed": []
}
```

**`integration` pregunta al registro, no al `PATH` del proceso**, en Windows: el registro es lo que sobrevive a la sesión. En Unix pregunta por el enlace, y no necesita el `PATH` porque la integración es el archivo de arranque. `not_modified` es el valor de `--no-modify-path`, y no es un fallo: el recibo registra que nunca se integró nada, y por eso `self uninstall` no toca ningún perfil.

**El directorio de programa registrado puede no estar en el `PATH` de la sesión** —se invoca por su ruta completa— y aun así es la instalación a la que se opera; en ese caso se informa igualmente en `coexisting`, y sin precedencia.

---

## Errores

| Situación | Código | `reason` |
|---|---|---|
| Algún chequeo falló | **1** | — (no hay `reason`: el veredicto va en `checks`/`failed`; por stderr, `Error:` y `Solución:`) |
| No se pudo resolver el ejecutable en ejecución | 1 | `doctor_failed` |

**`doctor` no tiene `reason` de contrato propios.** Un fallo de chequeo no es un error de la operación: es el dictamen que el comando existe para dar. Un recibo ilegible se informa como `install_receipt` fallido (el detalle nombra el fichero) y el veredicto sale con exit 1. La única excepción es `doctor_failed`, que es un fallo real del comando (no se pudo resolver `current_exe` o no se pudo serializar el reporte) y sale por el canal de error normal. `--repair` no añade códigos ni `reason`: tras barrer, el informe se reevalúa y el exit sigue al veredicto (0 si todo quedó limpio, 1 si algo sigue fallando).

---

## Ejemplos

```bash
ai-voice-interconnector doctor                       # reporte en texto, exit 0 o 1
ai-voice-interconnector doctor --repair              # barre los restos pendientes y reevalúa
ai-voice-interconnector --json doctor                # envelope con las nueve claves
ai-voice-interconnector --json doctor | jq .failed   # solo los chequeos que fallan
```

---

## Muestras de salida

Salidas reales del binario, en una máquina sin instalación registrada ni modelos descargados. Las duraciones son orientativas.

**`doctor`** (en la terminal; sale con 1):

```
Diagnóstico:
  ✗ Recibo de instalación: no hay recibo de instalación
  ✗ Comando en el PATH: el comando no resuelve a la instalación registrada
  ✓ PATH sin duplicados: sin entradas duplicadas en el PATH
  ✓ Restos pendientes: sin residuos de operaciones anteriores
  ✗ Modelos: faltan: opus-mt-es-en, opus-mt-en-es, parakeet-tdt-v3, dpdfnet8, qwen3-tts-0.6b-base → ejecuta `ai-voice-interconnector setup`
  ✓ ONNX Runtime: ONNX Runtime cargada desde %LOCALAPPDATA%\Programs\ai-voice-interconnector\onnxruntime.dll
Error: 3 comprobaciones fallan.
Solución: Ejecuta `ai-voice-interconnector setup`.
```

**`--json doctor`** (sale con 1). stdout lleva el reporte:

```json
{
  "channel": "unmanaged",
  "checks": [
    {
      "detail": "no hay recibo de instalación",
      "name": "install_receipt",
      "ok": false
    },
    {
      "detail": "el comando no resuelve a la instalación registrada",
      "name": "path_resolves",
      "ok": false
    },
    {
      "detail": "sin entradas duplicadas en el PATH",
      "name": "path_duplicates",
      "ok": true
    },
    {
      "detail": "sin residuos de operaciones anteriores",
      "name": "pending_artifacts",
      "ok": true
    },
    {
      "detail": "faltan: opus-mt-es-en, opus-mt-en-es, parakeet-tdt-v3, dpdfnet8, qwen3-tts-0.6b-base",
      "name": "models_provisioned",
      "ok": false
    },
    {
      "detail": "ONNX Runtime cargada desde %LOCALAPPDATA%\\Programs\\ai-voice-interconnector\\onnxruntime.dll",
      "name": "onnx_runtime",
      "ok": true
    }
  ],
  "failed": [
    "install_receipt",
    "path_resolves",
    "models_provisioned"
  ],
  "install": {
    "data_dir": "%LOCALAPPDATA%\\ai-voice-interconnector\\data",
    "dir": "%LOCALAPPDATA%\\Programs\\ai-voice-interconnector",
    "receipt": "absent",
    "version": null
  },
  "models": {
    "missing": [
      "opus-mt-es-en",
      "opus-mt-en-es",
      "parakeet-tdt-v3",
      "dpdfnet8",
      "qwen3-tts-0.6b-base"
    ],
    "provisioned": [],
    "root": "%LOCALAPPDATA%\\ai-voice-interconnector\\cache\\models",
    "shared_root": false,
    "size_bytes": 0
  },
  "path": {
    "coexisting": [
      {
        "channel": "unmanaged",
        "path": "C:\\Tools\\ai-voice-interconnector",
        "takes_precedence": true
      },
      {
        "channel": "unmanaged",
        "path": "%LOCALAPPDATA%\\Programs\\ai-voice-interconnector",
        "takes_precedence": false
      }
    ],
    "duplicate_entries": [],
    "integration": "not_modified",
    "resolves_to_this_install": false
  },
  "pending": {
    "failure_logs": [],
    "parked": [],
    "pending_failures": [],
    "scratch": [],
    "stagings": [],
    "temporaries": [],
    "transaction_journal": false,
    "unknown_owner": [],
    "unreadable": []
  },
  "schema_version": "7",
  "target": "x86_64-pc-windows-msvc",
  "version": "0.47.0"
}
```

y stderr lleva el veredicto:

```
Error: 3 comprobaciones fallan.
Solución: Ejecuta `ai-voice-interconnector setup`.
```

**`doctor --repair`** (en la terminal; sale con 1). El orden es cierre, comprobaciones y `Error:`:

```
Reparación de los restos pendientes
✓ Reparación completada en 0,0 s
Diagnóstico:
  ✗ Recibo de instalación: no hay recibo de instalación
  ✗ Comando en el PATH: el comando no resuelve a la instalación registrada
  ✓ PATH sin duplicados: sin entradas duplicadas en el PATH
  ✓ Restos pendientes: sin residuos de operaciones anteriores
  ✗ Modelos: faltan: opus-mt-es-en, opus-mt-en-es, parakeet-tdt-v3, dpdfnet8, qwen3-tts-0.6b-base → ejecuta `ai-voice-interconnector setup`
  ✓ ONNX Runtime: ONNX Runtime cargada desde %LOCALAPPDATA%\Programs\ai-voice-interconnector\onnxruntime.dll
──────────────────────────────────────────────────
Error: 3 comprobaciones fallan.
Solución: Ejecuta `ai-voice-interconnector setup`.
```

**`--json doctor --repair`** (sale con 1). El envelope añade `context`, `stages`, `status`, `total_ms` y `warnings` a las nueve claves:

```json
{
  "channel": "unmanaged",
  "checks": [
    {
      "detail": "no hay recibo de instalación",
      "name": "install_receipt",
      "ok": false
    },
    {
      "detail": "el comando no resuelve a la instalación registrada",
      "name": "path_resolves",
      "ok": false
    },
    {
      "detail": "sin entradas duplicadas en el PATH",
      "name": "path_duplicates",
      "ok": true
    },
    {
      "detail": "sin residuos de operaciones anteriores",
      "name": "pending_artifacts",
      "ok": true
    },
    {
      "detail": "faltan: opus-mt-es-en, opus-mt-en-es, parakeet-tdt-v3, dpdfnet8, qwen3-tts-0.6b-base",
      "name": "models_provisioned",
      "ok": false
    },
    {
      "detail": "ONNX Runtime cargada desde %LOCALAPPDATA%\\Programs\\ai-voice-interconnector\\onnxruntime.dll",
      "name": "onnx_runtime",
      "ok": true
    }
  ],
  "context": {},
  "failed": [
    "install_receipt",
    "path_resolves",
    "models_provisioned"
  ],
  "install": {
    "data_dir": "%LOCALAPPDATA%\\ai-voice-interconnector\\data",
    "dir": "%LOCALAPPDATA%\\Programs\\ai-voice-interconnector",
    "receipt": "absent",
    "version": null
  },
  "models": {
    "missing": [
      "opus-mt-es-en",
      "opus-mt-en-es",
      "parakeet-tdt-v3",
      "dpdfnet8",
      "qwen3-tts-0.6b-base"
    ],
    "provisioned": [],
    "root": "%LOCALAPPDATA%\\ai-voice-interconnector\\cache\\models",
    "shared_root": false,
    "size_bytes": 0
  },
  "path": {
    "coexisting": [
      {
        "channel": "unmanaged",
        "path": "C:\\Tools\\ai-voice-interconnector",
        "takes_precedence": true
      },
      {
        "channel": "unmanaged",
        "path": "%LOCALAPPDATA%\\Programs\\ai-voice-interconnector",
        "takes_precedence": false
      }
    ],
    "duplicate_entries": [],
    "integration": "not_modified",
    "resolves_to_this_install": false
  },
  "pending": {
    "failure_logs": [],
    "parked": [],
    "pending_failures": [],
    "scratch": [],
    "stagings": [],
    "temporaries": [],
    "transaction_journal": false,
    "unknown_owner": [],
    "unreadable": []
  },
  "schema_version": "7",
  "stages": [
    {
      "end_ms": 0,
      "phase": null,
      "stage": "repairing",
      "start_ms": 0
    }
  ],
  "status": "repaired",
  "target": "x86_64-pc-windows-msvc",
  "total_ms": 0,
  "version": "0.47.0",
  "warnings": []
}
```

y stderr, que sin terminal anuncia la etapa:

```
Reparación de los restos pendientes
● Reparando los restos…
✓ Reparación completada en 0,0 s
──────────────────────────────────────────────────
Error: 3 comprobaciones fallan.
Solución: Ejecuta `ai-voice-interconnector setup`.
```
