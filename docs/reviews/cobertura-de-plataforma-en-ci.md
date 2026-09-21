# Cobertura de plataforma en CI: ausencia de puerta de rama

- **Tipo**: brecha de cobertura conocida (abierta)
- **Ámbito**: la configuración de integración continua (`.circleci/config.yml`) y su relación con la validación previa a publicar.
- **Severidad**: alta — permite que rupturas específicas de una plataforma lleguen sin detección hasta el momento de etiquetar una versión.

## El problema

El proyecto compila y se prueba en tres plataformas (Linux, Windows, macOS), pero **la validación automática solo se dispara al crear un tag de versión**, no en los pushes a la rama de trabajo. Un desarrollador que trabaja en un host de una sola plataforma no recibe señal automática de una ruptura específica de otra plataforma hasta que etiqueta una release, es decir, después de que el código ya se considera listo.

## Estructura actual de CI

`.circleci/config.yml` define un **único workflow de release** (`build-all`) compuesto por:

- Puertas de código Rust por plataforma: `test-linux`, `test-windows`, `test-macos` (cada una corre la suite completa en su SO), más `coverage`.
- Puertas de empaquetado: `validate-licenses`, `validate-changelog`, `test-installer-*`.
- Los builds nativos y la publicación, que dependen de que las puertas anteriores estén verdes.

La puerta triple por plataforma **existe**: no falta un job de Linux ni de macOS. El problema no es la ausencia de un job, sino **cuándo se ejecutan**:

- Todos los jobs declaran `filters.tags: only /^v.*/` junto con `filters.branches: ignore /.*/`.
- CircleCI no ejecuta un job en un tag salvo que ese job **y todas sus dependencias** declaren `filters.tags`; por eso el filtro `v*` se propaga por toda la cadena.
- El efecto combinado: el workflow corre **solo en tags `v*`** y **nunca en un push de rama**. No hay ninguna puerta de rama.

La validación previa al tag (formato, lint y tests) queda delegada a un **procedimiento manual** documentado en `docs/BUILD.md`, que depende de que quien publica lo ejecute a conciencia y en las plataformas que su host no cubre.

## Consecuencia estructural

La detección de rupturas de plataforma se aplaza al peor momento posible: el tag. Las clases de fallo que este hueco deja pasar son precisamente las que un solo host no ejercita, por ejemplo:

- **Errores de compilación condicionados por plataforma**: código detrás de `#[cfg(...)]` (reexports, dependencias, llamadas al SO) que solo se compila fuera del host del desarrollador. Un símbolo mal condicionado rompe el build de la plataforma no cubierta y el host local nunca lo ve.
- **Divergencias de comportamiento del SO en tiempo de ejecución**: señalización de procesos, grupos/sesiones, rutas y APIs de audio difieren entre Unix y Windows. Una suposición válida en un SO puede colgar o fallar en otro, y sin ejecutar la suite en ese SO el fallo permanece latente.

Al no existir una señal automática en rama, estas rupturas conviven en `main` hasta que el proceso de release las expone, cuando corregirlas obliga a rehacer o mover el tag.

## Remedio recomendado

Añadir una **puerta de rama en push a `main`** (branch gate), independiente del workflow de release por tags:

- Como mínimo, `cargo check`/`cargo test` en **Linux** en cada push, para atrapar el error de compilación condicionado por plataforma sin esperar al tag.
- Idealmente, extender la puerta de rama a las tres plataformas si el presupuesto de CI lo permite, de modo que las divergencias de tiempo de ejecución también se ejerciten temprano.

No se trata de añadir un job de Linux —ya existe—, sino de **disparar la validación en el ciclo de rama** en lugar de depender de la validación manual previa al tag. Es una decisión de configuración de CI con su propio coste (minutos de build por push) y su propio alcance, ortogonal a la lógica del producto y a la estrategia de aislamiento de la suite de tests.
