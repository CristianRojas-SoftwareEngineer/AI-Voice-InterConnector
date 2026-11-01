# Descripción

<!-- Qué problema resuelve este PR y cómo. Enlaza el Issue relacionado si existe. -->

## Cómo verificarlo

<!-- Comandos o pasos para comprobar el cambio. -->

## Checklist

- [ ] `cargo test --all`, `cargo fmt --all --check` y `cargo clippy --all-targets` pasan en local.
- [ ] Añadí tests para todo comportamiento nuevo o corregido.
- [ ] La documentación afectada (`README.md`, `USAGE.md`, `docs/`, `AGENTS.md`) quedó sincronizada con el cambio.
- [ ] Si cambié `Cargo.toml`, regeneré `Cargo.lock` y revisé el diff.
- [ ] Si cambiaron las dependencias distribuidas o `packaging/notices/`, regeneré `THIRD-PARTY-LICENSES.md` con `cargo xtask licenses`.
- [ ] Mensajes de commit en español, con prefijo de tipo (`feat:`, `fix:`, `docs:`, …).
