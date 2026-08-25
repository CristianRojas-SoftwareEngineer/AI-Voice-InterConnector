# Descripción

<!-- Qué problema resuelve este PR y cómo. Enlaza el Issue relacionado si existe. -->

## Cómo verificarlo

<!-- Comandos o pasos para comprobar el cambio. -->

## Checklist

- [ ] `cargo test --all`, `cargo fmt --all --check` y `cargo clippy --all-targets` pasan en local.
- [ ] Añadí tests para todo comportamiento nuevo o corregido.
- [ ] La documentación afectada (`USAGE.md`, `docs/`, `CLAUDE.md`) quedó sincronizada con el cambio.
- [ ] Si cambié `Cargo.toml`, regeneré `Cargo.lock` y revisé el diff (y actualicé `THIRD-PARTY-LICENSES.md` si aplicó).
- [ ] Si cambiaron las dependencias empaquetadas, actualicé `THIRD-PARTY-LICENSES.md`.
- [ ] Mensajes de commit en español, con prefijo de tipo (`feat:`, `fix:`, `docs:`, …).
