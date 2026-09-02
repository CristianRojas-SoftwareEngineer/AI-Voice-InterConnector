# Revisión: hallazgos colaterales de la prueba E2E del actualizador (v0.18.24 → v0.18.25)

- **Fecha**: 2026-09-01
- **Estado**: Cerrado. Los cuatro hallazgos están diagnosticados con causa raíz; ninguno requiere acción sobre el código. Dos candidatos a mejora opcional quedan anotados en la sección de recomendaciones, pendientes de aprobación.
- **Alcance**: Resultados colaterales (no objetivos) de la prueba E2E de la auto-actualización B+C+D ejecutada el 2026-09-01: limpieza total → instalación pineada de v0.18.24 → upgrade real a v0.18.25 vía `upgrade-ai-voice-interconnector.ps1`. La prueba en sí fue exitosa (upgrade en 29 s, binario reemplazado, PATH intacto, 4 modelos conservados sin re-descarga).
- **Naturaleza**: Diagnóstico post-prueba. Cada hallazgo distingue defecto del repositorio vs. defecto del entorno local vs. cosmético.
- **Autor**: Orquestación con 3 subagentes secuencial (limpieza / instalación / upgrade) sobre Windows 11.

## Tabla de contenidos

- 1. Resumen
- 2. Hallazgo 1 — `uninstall` nativo roto en v0.18.17/0.18.18 (binario stale de CI, ya corregido)
- 3. Hallazgo 2 — `Get-FileHash` ausente en PowerShell 5.1 (contaminación de `PSModulePath` del entorno local)
- 4. Hallazgo 3 — Estimación de ~2 GB vs. 8.9 GB reales de modelos (error de estimación ajeno al repo)
- 5. Hallazgo 4 — Mojibake en los logs del instalador (codepage 850 vs. UTF-8 sin BOM)
- 6. Recomendaciones (opcionales, pendientes de aprobación)

## 1. Resumen

Durante la prueba E2E del actualizador surgieron cuatro hallazgos colaterales. Ninguno bloqueó la prueba ni afecta al veredicto de la feature (el actualizador funciona: `0.18.24 -> 0.18.25` en 29 s conservando los ~8.9 GB de modelos). Uno es un defecto real del repositorio pero **ya corregido en v0.18.19** (`5840109`) con las releases v0.18.17/0.18.18 aún publicadas y defectuosas en GitHub Releases; los otros tres son del entorno local de la máquina de pruebas o cosméticos.

| # | Hallazgo | Causa raíz | ¿Defecto del repo? | Gravedad |
|---|---|---|---|---|
| 1 | `uninstall` v0.18.18 exit 1 silencioso | Binario stale pre-`67fc1bc` publicado por cache-key neutralizado en CircleCI | Sí (corregido en v0.18.19) | Moderada: usuarios de v0.18.17/18 sin `uninstall` funcional |
| 2 | `Get-FileHash` ausente en PowerShell 5.1 | `PSModulePath` del proceso heredado con rutas de PS7 delante de las de 5.1 | No (entorno local) | Baja: solo afecta automatización con `powershell.exe` en esta máquina |
| 3 | Estimación de ~2 GB vs. 8.9 GB reales | Estimación basada en datos desactualizados; la documentación actual (README) es correcta | No | Nula |
| 4 | Mojibake en logs (`Instalaci??n`) | Consola cp850 + scripts `.ps1` UTF-8 sin BOM | No (cosmético) | Nula |

## 2. Hallazgo 1 — `uninstall` nativo roto en v0.18.17/0.18.18 (binario stale de CI, ya corregido)

### Síntoma

Sobre una instalación v0.18.18, `ai-voice-interconnector uninstall --force` (y toda variante: `--force --json`, `--no-daemon`, con `RUST_LOG=debug`/`RUST_BACKTRACE=1`) termina con **exit 1 y output completamente vacío** (stdout y stderr), sin borrar nada. `doctor --json` también fallaba con `doctor_checks_failed`. `--version` y `--help` sí respondían.

### Causa raíz

Binario stale publicado por el pipeline de CircleCI, documentado en CHANGELOG (sección v0.18.19) y corregido por `5840109`:

- El cache key de `target/` es `target-v2-...-{{ checksum "Cargo.lock.cachekey" }}` (`.circleci/config.yml:111`).
- `Cargo.lock.cachekey` se genera neutralizando la versión del crate raíz a `0.0.0` con `perl` (`.circleci/config.yml:104`), para que un bump de versión no invalide la caché de dependencias.
- Consecuencia: un bump de versión **sin cambios de dependencias** produce el mismo cache key → el job restaura un `target/` de la release anterior → `cargo build --release` con `sccache` relinkea un **binario previo a `67fc1bc`** (2026-08-12, "completar Fase 1 del host Rust"). El CHANGELOG de v0.18.19 lo dice textualmente: *"relinkeaba un binary stale previo a `67fc1bc`"*.
- El código fuente del tag v0.18.18 es correcto (dispatch de `uninstall` en `src/main.rs:328`, handlers íntegros): el defecto era del artefacto publicado, no del código. El binario relinkeado era de la era pre-Fase 1, con una superficie CLI/errores anterior, de ahí el exit 1 silencioso.

### Corrección y estado

- **Ya corregido** en v0.18.19 (commit `5840109`): `cargo clean --release` determinista en cada job `build-*` sobre tags `v*` + smoke-test de `version --json` validando `version == CIRCLE_TAG`, documentado en `docs/BUILD.md`.
- La prueba E2E confirma que el actualizador saca a un usuario de v0.18.18 directamente a la latest en 29 s — **el actualizador es la vía de escape** para los usuarios afectados (no depende del binario viejo ni de su PATH).

### Residual

Las releases v0.18.17 y v0.18.18 publicadas en GitHub Releases siguen defectuosas y no pueden retractarse. Un usuario en esas versiones no puede desinstalar con el comando nativo; su ruta de escape es re-ejecutar el instalador/actualizador (o limpieza manual documentada en el README de la release).

## 3. Hallazgo 2 — `Get-FileHash` ausente en PowerShell 5.1 (contaminación de `PSModulePath` del entorno local)

### Síntoma

En el primer intento de instalación, `install-windows.ps1` abortó con `El término 'Get-FileHash' no se reconoce como nombre de un cmdlet`. El cmdlet es parte estándar de `Microsoft.PowerShell.Utility` en PowerShell 5.1.

### Causa raíz

Contaminación del `PSModulePath` **del proceso**, no del registro:

- El `PSModulePath` heredado por la sesión lista 5 rutas con las 3 de PowerShell 7 **delante** de las de 5.1:
  - `C:\Users\...\Documents\PowerShell\Modules` (PS7 usuario)
  - `C:\Program Files\PowerShell\Modules` (PS7 sistema)
  - `c:\program files\powershell\7\Modules` (PS7 built-in)
  - `C:\Program Files\WindowsPowerShell\Modules` (5.1 sistema)
  - `C:\Windows\system32\WindowsPowerShell\v1.0\Modules` (5.1 built-in)
- El registro de Windows (`Machine`) está **sano**: solo contiene las 2 rutas de 5.1. La contaminación la inyecta el proceso padre al crear el entorno (launcher de la sesión), y `powershell.exe` 5.1 la hereda.
- Con las rutas de PS7 delante, 5.1 carga el `Microsoft.PowerShell.Utility` de PS7 (compilado para PowerShell Core), que exporta 0 comandos → `Get-FileHash` desaparece.
- Verificado en vivo: en un `powershell.exe` 5.1 con el env contaminado, `(Get-Command Get-FileHash -ErrorAction SilentlyContinue) -ne $null` → `False`; con el `PSModulePath` restaurado al estándar, el cmdlet existe.

### Impacto y mitigación

- No es un defecto del instalador ni del repositorio: en una máquina sin esa contaminación el flujo funciona (lo demostró la propia prueba).
- Mitigación aplicada durante la prueba (y recomendada para cualquier automatización en esta máquina): restaurar el `PSModulePath` estándar de 5.1 antes de invocar `powershell.exe`:
  `export PSModulePath="C:\Windows\system32\WindowsPowerShell\v1.0\Modules;C:\Program Files\WindowsPowerShell\Modules"`
- Alternativa práctica: usar `pwsh` (PowerShell 7 está instalado en la máquina y no le afecta la contaminación; `Get-FileHash` funciona).

## 4. Hallazgo 3 — Estimación de ~2 GB vs. 8.9 GB reales de modelos (error de estimación ajeno al repo)

### Síntoma

La planificación de la prueba estimó "~2 GB de modelos" para el `setup`; la provisión real descargó **8.9 GB**.

### Causa raíz

Error de estimación de la sesión de planificación, que usó cifras de estado antiguo del proyecto (era pre-STT/traducción ampliada). La documentación **actual** es correcta y exacta:

- `README.md` (líneas 131-133): *"Cinco modelos pinneados (4 + 1 opt-in)... `qwen3-tts-0.6b` (~4,7 GB), `marian-es-en`/`marian-en-es` (~3 GB), `parakeet-tdt-v3` (~600 MB, int8)... vía `setup` (~9 GB base, ~11,5 GB con `--with-base`)"*.
- Medición en vivo (caché HF, esta máquina): 4.7 GB (Qwen3-TTS) + 1.8 GB (opus-mt-en-es) + 1.3 GB (parakeet) + 1.2 GB (opus-mt-es-en) = **9.0 GB** — coincide con el README.

### Matiz adicional: ruta del `data_dir`

`data_dir()` (`crates/avi-store/src/lib.rs:6`) usa `directories::ProjectDirs::from("", "", "ai-voice-interconnector").data_dir()`, que en Windows resuelve a **`%APPDATA%` (Roaming)**: `C:\Users\<user>\AppData\Roaming\ai-voice-interconnector\data` (confirmado por `doctor --json`). El peso real de los modelos vive en la caché HF (`~/.cache/huggingface/hub`); el data_dir contiene manifiestos, registro y voces. La asunción intuitiva de `%LOCALAPPDATA%` es incorrecta — relevante para cálculos de espacio en entornos con roaming de perfiles de dominio.

### Estado

Sin acción. La documentación actual es exacta; la estimación errónea fue de la planificación de la prueba, no del repo.

## 5. Hallazgo 4 — Mojibake en los logs del instalador (codepage 850 vs. UTF-8 sin BOM)

### Síntoma

Los logs capturados en archivo muestran `A??adido`, `Instalaci??n`, `Resolviendo el ?ltimo` en los mensajes con acentos, en lugar de `Añadido`, `Instalación`, `último`.

### Causa raíz

Interacción de tres factores, todos verificados:

1. **Los `.ps1` están en UTF-8 sin BOM** (bytes iniciales `23 20 49` = `# I` en `install-windows.ps1`; `23 20 57` = `# W` en el wrapper: sin `EF BB BF`). PowerShell 5.1, sin BOM, decodifica el script con el ANSI de la configuración regional.
2. **La consola está en codepage 850** (`chcp` → "Página de códigos activa: 850"; `[Console]::OutputEncoding` → "Europa occidental (DOS)"), típico de Windows con locale español.
3. **La captura**: los bytes salen codificados en cp850 y el lector del redirect (Git Bash, UTF-8) los decodifica como UTF-8 → cada vocal acentuada UTF-8 de 2 bytes (`ó` = `C3 B3`) produce 2 × U+FFFD → `Instalaci??n`.

### Impacto

- Puramente cosmético: los literales afectados son mensajes `Write-Log` de UI; las rutas y comparaciones del instalador son ASCII pura, así que no hay riesgo funcional.
- Riesgo latente menor: PowerShell 5.1 leyendo un script como ANSI podría alterar literales no-ASCII usados en lógica. Hoy no ocurre (todo literal no-ASCII es de mensajería).

## 6. Recomendaciones (opcionales, pendientes de aprobación)

Ninguna recomendación es necesaria para la salud del proyecto; ambas son mejoras de pulido y **no se han ejecutado**:

1. **BOM UTF-8 en los `.ps1`** (`install-windows.ps1`, `upgrade-ai-voice-interconnector.ps1`, y de paso los `.tests.ps1`): PowerShell respeta siempre el BOM; con él, 5.1 decodificaría los scripts como UTF-8 y los logs serían legibles en cualquier codepage de consola. Eliminaría también el riesgo latente del hallazgo 4.
2. **Documentar la ruta Roaming del `data_dir`** en `docs/SELF-HOSTED-INSTALL.md` (hoy menciona `%LOCALAPPDATA%\Programs` para el binario, correcto, pero no explicita que los datos del usuario van a `%APPDATA%\Roaming`): evitaría cálculos de espacio erróneos y sorpresas en entornos con roaming de perfiles.

Adicionalmente, para los usuarios atrapados en v0.18.17/0.18.18 (hallazgo 1): no se requiere acción en el repo — el actualizador (o re-ejecutar el instalador one-liner) los lleva a una versión sana, como validó esta misma prueba.
