# Diseño e implementación del paralelismo y aislamiento de los tests — revisión

- **Fecha**: 2026-09-21
- **Alcance**: la estrategia de concurrencia de la suite de tests (`tests/cli_golden.rs`, `crates/avi-daemon/tests/golden.rs`) y las costuras del producto que la habilitan o la bloquean (`src/main.rs`, `crates/avi-daemon/src/lib.rs`, `crates/avi-tts/src/lib.rs`, `crates/avi-config/src/lib.rs`, `crates/avi-store/src/lib.rs`). No cubre defectos funcionales del producto (ver el consolidado de hallazgos pendientes).
- **Método**: lectura del código, no ejecución. Cada afirmación se ancla a un símbolo (función/constante) y, donde ayuda, a una línea aproximada. Las líneas pueden derivar con el tiempo; el símbolo es el ancla estable.
- **Tesis**: el harness no tiene un defecto de paralelismo aislado, sino **dos defectos estructurales de origen** que compensa en el borde con andamiaje defensivo. Corregir el origen elimina cinco de los seis mecanismos defensivos; el sexto se reduce a su núcleo físico legítimo.

> **Leyenda de severidad**: 🔴 **Crítica** — el defecto puede pasar la suite en verde ocultando un fallo real, o corromper corridas ajenas · 🟠 **Alta** — flake reproducible o andamiaje que enmascara la causa raíz · 🟡 **Media** — acoplamiento que impide el paralelismo seguro pero hoy contenido por serialización · ⚪ **Baja** — deuda de forma o de segregación.

## Tabla de contenidos

- [1. Resumen ejecutivo](#1-resumen-ejecutivo)
- [2. Hipótesis retirada](#2-hipótesis-retirada)
- [3. Los dos defectos estructurales](#3-los-dos-defectos-estructurales)
  - [D-I — Dominio del recurso ≠ dominio del candado](#d-i--dominio-del-recurso--dominio-del-candado)
  - [D-II — Arranque sin señal de readiness](#d-ii--arranque-sin-señal-de-readiness)
- [4. Inventario de síntomas](#4-inventario-de-síntomas)
- [5. Hallazgos](#5-hallazgos)
- [6. Estrategia de remediación por ejes](#6-estrategia-de-remediación-por-ejes)
- [7. Qué parches se eliminan al corregir la raíz](#7-qué-parches-se-eliminan-al-corregir-la-raíz)
- [8. Orden de ejecución obligatorio](#8-orden-de-ejecución-obligatorio)
- [9. Fuera de alcance (ortogonal)](#9-fuera-de-alcance-ortogonal)

## 1. Resumen ejecutivo

El sistema bajo prueba es un **singleton de máquina**: el daemon liga un puerto fijo, el residente `qwen_tts` otro, y ambos coordinan por un pidfile en el directorio de usuario. La suite serializa el acceso con `Mutex` de proceso y absorbe la no-determinación del arranque con reintentos. Ambos mecanismos son parches: el `Mutex` no puede excluir un recurso de máquina, y el reintento no puede sustituir una señal de readiness ausente. El flake que se observa es la consecuencia esperable de ambos.

La corrección no es "más locks" ni "más reintentos", sino **hacer coincidir el dominio del recurso con el dominio del test**: cada unidad de test debe poseer una instancia aislada del sistema, de modo que el paralelismo sea seguro por construcción. La biblioteca ya está preparada para ello (el servidor acepta la dirección por parámetro); el binario la desaprovecha.

## 2. Hipótesis retirada

Una caracterización previa atribuyó el flake a "contención de muchos binarios de test corriendo en paralelo". La lectura del código la desmiente:

- `cargo test` ejecuta los binarios de integración de forma secuencial; el paralelismo real es **intra-binario** (hilos de `libtest` dentro de un mismo binario). Lo confirma el propio comentario de `STATE_LOCK`: *"sin este lock, `cargo test` los corre en paralelo dentro del mismo binario"* (`cli_golden.rs`, `STATE_LOCK`).
- `crates/avi-daemon/tests/golden.rs` ejerce el contrato por `oneshot` en memoria, **sin socket ni puertos** (cabecera "Ceguera deliberada del harness"). No compite por 8765/8766.
- El único consumidor de los puertos reales es `cli_golden.rs`, y sus tests pesados ya están serializados (`STATE_LOCK` + `TTS_LOCK`).

La contención de memoria, además, ya fue mitigada por la *fixture por sesión del daemon* (`cli_golden.rs`, bloque "Fixture por sesión"), que colapsó N calentamientos a uno por corrida. Lo que persiste no es contención, sino los dos defectos siguientes.

## 3. Los dos defectos estructurales

### D-I — Dominio del recurso ≠ dominio del candado

Los candados que serializan el acceso (`STATE_LOCK`, `TTS_LOCK`) tienen alcance **de proceso**. Los recursos que protegen tienen alcance **de máquina**:

- Puerto del daemon: `src/main.rs` liga un **literal** `"127.0.0.1:8765"` (`run_daemon_server`, bind del `TcpListener`), sin env ni config que lo desvíe.
- Puerto del residente: `8766` (`avi-tts`, `DEFAULT_PORT`).
- Pidfile: `data_dir()/daemon.pid`, con `data_dir()` derivado de `LOCALAPPDATA`/directorio de usuario (`avi-store::data_dir`).

Cuando esos dominios no coinciden, **ningún `Mutex` puede garantizar exclusión**: un `cargo test` abortado (Ctrl-C, timeout de CI) deja un daemon/residente zombi en 8765/8766 y un pidfile obsoleto, y la corrida siguiente los hereda. El harness lo reconoce implícitamente: existe `verificar_cero_huerfanos` precisamente para atrapar ese residuo heredado.

> **Estado tras la remediación (addendum).** La parte **eliminable** de D-I quedó cerrada: el puerto del daemon pasó a efímero por instancia (Eje 1) y el estado a data-dir por instancia (Eje 2). El residente, que quedaba como último punto de D-I abierto, ya **no** usa `8766` como identidad ni faro de descubrimiento: su detección, limpieza y verificación se re-anclaron a su **identidad estable** —el `resident_pid` registrado en el pidfile por instancia y, como faro independiente del pidfile, un barrido por **imagen `qwen_tts`** (seguro porque el residente tiene imagen propia; el daemon comparte imagen con el CLI y por eso su kill-por-imagen sigue prohibido)—. El `8766` permanece **solo** como puerto de servicio real (`DEFAULT_PORT`/`default_port`/`resident.port`): el camino feliz daemon→residente le sigue hablando por ahí. Lo irreducible que queda es un **límite físico razonado**, no deuda: (a) el semáforo de capacidad de una única inferencia pesada residente y (b) una higiene por PID/imagen contra el huérfano que sobrevive a un aborto duro (Ctrl-C/`SIGKILL`), donde ningún `Drop` de Rust corre.

### D-II — Arranque sin señal de readiness

`run_daemon_server` publica dos hechos en momentos distintos y sin evento consumible: primero liga el socket e imprime "escuchando", y **después** lanza el warmup del motor TTS en `spawn_blocking` de segundo plano, acotado por `WARMUP_DEADLINE` (40 s). El estado atraviesa `running-pero-frío → running+warm` de forma asíncrona, y la inferencia solo es fiable en caliente.

El harness reconstruye "listo" por **sondeo con reintentos**: `esperar_estado_daemon` hace polling cada 200 ms hasta `REINTENTOS_WARM_FAILSAFE` (225 polls ≈ 45 s), y en el reinicio absorbe la ventana proceso-muerto → nuevo-sin-ligar donde un probe recibe `os error 10061` (connection refused). El defecto no es el 10061 en sí, sino que la espera se define como *"reintentar hasta que deje de fallar"* en lugar de *"esperar la señal de readiness; si no llega en el presupuesto, es un bug a diagnosticar"*. Un flake es, por construcción, una carrera perdida contra ese bucle.

## 4. Inventario de síntomas

Cada mecanismo defensivo del harness compensa uno de los dos defectos:

| Síntoma en el código (símbolo) | Compensa | Clasificación |
|---|---|---|
| `REINTENTOS_WARM_FAILSAFE` (225 polls a 200 ms) | D-II readiness | reintento disfrazado |
| `esperar_estado_daemon` (polls de 200 ms) | D-II readiness | best-effort |
| `reaper_ante_fallo`, `barrer_residente_por_imagen`, `puerto_abierto` | D-I recurso global | limpieza defensiva |
| `verificar_cero_huerfanos` | D-I residuo heredado | verificación de higiene |
| `into_inner()` sobre `Mutex` envenenado (`bloquear_estado`, `TTS_LOCK`) | secuela de D-I (panic con lock tomado) | recuperación silenciosa |
| Puertos fijos 8765/8766 | D-I falta de aislamiento por instancia | acoplamiento global (retirado: daemon a efímero; residente re-anclado a PID/imagen, 8766 solo como puerto de servicio) |

## 5. Hallazgos

### P-01 — La costura de dirección del daemon está muerta y bypasseada · 🔴 Crítica

Existen **tres** niveles de parametrización de la dirección del daemon y los tres se ignoran en el arranque real:

- `run_daemon_server(addr: SocketAddr, …)` está parametrizada por firma (`avi-daemon/src/lib.rs`) — el lado biblioteca es **correcto**.
- `avi-config` define `pub daemon_port: u16` con default `8765` (`avi-config/src/lib.rs`), pero **no se lee en ningún sitio salvo sus propios tests**: es configuración muerta.
- El binario, en vez de leer la config o la constante `DAEMON_ADDR`, liga un **literal** `"127.0.0.1:8765".parse()` (`src/main.rs`, rama de arranque).

**Impacto**: el aislamiento por instancia es imposible mientras el eslabón superior de la cadena de procesos esté clavado. No es un defecto de "falta parametrización", sino de "hay parametrización y se descarta".
**Corrección**: cablear `daemon_port` (o un env análogo) → `run_daemon_server`. Trabajo de conexión, no de diseño nuevo.

### P-02 — Asimetría padre/hijo en la desviabilidad · 🟠 Alta

El residente (proceso hijo) **sí** es desviable: `avi-tts::resolver_puerto` lee `QWEN3_TTS_PORT` y cae a `DEFAULT_PORT`. El daemon (proceso padre, el que fija el dominio del recurso de máquina) **no** lo es (ver P-01). Aislar solo el eslabón inferior no aísla nada: el padre sigue colisionando.

### P-03 — El readiness se infiere, no se señala · 🟠 Alta

Ver D-II. `esperar_estado_daemon` codifica la ausencia de señal como presupuesto de reintentos. Un timeout aquí hoy se trata como flake (se reintenta/absorbe) cuando debería tratarse como bug a diagnosticar. La marca `warm_failed` ya existe en el producto y se propaga; falta el evento positivo "ligado + warm" que el test pueda esperar con un `recv` acotado en vez de sondear.

### P-04 — Tolerancia al envenenamiento del `Mutex` como recuperación silenciosa · 🟡 Media

`bloquear_estado` y el guard de `TTS_LOCK` recuperan el lock envenenado con `unwrap_or_else(|e| e.into_inner())`. Es defendible **hoy** (sin aislamiento, un panic en un test contaminaría a los demás de la clase pesada), pero convierte "un test reventó" en una recuperación invisible. Con aislamiento por instancia deja de ser necesario y debe retirarse para que el envenenamiento vuelva a ser la señal que es.

### P-05 — Serialización que mezcla capacidad física con colisión de recurso · 🟡 Media

`STATE_LOCK`/`TTS_LOCK` serializan por dos razones distintas fundidas en una: (a) evitar colisión de puerto/pidfile (síntoma de D-I) y (b) respetar que la máquina no admite dos motores TTS residentes de ~1.8 GB de RAM simultáneos sobre el puerto de servicio fijo (restricción física legítima). Al mezclarlas, la serialización parece intrínseca cuando en realidad la mitad es eliminable.

### P-06 — Clases de test sin segregar formalmente · ⚪ Baja

`golden.rs` (contrato puro por `oneshot`, sin proceso) es paralelizable sin restricción; `cli_golden.rs` (E2E con proceso real) no lo es hoy. La distinción existe de facto pero no está declarada como contrato: no hay una frontera explícita que impida que un test nuevo de contrato herede accidentalmente el andamiaje pesado, ni que uno pesado se cuele sin serializar.

### P-07 — Precedente de la corrección correcta ya aplicado · (informativo)

`open_atomic_tmp` documenta y resuelve un flake intra-binario **real**: dos hilos de `libtest` generaban el mismo tempfile (resolución gruesa de `SystemTime` en Windows) y el `remove_file` de uno borraba el del otro → `NotFound (os 2)` solo en `win-server-2022`. Se corrigió **estructuralmente** (creación atómica `O_CREAT|O_EXCL`, exclusión garantizada por el SO), no con retry sintomático. Es la misma medicina que proponen los ejes de abajo, ya validada en este harness.

## 6. Estrategia de remediación por ejes

**Principio rector**: hacer coincidir el dominio del recurso con el dominio del test. Cada unidad de test posee una instancia aislada; el paralelismo es seguro por construcción, sin locks de conveniencia, sin reapers, sin reintentos.

- **Eje 1 — Puerto efímero por instancia (`:0`)**. Cablear `daemon_port`/env → `run_daemon_server` y ligar `:0`, dejando que el SO asigne; el binario imprime el puerto real. Ya hay precedente (`QWEN3_TTS_PORT` en el residente). Elimina la colisión de puertos y todo el barrido por puerto.
- **Eje 2 — Directorio de estado por instancia**. Inyectar `LOCALAPPDATA`/`HOME`/data-dir a un tempdir único por test. El fontanero ya existe: `run_json_env(args, envs)` acepta envs y `TMP_COUNTER` provee unicidad. El pidfile deja de ser compartido → cero huérfanos heredados por construcción.
- **Eje 3 — Readiness por señal, no por sondeo**. El arranque emite un evento explícito "ligado + warm" (puerto y estado en stdout/fichero) que el test espera con un `recv` acotado. La espera pasa de "reintentar hasta que no falle" a "esperar el evento; timeout = bug".
- **Eje 4 — Serializar solo por capacidad física real**. Modelar la restricción de una única inferencia pesada residente con un semáforo de capacidad para la clase pesada, documentado como límite real de recursos. Serializar por capacidad de inferencia (RAM + puerto de servicio fijo) es correcto; serializar por puerto/pidfile es un síntoma que desaparece con los ejes 1-2.
- **Eje 5 — Retirar la tolerancia al envenenamiento**. Con instancias aisladas y serialización solo por capacidad física, un panic debe propagarse como fallo. Se retira `into_inner()`; un `Mutex` envenenado vuelve a ser la señal de que un test reventó.
- **Eje 6 — Segregar clases de test**. Declarar el contrato: contrato puro (paralelizable) vs E2E-con-proceso (aislado por instancia + serializado solo por capacidad de inferencia). Son garantías distintas y deben nombrarse como tales.

## 7. Qué parches se eliminan al corregir la raíz

| Parche actual | Defecto que compensa | Eliminado por | Por qué desaparece |
|---|---|---|---|
| `puerto_abierto` sobre el puerto del daemon, sondeo del puerto fijo del daemon | D-I | Eje 1 | Con puerto asignado por el SO no hay colisión → nada que barrer por puerto. |
| Sondeo/verificación del residente por `8766` (identidad, faro y cierre) | D-I | Re-anclaje a PID/imagen | El residente se gobierna por su identidad estable —`resident_pid` registrado + barrido por imagen `qwen_tts`—; `8766` queda solo como puerto de servicio real. `barrer_residente_por_imagen` sustituye al barrido por puerto como faro pidfile-independiente. |
| `reaper_ante_fallo`, `GuardReaper`, `verificar_cero_huerfanos`, reclamo por PID como muleta | D-I | Eje 2 | Pidfile por instancia → ningún test hereda el zombi de otro → cero huérfanos por construcción. *(El reclamo por PID sigue siendo lógica de producto legítima; deja de ser andamiaje del harness.)* |
| `REINTENTOS_WARM_FAILSAFE`, polls de `esperar_estado_daemon` | D-II | Eje 3 | La espera se ancla a un evento; el reintento pierde su razón de ser. |
| `into_inner()` sobre `Mutex` envenenado | secuela de D-I | Ejes 2 + 5 | Sin contaminación cruzada, el envenenamiento vuelve a ser fallo legítimo. |
| Serialización total bajo `STATE_LOCK`/`TTS_LOCK` | D-I + capacidad física | Ejes 1-2 + 4 | Se reduce al semáforo por capacidad de inferencia; la parte que compensaba colisión desaparece. |

**Resultado neto**: de los seis mecanismos defensivos, **cinco desaparecen**. El único que sobrevive es la serialización, reducida a su núcleo físico real (una única inferencia pesada residente), documentada como límite real de recursos y no como muleta.

## 8. Orden de ejecución obligatorio

Por dependencia, no negociable:

1. **Aislar** (Eje 1 puerto efímero + Eje 2 data-dir por instancia).
2. **Señalizar readiness** (Eje 3).
3. **Segregar clases** y **modelar capacidad física** (Ejes 6 + 4).
4. **Retirar la tolerancia al envenenamiento** (Eje 5).

Retirar `into_inner()` (Eje 5) antes de aislar (Ejes 1-2) haría que un solo test roto cascadeara al resto de la clase pesada — justo lo que la tolerancia evita hoy. El Eje 5 es siempre el último.

## 9. Fuera de alcance (ortogonal): cobertura de plataforma en CI

Deslindado del paralelismo pero relevante para no confundir clases de defecto: existe una brecha de cobertura de plataforma **abierta**.

- `.circleci/config.yml` define una puerta triple simétrica —`test-linux` (`cargo test --all`), `test-windows`, `test-macos`— más `coverage`. La puerta **existe**: no falta un job de Linux.
- Pero **todos** los jobs declaran `filters.tags: only /^v.*/` con `branches: ignore /.*/`: el workflow `build-all` corre **solo en tags `v*`, nunca en push de rama**. No hay ninguna puerta de rama; la validación previa al tag (fmt/clippy/test) se delega a un procedimiento manual documentado en `docs/BUILD.md`.
- **Consecuencia estructural (estado actual)**: un desarrollador en host Windows no ve rupturas específicas de no-Windows hasta etiquetar. La brecha está activa: se materializó en dos rupturas no-Windows que solo afloraron tras el tag `v0.19.0` — un reexport sin `#[cfg(windows)]` (E0432 en el build no-Windows) y un `kill` al grupo `-<pid>` mal dirigido que colgaba solo en Linux/Docker (el residente se spawnea sin grupo/sesión propia, hereda el del daemon).
- **Remedio preciso**: no "añadir un job de Linux" (ya existe), sino **disparar una puerta no-Windows en push a `main`** (branch gate con al menos `cargo check`/`test` en Linux), en vez de depender de la validación manual pre-tag. Deliverable distinto con su propia decisión, ajeno a los ejes 1-6.
