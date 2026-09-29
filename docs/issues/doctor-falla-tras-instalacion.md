# `doctor` falla justo después de una instalación correcta

| Campo | Valor |
|---|---|
| Estado | abierto |
| Severidad | alta |
| Tipo | funcional |
| Componente | `avi-lifecycle`: `install` (paso de provisión) y `doctor` |
| Versión detectada | 0.25.0 |
| Plataforma | Windows 11. Por el análisis de causa, afecta a todas las plataformas |
| Reproducibilidad | siempre, en una instalación desde cero |
| Detectado en | prueba E2E de v0.25.0, 2026-09-28 |

## Resumen

Tras una instalación desde cero que termina con exit 0, `doctor` sale con 1 por dos
motivos independientes:

1. Los derivados CT2 de traducción (`es-en` y `en-es`) no quedan convertidos, aunque la
   instalación descargó sus modelos Marian.
2. El modelo Base de clonado, que es **opt-in**, cuenta como modelo faltante.

El primero es un defecto real de la instalación: la traducción no funciona hasta
ejecutar `setup` a mano. El segundo hace que `doctor` nunca pase para quien no activó
el clonado de voz.

## Entorno

- Binario: v0.25.0, instalado con el bootstrap `install.ps1` (con el BOM retirado, ver
  [install-ps1-bom-rompe-irm-iex.md](install-ps1-bom-rompe-irm-iex.md)).
- SO: Windows 11, PowerShell 5.1.
- Caché de modelos vacía antes de instalar. Sin daemon.

## Precondiciones

- Caché de modelos vacía: en particular, sin snapshots `marian-es-en` / `marian-en-es`
  y sin derivados `ct2/`.
- Instalación sin `--with-voice-cloning`.

## Pasos para reproducir

1. Instalar desde cero con provisión de modelos (el bootstrap delega en
   `self install`, que provisiona en su paso 11).
2. Ejecutar `ai-voice-interconnector doctor`.
3. Ejecutar `ai-voice-interconnector setup --json` y repetir `doctor`.
4. Ejecutar `ai-voice-interconnector setup --with-voice-cloning` y repetir `doctor`.

## Resultado observado

- Paso 2: exit 1. Fallan `models_provisioned` (falta el modelo Base de clonado) y
  `models_ct2` («derivado CT2 incompleto: es-en, en-es»). También falla `path_resolves`,
  pero solo porque la sesión no había recargado el PATH; eso es esperado y no forma
  parte de este defecto.
- Paso 3: `setup` informa `completed` y **convierte** los dos derivados CT2. `doctor`
  sigue en exit 1, ahora solo por `models_provisioned` (Base).
- Paso 4: `doctor` pasa.

## Resultado esperado

- Una instalación que termina con éxito deja la traducción operativa. El contrato
  describe `setup` como la operación que descarga Marian y convierte **siempre** su
  derivado CT2 obligatorio, y `self install` delega en esa misma provisión.
- Un modelo opt-in no activado no es un fallo. El propio reporte ya distingue su estado
  con el campo `models.base` = `missing_opt_in`; el veredicto no debería tratarlo como
  un modelo obligatorio ausente.

## Impacto y workaround

Tras instalar, la traducción (`translate` y `speech dub` con cambio de idioma) sale con
`model_missing` hasta que el usuario ejecuta `setup`. `doctor`, la herramienta que
debería orientarle, falla también en una instalación sana sin clonado, lo que resta
valor a su veredicto y rompe cualquier script que lo use como comprobación.

Workaround: ejecutar `ai-voice-interconnector setup` después de instalar. Para que
`doctor` pase, ejecutar además `setup --with-voice-cloning`, aunque no se quiera usar
el clonado.

## Evidencia

- El `setup` posterior a la instalación convirtió los dos derivados, así que antes no
  existían o no pasaban el gate.
- Un segundo `setup` informa que omite CT2 (idempotente), lo que confirma que la
  conversión es correcta cuando se ejecuta.

## Análisis de causa

**Parte 1, CT2 no convertido. Confirmada por lectura del código.**

- `crates/avi-lifecycle/src/install.rs`, `pending_models`, calcula
  `setup::pending(&store, …)` **antes de descargar**, y `provision` recorre después
  `pending.models` y `pending.ct2` sin recalcular nada.
- `crates/avi-lifecycle/src/setup.rs`, `pending`, solo incluye un par en `ct2` si el
  repo `marian-{pair}` **ya está provisionado**. Así lo dice su comentario: «sin
  snapshot no hay nada que convertir».
- En una caché vacía, Marian todavía no está en el momento del cálculo, así que
  `pending.ct2` queda vacío. `provision` descarga Marian y termina sin convertir nada.
- `setup::run`, en cambio, descarga y después recorre `CT2_PAIRS` comprobando el estado
  real del almacén. Por eso `setup` sí convierte.
- Este defecto solo aparece en una instalación desde cero. Con Marian ya en la caché, el
  cálculo previo sí incluye los pares.

**Parte 2, Base opt-in contado como faltante. Confirmada por lectura del código.**

- `crates/avi-lifecycle/src/doctor.rs`, función `models`, llena `missing` recorriendo
  todo `avi_store::MODEL_REVISIONS`, que incluye el modelo de clonado
  (`setup::CLONING_MODEL`).
- La comprobación `models_provisioned` falla si `missing` no está vacío. El estado
  opt-in se calcula aparte, en `base`, pero no se descuenta de `missing`.

## Diagnóstico sugerido

- Parte 1: en `provision`, decidir la conversión CT2 **después** de las descargas.
  Puede hacerse recalculando `setup::pending` tras descargar o reutilizando el tramo de
  derivados de `setup::run`, para que install y setup compartan una sola regla. La
  regla actual de `pending` sigue sirviendo para el resumen previo, que informa del
  tamaño, siempre que el resumen cuente como pendiente el CT2 de un Marian que se va a
  descargar.
- Parte 2: excluir `CLONING_MODEL` de `missing` cuando no está en la selección guardada
  del usuario, o que `models_provisioned` evalúe solo la selección. Hay que revisar las
  pruebas de `doctor` que afirman `base = "missing_opt_in"` para que sigan cubriendo el
  campo.

## Criterio de aceptación

- Tras `self install` con provisión sobre una caché vacía, los dos derivados CT2 pasan
  el gate (`is_ct2_provisioned`) sin ejecutar `setup`.
- Justo después de esa instalación, sin clonado, `doctor` sale con 0 (una vez recargado
  el PATH) y reporta `models.base = "missing_opt_in"`.
- Existe una prueba de `avi-lifecycle` que parte de un almacén sin Marian y comprueba
  que la provisión de install convierte CT2 tras descargar (con el descargador simulado).
- Existe una prueba de `doctor` con un almacén completo salvo el modelo Base, que
  espera `models_provisioned` en verde.

## Relacionados

- [install-ps1-bom-rompe-irm-iex.md](install-ps1-bom-rompe-irm-iex.md): el canal por el
  que se reprodujo la instalación.
- Contrato de la CLI, sección de `setup`: conversión obligatoria del derivado CT2 y
  exigencia de `doctor` sobre ambas direcciones.
