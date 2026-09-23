# Generación mecánica del inventario de licencias de terceros

Estado: **propuesta pendiente** — describe un problema vigente y la estrategia
para resolverlo; no hay implementación.

## Tabla de contenidos

- [Contexto](#contexto)
- [Problema](#problema)
  - [El inventario se mantiene a mano](#el-inventario-se-mantiene-a-mano)
  - [La verificación compara solo nombres](#la-verificación-compara-solo-nombres)
  - [Desfases observados](#desfases-observados)
  - [El desfase se descubre tarde](#el-desfase-se-descubre-tarde)
- [Estrategia propuesta](#estrategia-propuesta)
  - [Modo generación](#modo-generación)
  - [Modo verificación](#modo-verificación)
  - [Decisiones a tomar al implementar](#decisiones-a-tomar-al-implementar)
- [Impacto esperado](#impacto-esperado)
- [Fuera de alcance](#fuera-de-alcance)

## Contexto

El proyecto se distribuye bajo GPL-3.0-or-later y su binario empaqueta crates de
terceros cuyas licencias exigen conservar avisos de atribución. Esos avisos se
reúnen en `THIRD-PARTY-LICENSES.md`, cuya sección «Inventario completo del
lockfile» es una tabla con una fila por crate:

| Columna | Contenido |
|---|---|
| Paquete | nombre del crate |
| Versión | versión resuelta en `Cargo.lock` |
| Licencia (metadato) | expresión SPDX declarada por el crate |
| Familia | normalización de la licencia para agruparla |

Encima de la tabla hay dos datos agregados: el número de crates únicos y un
resumen de filas por familia.

El subcomando `cargo run -p xtask -- licenses --check` verifica esa tabla contra
`Cargo.lock` y el job `validate-licenses` de la pipeline de release lo ejecuta
como puerta de los builds.

## Problema

La comprobación ataca el síntoma (una tabla que se desalineó del lockfile) y no
la causa (que la tabla dependa de un proceso manual).

### El inventario se mantiene a mano

No existe en el repositorio un generador de la tabla. La sección «Regeneración»
del propio documento indica reconstruirla con `cargo metadata` o `cargo-license`,
herramientas externas cuya salida hay que transformar a mano al formato de la
tabla. Cada alta, baja o actualización de dependencias exige repetir ese trabajo,
y nada garantiza que el resultado sea reproducible.

### La verificación compara solo nombres

`licenses --check` extrae el conjunto de nombres de `Cargo.lock` y el de la
columna «Paquete», y falla solo si difieren: crates del lock sin fila
(atribución faltante) o filas sin crate (obsoletas). Queda fuera de la
comparación todo lo demás:

- **Versión.** Una actualización de un crate no altera el conjunto de nombres,
  así que la columna «Versión» puede quedar desactualizada sin que la puerta lo
  detecte.
- **Licencia.** La columna «Licencia (metadato)» no se contrasta con el
  metadato real del crate. Es el caso más relevante para el cumplimiento,
  porque una licencia mal registrada puede ocultar obligaciones de atribución
  distintas o una incompatibilidad con GPLv3.
- **Versiones múltiples.** Cuando el lock resuelve varias versiones de un mismo
  crate, la tabla tiene una sola fila y el conjunto de nombres las colapsa, así
  que la comprobación no distingue esa situación.
- **Datos agregados.** El conteo de crates únicos y el resumen por familia son
  texto fijo que nadie recalcula.

### Desfases observados

Una auditoría de la tabla contra `Cargo.lock` y contra
`cargo metadata --all-features`, hecha mientras `licenses --check` pasaba,
encontró desfases en cada una de las dimensiones anteriores:

| Dimensión | Estado declarado | Estado real |
|---|---|---|
| Versión | La fila `ai-voice-interconnector` declara 0.18.26 | El lock resuelve la versión vigente del proyecto; es la única fila con una versión inexistente en el lock |
| Versiones múltiples | 447 filas, una por nombre | 496 paquetes: 37 crates con dos o más versiones suman 86, así que 49 versiones resueltas no tienen fila |
| Licencia de terceros | 16 filas registran `MIT OR Apache-2.0` | El crate declara otra cosa: solo `MIT` (p. ej., `castaway`, `compact_str`, `ct2rs`, `onig`), solo `Apache-2.0` (p. ej., `tokenizers`, `prost`, `sentencepiece-sys`), `Zlib` (`foldhash`) o `Unlicense OR MIT` (`termcolor`) |
| Licencia incompleta | 4 filas registran `MIT OR Apache-2.0` | La expresión real ofrece más alternativas: `Apache-2.0 OR MIT OR Zlib` (`macro_rules_attribute` y su proc-macro) y `BSD-2-Clause OR Apache-2.0 OR MIT` (`zerocopy` y su derive) |
| Crates del workspace | Los crates `avi-*` y `ai-voice-interconnector` figuran como `GPL-3.0-or-later`, `xtask` como `MIT OR Apache-2.0` | Ningún `Cargo.toml` del workspace declara el campo `license`: esas filas no tienen metadato que las respalde |
| Conteo | «455 crates únicos» | 447 nombres únicos |
| Resumen por familia | Suma 451 (MIT 393, GPL-3.0-or-later 9) | Las filas suman 447 (MIT 390, GPL-3.0-or-later 8) |

Las filas con licencia mal registrada se concentran en las dependencias de la
pila de STT y traducción. El patrón sugiere que se completaron con un valor
supuesto en lugar de leer el metadato. Ninguna de las licencias reales es
incompatible con GPLv3, pero la atribución publicada es incorrecta.

### El desfase se descubre tarde

La pipeline de CI corre solo al empujar un tag `v*`. Un cambio de dependencias
puede quedar en `main` durante varias iteraciones sin que nada señale que el
inventario quedó atrás, y la falla aparece recién al cortar la release. En ese
momento la corrección obliga a regenerar la tabla a mano bajo la presión del
corte.

## Estrategia propuesta

Tratar el inventario como un artefacto **generado** por `xtask`, con el mismo
patrón que ya usa `SOURCE-OFFER.md`: una función que renderiza el contenido
esperado a partir de la fuente de verdad, un modo que lo escribe y un modo
`--check` que compara el archivo con el renderizado completo.

### Modo generación

`cargo run -p xtask -- licenses` (sin `--check`) debe:

1. Ejecutar `cargo metadata --format-version 1 --locked --all-features` y
   leer, para cada paquete, nombre, versión y expresión de licencia
   (`license`, o `license_file` cuando el crate no declara expresión SPDX).
   `--all-features` es imprescindible: sin él, `cargo metadata` omite los
   crates que solo entran por features opcionales (en la auditoría, 431
   paquetes frente a 495).
2. Calcular la columna «Familia» con una tabla de normalización explícita en
   el código (por ejemplo, `MIT OR Apache-2.0` → `MIT`), que falle ruidosamente
   ante una expresión desconocida en lugar de asignarle una familia por defecto.
   Así toda licencia nueva exige una decisión consciente.
3. Renderizar la tabla ordenada por nombre y versión, con una fila por versión
   resuelta, y calcular el conteo y el resumen por familia desde los mismos
   datos.
4. Reemplazar en `THIRD-PARTY-LICENSES.md` solo la región del inventario,
   delimitada por marcadores estables (por ejemplo, comentarios HTML de inicio
   y fin), sin tocar la prosa escrita a mano del resto del documento.

### Modo verificación

`licenses --check` debe renderizar la región con el mismo código y compararla
con la región del archivo, normalizando fines de línea y mostrando un diff. Así
cualquier divergencia de nombre, versión, licencia, familia o datos agregados
hace fallar la puerta, y la corrección se reduce a ejecutar el modo generación
y revisar el diff.

Con el generador en su lugar, la comprobación puede ejecutarse también desde
`xtask release` antes de mutar archivos, de modo que el desfase se detecte en
local antes de crear el tag y no solo en la pipeline.

### Decisiones a tomar al implementar

- **Dependencia para leer JSON.** `cargo metadata` emite JSON; leerlo de forma
  robusta requiere añadir `serde_json` (y probablemente `serde`) al crate
  `xtask`. `xtask` no se distribuye, así que el costo se limita al tiempo de
  compilación de los jobs que lo ejecutan.
- **Alcance del inventario.** Hoy la tabla cubre todos los paquetes de
  `Cargo.lock`, incluidos los de desarrollo y el propio `xtask`, que no llegan
  al binario. Conviene decidir si mantener esa cobertura amplia (más simple y
  conservadora) o limitarla al grafo de dependencias del binario distribuido
  (más precisa, pero dependiente de la plataforma de destino). La primera
  implementación puede conservar la cobertura actual para que la migración no
  cambie el contenido más allá de corregir el desfase.
- **Fuente del conjunto de paquetes.** Incluso con `--all-features`,
  `cargo metadata` no reporta todos los paquetes de `Cargo.lock`: la auditoría
  encontró uno (`cxxbridge-cmd`) presente en el lock y ausente del metadato.
  Si la puerta sigue exigiendo que todo paquete del lock tenga fila, el
  generador debe tomar el conjunto de paquetes de `Cargo.lock` y usar
  `cargo metadata` solo para las licencias, o leer la licencia de esos casos
  desde el manifiesto del registro local.
- **Licencia de los crates del workspace.** Los `Cargo.toml` del workspace no
  declaran `license`, así que el generador no tiene de dónde leerla. Lo más
  limpio es declarar `license = "GPL-3.0-or-later"` en cada manifiesto
  distribuido y decidir explícitamente la licencia de `xtask`, en lugar de
  codificar excepciones en el generador.
- **Reproducibilidad.** `cargo metadata` puede requerir red si el registro local
  no tiene los paquetes. Usar `--locked` y documentar que el modo generación se
  ejecuta con las dependencias ya descargadas; el modo verificación en CI corre
  después de restaurar la caché del registro.

## Impacto esperado

- La tabla pasa a ser reproducible con un solo comando, sin herramientas
  externas.
- La puerta detecta cambios de versión y de licencia, no solo altas y bajas.
- Los datos agregados dejan de ser texto fijo y no pueden desalinearse.
- El primer uso del generador corregirá de una vez los desfases descritos en
  «Desfases observados». Ese diff debe revisarse con atención: agregará cerca
  de medio centenar de filas por versiones múltiples y cambiará licencias ya
  publicadas.

## Fuera de alcance

El inventario cubre solo crates de Rust resueltos en `Cargo.lock`. Los
componentes nativos que se compilan o empaquetan fuera de Cargo (el motor TTS
vendorizado y bibliotecas C/C++ enlazadas) no aparecen en el lockfile, y esta
estrategia no los cubre. Confirmar que su atribución está completa es una
revisión aparte.
