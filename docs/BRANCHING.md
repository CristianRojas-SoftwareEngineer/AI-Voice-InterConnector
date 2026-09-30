# Estrategia de ramificación y higiene del entorno

Este repositorio usa una ramificación deliberadamente simple: `main` contiene
siempre el estado más actualizado de la aplicación, y todo cambio entra por
ramas transitorias de vida corta. No hay ramas de desarrollo, release ni
soporte: hasta el lanzamiento hay un único mantenedor y una estructura mayor
complicaría la gestión de versiones y el CI/CD sin aportar nada.

## Ramas

- `main`: estado vigente. Nunca se commitea directamente salvo retoques
  triviales; todo lo demás llega por merge de rama transitoria.
- Transitorias: `fix/<sintoma>`, `feat/<capacidad>`, `hotfix/<asunto>` o
  `docs/<tema>`. Una rama = un cambio revisable; al integrarse se elimina.

## Actualizar e integrar (siempre merge, nunca rebase publicado)

Para poner la rama al día con `main`:

```bash
git checkout fix/mi-cambio
git merge main
```

Para integrar la rama en `main` (con commit de merge explícito, para que el
historial conserve qué commits agrupó cada flujo):

```bash
git checkout main
git merge --no-ff fix/mi-cambio
git branch -d fix/mi-cambio
```

No se rebasea lo ya publicado: el rebase reescribe hashes y rompe la
trazabilidad que el merge conserva.

## Tags y CI/CD

Las releases se cortan con tags `v*` sobre `main` (`cargo xtask release`
promociona la sección `## [No publicado]` del CHANGELOG). El pipeline de
build solo corre en tags; los jobs de test corren en `main` y ramas.

## Worktrees

Cada worktree lleva su propia rama transitoria y resuelve su propia raíz con
`git rev-parse --show-toplevel`, así que el hook y `xtask` operan por
worktree. Para no multiplicar el coste en disco, cada worktree compila con su
propio `target-dir` o comparte el principal por turnos (nunca dos builds
concurrentes contra el mismo `target/`), y cada uno usa su propio `data-dir`
y puertos del daemon.

## Hooks (`core.hooksPath`, opt-in por clon)

Los hooks viven versionados en `.githooks/` y no se activan solos. Activarlos
una vez por clon (y por cada worktree que los necesite):

```bash
git config core.hooksPath .githooks
```

- `pre-commit`: puertas de idioma, comentarios, pines y features. Falla el
  commit si no pasan.
- `post-merge`: poda fina tras integrar en `main`. Solo actúa si la rama
  actual es `main`, el merge tocó dependencias o toolchain (`Cargo.lock`,
  manifiestos, `vendor/`, toolchain o pines) y `target/` supera
  `PRUNE_THRESHOLD_GB` (10 GB, constante en la cabecera del hook).
  Entonces ejecuta `cargo xtask clean --prune --yes`. Es best-effort y nunca
  falla el merge (sale siempre con 0).

## Higiene de `target/`

`target/` crece sin límite (hashes obsoletos, PDBs, `incremental/`,
perfiles y cruces). Dos herramientas lo contienen:

```bash
# Ver qué podaría la poda fina, sin borrar nada
cargo xtask clean --prune --dry-run

# Podar (conserva la compilación vigente; el siguiente build es incremental)
cargo xtask clean --prune --yes
```

El defecto de `clean` no cambia: sin `--prune` sigue borrando la capa
entera. `cargo xtask doctor` incluye la fila opcional `target-hygiene`
(tamaño frente al umbral de 10 GB y estado del hook) con el comando exacto
cuando hay exceso. Los PDBs se consideran viejos a partir de
`PRUNE_PDB_MAX_AGE_DAYS` (7 días, constante en `crates/xtask/src/clean.rs`).
