# Continuity prompt — validación completa de la superficie del CLI (v0.9.0)

Eres una sesión de Claude Code que continúa el trabajo sobre **TTS-Sidecar**.
El release **v0.9.0** quedó publicado en firme el 2026-07-31 (PyPI + GitHub
Release + binarios nativos, pipeline CircleCI #193 verde). Tu tarea es
**ejecutar y verificar toda la superficie del CLI en orden lógico, con y sin
daemon**, contra la versión publicada, y reportar un veredicto por comando.

## Estado del entorno al iniciar (ya preparado)

- La **instalación anterior** (uv tool `tts-sidecar` v0.7.8) fue **desinstalada**.
- El **caché del modelo** de HuggingFace fue **borrado** (`scripts/clean_build.py`
  eliminó `models--ResembleAI--Chatterbox-Multilingual-es-mx-latam` y
  `models--ResembleAI--chatterbox`). El primer `setup`/síntesis **re-descargará
  ~4 GB**; cuéntalo en los tiempos.
- El **data_root** (`%LOCALAPPDATA%\tts-sidecar` en Windows) está **vacío**: sin
  voces de usuario, sin locuciones, sin `daemon.pid`. No hay daemon corriendo.
- Sigue presente un **install editable de desarrollo** (pip, 0.9.0) que apunta al
  repo. **No lo elimines**: es el entorno de dev y la suite depende de él.

## Caveats que debes manejar

1. **No invoques nunca el bare command `tts-sidecar`.** El install editable de
   dev (`.../Python313/Scripts/tts-sidecar`, posición 28 del PATH) **siempre**
   tapa a cualquier install de `uv tool` (`~/.local/bin`, posición 48): el orden
   del PATH es fijo y el editable gana. Por eso este recorrido usa un **venv
   dedicado invocado por ruta absoluta** (variable `$TTS`, ver §Instalación).
   Esto no es opcional: el bare command validaría el código del repo, no el
   artefacto de PyPI. El daemon es seguro con este método porque se relanza con
   `sys.executable -m tts_sidecar.daemon.run` (`daemon.py`), heredando el python
   del venv y cargando el mismo 0.9.0 publicado.
2. **No existe `--version`.** La versión se consulta con el subcomando
   `tts-sidecar version` (y `version --json`). `--version` devuelve error de uso.
3. **`voice clone` requiere dos WAV propios**: `--timbre-reference` (cualquier
   largo) y `--speech-reference` (10+ s de habla limpia). Ten dos WAV a mano
   antes del paso 6; sin ellos, ese bloque no se puede ejercitar.

## Fuentes canónicas (léelas antes de empezar)

- `docs/MANUAL-VALIDATION.md` — recorrido base en orden lógico. **Esta es la
  secuencia madre**; el procedimiento de abajo la extiende con la matriz
  daemon/no-daemon y las aserciones de exit code. No la contradigas.
- `docs/CLI-CONTRACT.md` — contrato normativo de comandos, flags y payloads
  `--json`.
- `src/tts_sidecar/exit_codes.py` — tabla congelada de códigos de salida.

## Contrato de exit codes (para las aserciones)

```
0 éxito | 1 error genérico | 2 entrada inválida/uso | 3 recurso no encontrado
4 modelo no provisionado (setup) | 5 daemon inalcanzable | 6 conflicto de estado
7 no aplicable al contexto | 8 precondición de entorno | 130 interrupción (SIGINT)
```

Tras cada comando, captura el exit code (`echo $?` en bash) y compáralo con el
esperado. Con `--json`, valida además que el payload sea JSON parseable y que
incluya la clave `error` en los fallos.

## Instalación de la versión a probar

Objetivo: **artefacto PyPI 0.9.0**, en un **venv dedicado fuera del repo**, para
que ni el PATH ni el install editable interfieran. Ejecuta exactamente esto:

```bash
# 1. Venv aislado (fuera del repo; se borra al cerrar)
uv venv "$HOME/.tts-sidecar-validation"

# 2. Instala el artefacto publicado exacto en ese venv
uv pip install --python "$HOME/.tts-sidecar-validation/Scripts/python.exe" "tts-sidecar==0.9.0"

# 3. Fija el binario BAJO PRUEBA por ruta absoluta. En TODO el procedimiento,
#    donde el texto diga `tts-sidecar`, ejecuta "$TTS" en su lugar.
TTS="$HOME/.tts-sidecar-validation/Scripts/tts-sidecar.exe"

# 4. Confirma que es el publicado antes de seguir
"$TTS" version                 # DEBE imprimir 0.9.0; si no, detente y reporta
```

> **Regla firme:** cada comando de §Procedimiento se ejecuta como `"$TTS" ...`,
> nunca como `tts-sidecar ...` a secas. El bare command apunta al editable.
> (Ruta de Windows: el ejecutable vive en `Scripts/`, no en `bin/`.)

(Alternativa nativa: instalar el binario del GitHub Release v0.9.0 y apuntar
`$TTS` a él; el resto del recorrido es idéntico.)

## Procedimiento — superficie completa en orden lógico

Ejecuta en este orden; cada paso asume que el anterior pasó. Marca cada comando
como ✅/❌ con el exit code observado.

### 1. Identidad y diagnóstico (sin modelo aún)
- `tts-sidecar version` → 0; imprime `0.9.0`.
- `tts-sidecar version --json` → 0; JSON con la versión.
- `tts-sidecar doctor` → puede salir **1** si el modelo aún no está (chequeo
  fallido esperado); léelo como diagnóstico, no como bug.
- `tts-sidecar doctor --json` → JSON de diagnóstico.
- `tts-sidecar devices` y `--json` → 0; lista dispositivos de salida.

### 2. Provisión del modelo (re-descarga ~4 GB)
- `tts-sidecar setup` → 0; descarga el modelo es-mx-latam (idempotente).
- Repite `tts-sidecar doctor` → ahora debe salir **0** (modelo presente).

### 3. Síntesis con la voz de fábrica — **sin daemon**
- `tts-sidecar speech say --text "Hola mundo, prueba de síntesis." --no-daemon` → 0.
- `tts-sidecar speech say --text "Voz de fábrica." --no-daemon --json` → 0; JSON con la voz efectiva.
- `tts-sidecar speech synthesize --text "Guardando a archivo." --label prueba --no-daemon` → 0.
- `tts-sidecar speech synthesize --text "Otra toma." --label prueba --no-daemon` → **6** (colisión de etiqueta).
- `tts-sidecar speech synthesize --text "Sobrescrita." --label prueba --no-daemon --force` → 0.

### 4. Almacén de locuciones (agnóstico al daemon)
- `tts-sidecar speech list` / `--json` → 0; aparece `prueba`.
- `tts-sidecar speech play --label prueba` → 0 (reproduce sin re-sintetizar).
- `tts-sidecar speech remove --label prueba` → 0.
- `tts-sidecar speech play --label prueba` → **3** (ya no existe).

### 5. Voces de fábrica (agnóstico al daemon)
- `tts-sidecar voice list` / `--json` → 0; aparece `default`.

### 6. Clonación de voz — **sin y con daemon** (requiere 2 WAV)
- `tts-sidecar voice clone --name mi_voz -t timbre.wav -s habla.wav --no-daemon` → 0.
- `tts-sidecar voice clone --name mi_voz -t timbre.wav -s habla.wav --no-daemon` → **6** (ya existe) y con `--force` → 0.
- `tts-sidecar speech say --text "Mi voz clonada." --voice mi_voz --no-daemon` → 0.
- Deja `mi_voz` registrada para el paso 7 (se reusará vía daemon), o re-clónala allí.

### 7. Camino con daemon — **misma síntesis, modelo en memoria**
- `tts-sidecar daemon status` → daemon detenido (exit distinto de 0 esperado; anótalo).
- `tts-sidecar daemon start` → 0; luego `daemon status` → 0 (corriendo).
- `tts-sidecar speech say --text "Síntesis vía daemon." --daemon` → 0.
- `tts-sidecar speech say --text "Con voz vía daemon." --voice mi_voz --daemon` → 0.
- `tts-sidecar speech synthesize --text "Guardada vía daemon." --label demo --daemon --json` → 0; JSON con tiempos t3/s3gen y marca de que fue vía daemon.
- `tts-sidecar voice clone --name mi_voz2 -t timbre.wav -s habla.wav --daemon` → 0 (precómputo vía daemon).
- `tts-sidecar daemon restart` → 0; `daemon status` → 0.
- `tts-sidecar daemon stop` → 0; `daemon status` → daemon detenido.
- **Modo estricto:** con el daemon detenido, `tts-sidecar speech say --text "x" --daemon` → **5** (exige daemon y no está).
- (Opcional) `tts-sidecar daemon serve` en primer plano en otra terminal y Ctrl-C → **130**.

### 8. Limpieza de datos (`cleanup`) — verifica antes de borrar
- `tts-sidecar cleanup --dry-run` → 0; lista qué borraría sin borrar.
- `tts-sidecar cleanup --voices --yes` → 0 (borra voces de usuario, no `default`).
- `tts-sidecar voice list` → ya no aparecen `mi_voz`/`mi_voz2`.
- `tts-sidecar cleanup --json` **sin** `--yes`/`--dry-run` → **2** (requiere uno de esos).

### 9. Casos de error esperados (cierre del contrato)
- `tts-sidecar speech say --text "x" --voice no_existe` → **3**; mensaje en español con sugerencia.
- `tts-sidecar voice remove --name no_existe` → **3**.
- `tts-sidecar speech say --text "x" --daemon --no-daemon` → **2** (flags mutuamente excluyentes).
- `tts-sidecar speech synthesize --text "x" --label y --play --json` → **2** (`--play` incompatible con `--json`).

## Cierre (opcional, según lo que quiera el propietario)

Con el método de venv dedicado, deshacer la instalación bajo prueba es borrar el
venv: `rm -rf "$HOME/.tts-sidecar-validation"` (no toca PATH ni el editable de
dev). `setup --uninstall --yes` aplica solo a instalaciones **nativas** (encadena
`cleanup --all`, revierte PATH y borra el binario); no aplica aquí. **No ejecutes
la desinstalación ni el borrado del venv sin confirmarlo con el propietario.**

## Criterio de éxito y reporte

- Éxito: cada comando devuelve el exit code esperado y, con `--json`, un payload
  parseable coherente con `docs/CLI-CONTRACT.md`.
- Entrega una tabla por comando (comando · modo · exit esperado · exit observado ·
  ✅/❌ · nota). Para cualquier ❌, documenta el comando exacto, stdout/stderr y el
  exit code, y contrástalo con el contrato antes de concluir si es bug o esperado.
- Recuerda: `doctor` sin modelo y `daemon status` sin daemon **no** son fallos.
