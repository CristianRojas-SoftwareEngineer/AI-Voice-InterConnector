# `doctor`

Diagnóstico del sistema y del estado del ciclo de vida, **sin efectos secundarios salvo `--repair`**: sin flags no descarga, no instala, no borra y no inicia procesos. Es la única operación del ciclo de vida que el usuario puede ejecutar sin riesgo (en su modo sin flags), y por eso es donde el estado de la instalación tiene que ser legible: versión y target, canal, instalación y recibo, `PATH` con duplicados y precedencia, pendientes de una operación anterior y modelos.

Emite un **veredicto** —texto o JSON— y termina con exit code **1** si algún chequeo falla.

**Implementación:** el motor es el módulo `doctor` de `avi-lifecycle` (`crates/avi-lifecycle/src/doctor.rs`), que devuelve el reporte **como dato serializable** y sin imprimir nada. `src/main.rs` (`handle_doctor`) solo lo estampa, decide el código de salida y compone el veredicto.

**Por qué el motor no imprime.** El contrato de la CLI exige que cada invocación emita **exactamente un objeto JSON**, y que la salida por veredicto de `doctor` —código ≠ 0 con el reporte ya emitido y **sin** objeto `error` detrás— no lleve un segundo objeto. Eso solo es posible si el veredicto es un **dato** que el binario compone, y no una impresión con un `exit` detrás. En el motor no hay `println!` ni `exit`: hay una función que devuelve el reporte con su veredicto dentro.

---

## Definición CLI (parser)

```
ai-voice-interconnector doctor [--repair] [--json]
```

`Doctor` admite un solo flag propio, `--repair`: recoge los restos pendientes (aparcados, stagings y temporales, solo lo que el barrido recoge) tomando el bloqueo, y vuelve a evaluar el informe sobre lo que quedó. Sin flags no admite subcomandos. Hereda el flag global `--json`. `doctor` no declara `--daemon` ni `--no-daemon` (el parser los rechaza como argumento desconocido) porque nunca dialoga con el daemon.

---

## Las nueve claves del envelope

| Clave | Contenido |
|---|---|
| `version` | Versión del binario en ejecución |
| `target` | Tripla del target del host |
| `channel` | `script`, `homebrew` o `unmanaged` |
| `install` | `dir` (directorio de programa), `data_dir` (raíz de datos efectiva), `receipt` (`valid`/`absent`/`invalid`) y `version` (la del recibo, si lo hay) |
| `path` | `resolves_to_this_install`, `duplicate_entries`, `integration` (`present`/`absent`/`not_modified`) y `coexisting` (instalaciones ajenas, con su canal y cuál tiene precedencia) |
| `pending` | `transaction_journal`, `parked`, `stagings`, `temporaries`, `temporaries_kept` y `unreadable` (directorios del barrido que existen y no se pueden listar; hacen fallar `pending_artifacts` y en texto salen una línea por ruta) |
| `models` | `root`, `shared_root`, `provisioned`, `missing` (repos de la selección sin provisionar) y `size_bytes` |
| `checks` | Una entrada `{name, ok, detail}` por comprobación |
| `failed` | Los `name` de las comprobaciones que fallan — **es el veredicto** |

`schema_version` lo inyecta `emit_raw_json` y vale **`"5"`**. El protocolo del daemon va por `"5"`: son contratos independientes y este no lo toca.

---

## Los cinco chequeos

`checks` y `failed` son el veredicto, y `failed` no está vacío si y solo si hay algún `ok: false`:

| `name` | Falla cuando | Qué mira |
|---|---|---|
| `install_receipt` | No hay recibo en el directorio de programa, o existe pero es ilegible (el detalle nombra el fichero) | Recibo válido |
| `path_resolves` | La **primera** entrada del `PATH` que apunta a una instalación no es la registrada | Orden de precedencia |
| `path_duplicates` | Más de una entrada del `PATH` apunta a una instalación | Coexistencia (Cask + `script`) |
| `pending_artifacts` | Quedan aparcados, stagings o temporales por recoger | Transacción interrumpida |
| `models_provisioned` | Falta algún repo de la selección, el modelo Base de clonado incluido | Solo presencia de snapshot, con los ficheros críticos |
| `onnx_runtime` | La librería de ONNX Runtime del directorio real del programa (o la de `ORT_DYLIB_PATH`) no existe o no carga; el detalle nombra la ruta buscada | Solo en binarios con STT (`native-stt`): carga la librería, sin modelos |

**El modelo Base de clonado no tiene un chequeo propio.** `qwen3-tts-0.6b-base` es un modelo obligatorio como los demás: su ausencia aparece en `models.missing` y hace fallar `models_provisioned`.

**Nada de esto escribe en disco, salvo `--repair`.** La recuperación de §8.1 se ejecuta aquí en **modo informe**: `doctor` **calcula** lo que la recuperación haría —con la misma decisión que usa el barrido real— y lo publica en `pending`, sin tomar el bloqueo y sin modificar nada. Barrer de verdad desde un diagnóstico convertiría el comando más inocuo del producto en uno que borra temporales de la máquina que lo invoca. La excepción es `--repair`, que toma el bloqueo, ejecuta ese mismo barrido y reevalúa: su alcance es exactamente lo que el barrido recoge, ni más ni menos.

---

## Un solo objeto, también cuando falla

Este es el punto donde el contrato es más estricto y donde el comportamiento se decidió a propósito:

- Con `--json`, `doctor` emite **un único objeto**: el reporte, con el veredicto dentro (`checks` y `failed`).
- Si algún chequeo falla, el código de salida es **1** y **no se adjunta el objeto `error` detrás**. Es una **salida por veredicto** (`Salida::Veredicto`), no un error: el comando corrió sin error y su resultado es negativo.
- Devolver `Err(CliError)` desde aquí haría que `main` escribiera un segundo objeto `error` detrás del reporte y el envelope sería ilegible, que es exactamente el defecto que «cada invocación emite exactamente un objeto JSON» prohíbe.

Sin `--json`, los chequeos fallidos van a stderr con prefijo `✗` y la línea de veredicto detrás; si entre ellos está `pending_artifacts`, se añade la pista de recoger los restos con `doctor --repair`. Si todo pasa, la lista completa con `✓` va a stdout.

---

## Contrato `--json` (ejemplo)

```json
{
  "schema_version": "5",
  "version": "0.23.1",
  "target": "x86_64-pc-windows-msvc",
  "channel": "script",
  "install": {
    "dir": "C:\\Users\\…\\AppData\\Local\\Programs\\ai-voice-interconnector",
    "data_dir": "C:\\Users\\…\\AppData\\Local\\ai-voice-interconnector\\data",
    "receipt": "valid",
    "version": "0.23.1"
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
    "temporaries": [],
    "temporaries_kept": [],
    "unreadable": []
  },
  "models": {
    "root": "C:\\Users\\…\\AppData\\Local\\ai-voice-interconnector\\cache\\models",
    "shared_root": false,
    "provisioned": ["qwen3-tts-0.6b", "opus-mt-es-en", "opus-mt-en-es", "parakeet-tdt-v3", "qwen3-tts-0.6b-base"],
    "missing": [],
    "size_bytes": 9876543210
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
| Algún chequeo falló | **1** | — (no hay `reason`: el veredicto va en `checks`/`failed`) |
| No se pudo resolver el ejecutable en ejecución | 1 | `doctor_failed` |

**`doctor` no tiene `reason` de contrato propios.** Un fallo de chequeo no es un error de la operación: es el dictamen que el comando existe para dar. La única excepción es `doctor_failed`, que es un fallo real del comando (no se pudo resolver `current_exe`, el recibo de la instalación registrada es ilegible —el mensaje nombra el fichero— o no se pudo serializar el reporte) y sale por el canal de error normal. `--repair` no añade códigos ni `reason`: tras barrer, el informe se reevalúa y el exit sigue al veredicto (0 si todo quedó limpio, 1 si algo sigue fallando).

---

## Ejemplos

```bash
ai-voice-interconnector doctor                       # reporte en texto, exit 0 o 1
ai-voice-interconnector doctor --repair              # barre los restos pendientes y reevalúa
ai-voice-interconnector --json doctor                # envelope con las nueve claves
ai-voice-interconnector --json doctor | jq .failed   # solo los chequeos que fallan
```
