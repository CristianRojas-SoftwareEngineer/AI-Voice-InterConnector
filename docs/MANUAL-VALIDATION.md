# Validación manual de la superficie de la CLI

Este documento es el **procedimiento operativo** de la validación end-to-end que
[docs/GOAL.md](GOAL.md) §"Validación E2E" define a nivel de política: el recorrido
manual `instalar → setup → síntesis real → desinstalar` que el propietario
ejecuta en Windows sobre cada release, y que sirve de guion para el feedback de
usuarios reales en Linux y macOS. El pipeline de CI solo corre un **smoke test
automatizado** del binario congelado (`tts-sidecar version`, exit 0); la matriz de
comandos de abajo es la parte que **no** cabe en un runner de CI porque exige
cargar los modelos Chatterbox, descargar ~8 GB de pesos (ambos modelos) y sintetizar audio real.

La secuencia está en orden lógico: cada paso asume que el anterior pasó. Ejecutar
tras instalar el artefacto del release (en Windows,
`tts-sidecar-X.Y.Z-x86_64-setup.exe`) y marcar la casilla de setup del
instalador, o bien desde una terminal nueva (el instalador agrega el `PATH`
automáticamente).

> Los comandos se muestran para una shell POSIX. En Windows (`cmd`/PowerShell)
> son equivalentes salvo `which tts-sidecar`, que allí es `where tts-sidecar`.

## Tabla de contenidos

- [1. Entorno y versión](#1-entorno-y-versión)
- [2. Diagnóstico del entorno](#2-diagnóstico-del-entorno)
- [3. Provisión del modelo](#3-provisión-del-modelo)
- [4. Dispositivos de audio](#4-dispositivos-de-audio)
- [5. Síntesis básica con la voz de fábrica](#5-síntesis-básica-con-la-voz-de-fábrica)
- [6. Gestión de voces](#6-gestión-de-voces)
- [7. Gestión de habla sintética](#7-gestión-de-habla-sintética)
- [8. Daemon](#8-daemon)
- [9. Casos de error esperados](#9-casos-de-error-esperados)

## 1. Entorno y versión

```bash
# Verificar que el comando está en el PATH (Windows: where tts-sidecar)
which tts-sidecar

# Versión legible por humano
tts-sidecar version

# Versión en JSON (contrato legible por máquina)
tts-sidecar version --json
```

## 2. Diagnóstico del entorno

```bash
# Diagnóstico completo: audio, modelo, dispositivos
tts-sidecar doctor

# Diagnóstico en JSON
tts-sidecar doctor --json
```

## 3. Provisión del modelo

Solo si no se hizo desde el instalador.

```bash
# Descarga ambos modelos (es-mx-latam y en) a ~/.cache/huggingface/hub (idempotente)
tts-sidecar setup
```

## 4. Dispositivos de audio

```bash
# Listar solo dispositivos de salida (render)
tts-sidecar devices

# En JSON
tts-sidecar devices --json
```

## 5. Síntesis básica con la voz de fábrica

```bash
# Reproducir con la voz de fábrica 'default' (sin audios explícitos)
tts-sidecar speech say --text "Hola mundo, esto es una prueba de síntesis de voz."

# Sintetizar y guardar como locución reutilizable (persiste siempre)
tts-sidecar speech synthesize --text "Guardando a archivo." --label prueba

# Forzar modo directo (sin daemon)
tts-sidecar speech say --text "Modo directo." --no-daemon
```

## 6. Gestión de voces

```bash
# Listar voces disponibles (debe aparecer 'default' de fábrica)
tts-sidecar voice list
tts-sidecar voice list --json

# Registrar una voz de usuario (requiere dos archivos WAV propios)
tts-sidecar voice clone --name mi_voz --timbre-reference timbre.wav --speech-reference habla.wav

# Verificar que aparece la nueva voz
tts-sidecar voice list

# Sintetizar con la voz registrada
tts-sidecar speech say --text "Esta es mi voz clonada." --voice mi_voz

# Guardar síntesis con voz registrada como locución reutilizable
tts-sidecar speech synthesize --text "Guardando con mi voz." --label saludo --voice mi_voz

# Eliminar la voz de usuario
tts-sidecar voice remove --name mi_voz

# Confirmar que se eliminó
tts-sidecar voice list
```

## 7. Gestión de habla sintética

El almacén de locuciones (`speech synthesize` las persiste; estas sub-acciones
operan sobre ellas sin re-sintetizar).

```bash
# Listar locuciones guardadas
tts-sidecar speech list
tts-sidecar speech list --voice mi_voz
tts-sidecar speech list --json

# Reproducir una locución guardada sin re-sintetizar
tts-sidecar speech play --label prueba

# Eliminar una locución guardada
tts-sidecar speech remove --label prueba
```

## 8. Daemon

```bash
# Iniciar el daemon en segundo plano
tts-sidecar daemon start

# Ver estado
tts-sidecar daemon status

# Síntesis vía daemon (automático si está corriendo)
tts-sidecar speech say --text "Síntesis con modelo en memoria." --daemon

# Reiniciar
tts-sidecar daemon restart

# Detener
tts-sidecar daemon stop

# Confirmar que se detuvo
tts-sidecar daemon status
```

## 9. Casos de error esperados

```bash
# Voz inexistente — debe mostrar mensaje en español con sugerencia
tts-sidecar speech say --text "Prueba." --voice voz_que_no_existe

# Eliminar voz inexistente — debe indicar que no fue encontrada
tts-sidecar voice remove --name voz_que_no_existe

# Etiqueta de locución inexistente — debe indicar que no fue encontrada
tts-sidecar speech play --label etiqueta_que_no_existe
```
