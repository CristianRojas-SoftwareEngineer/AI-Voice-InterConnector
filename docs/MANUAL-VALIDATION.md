# Validación manual de la superficie de la CLI

Este documento es el **procedimiento operativo** de la validación end-to-end que
[docs/GOAL.md](GOAL.md) §"Validación E2E" define a nivel de política: el recorrido
manual `instalar → setup → síntesis real → desinstalar` que el propietario
ejecuta en Windows sobre cada release, y que sirve de guion para el feedback de
usuarios reales en Linux y macOS. El pipeline de CI solo corre un **smoke test
automatizado** del binario congelado (`ai-voice-interconnector version`, exit 0); la matriz de
comandos de abajo es la parte que **no** cabe en un runner de CI porque exige
cargar los modelos Chatterbox, descargar ~6 GB de pesos (ambos modelos) y sintetizar audio real.

La secuencia está en orden lógico: cada paso asume que el anterior pasó. Ejecutar
tras instalar el artefacto del release (en Windows,
`ai-voice-interconnector-X.Y.Z-x86_64-setup.exe`) y marcar la casilla de setup del
instalador, o bien desde una terminal nueva (el instalador agrega el `PATH`
automáticamente).

> Los comandos se muestran para una shell POSIX. En Windows (`cmd`/PowerShell)
> son equivalentes salvo `which ai-voice-interconnector`, que allí es `where ai-voice-interconnector`.

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
# Verificar que el comando está en el PATH (Windows: where ai-voice-interconnector)
which ai-voice-interconnector

# Versión legible por humano
ai-voice-interconnector version

# Versión en JSON (contrato legible por máquina)
ai-voice-interconnector version --json
```

## 2. Diagnóstico del entorno

```bash
# Diagnóstico completo: audio, modelo, dispositivos
ai-voice-interconnector doctor

# Diagnóstico en JSON
ai-voice-interconnector doctor --json
```

## 3. Provisión del modelo

Solo si no se hizo desde el instalador.

```bash
# Descarga ambos modelos (es-mx-latam y en) a ~/.cache/huggingface/hub (idempotente)
ai-voice-interconnector setup
```

## 4. Dispositivos de audio

```bash
# Listar solo dispositivos de salida (render)
ai-voice-interconnector devices

# En JSON
ai-voice-interconnector devices --json
```

## 5. Síntesis básica con la voz de fábrica

```bash
# Reproducir con la voz de fábrica 'default' (sin audios explícitos)
ai-voice-interconnector speech say --text "Hola mundo, esto es una prueba de síntesis de voz."

# Sintetizar y guardar como locución reutilizable (persiste siempre)
ai-voice-interconnector speech synthesize --text "Guardando a archivo." --label prueba

# Forzar modo directo (sin daemon)
ai-voice-interconnector speech say --text "Modo directo." --no-daemon
```

## 6. Gestión de voces

```bash
# Listar voces disponibles (debe aparecer 'default' de fábrica)
ai-voice-interconnector voice list
ai-voice-interconnector voice list --json

# Registrar una voz de usuario con una sola muestra (caso base: --speech-reference,
# ≥10s, es el único obligatorio; el habla cubre también el Voice Encoder)
ai-voice-interconnector voice clone --name mi_voz --speech-reference habla.wav

# Registrar una voz de usuario con timbre y habla por separado (--timbre-reference
# es opcional; útil para separar timbre y prosodia)
ai-voice-interconnector voice clone --name mi_voz_dual --timbre-reference timbre.wav --speech-reference habla.wav

# Verificar que aparece la nueva voz
ai-voice-interconnector voice list

# Sintetizar con la voz registrada
ai-voice-interconnector speech say --text "Esta es mi voz clonada." --voice mi_voz

# Guardar síntesis con voz registrada como locución reutilizable
ai-voice-interconnector speech synthesize --text "Guardando con mi voz." --label saludo --voice mi_voz

# Eliminar la voz de usuario
ai-voice-interconnector voice remove --name mi_voz

# Confirmar que se eliminó
ai-voice-interconnector voice list
```

## 7. Gestión de habla sintética

El almacén de locuciones (`speech synthesize` las persiste; estas sub-acciones
operan sobre ellas sin re-sintetizar).

```bash
# Listar locuciones guardadas
ai-voice-interconnector speech list
ai-voice-interconnector speech list --voice mi_voz
ai-voice-interconnector speech list --json

# Reproducir una locución guardada sin re-sintetizar
ai-voice-interconnector speech play --label prueba

# Eliminar una locución guardada
ai-voice-interconnector speech remove --label prueba
```

## 8. Daemon

```bash
# Iniciar el daemon en segundo plano
ai-voice-interconnector daemon start

# Ver estado
ai-voice-interconnector daemon status

# Síntesis vía daemon (automático si está corriendo)
ai-voice-interconnector speech say --text "Síntesis con modelo en memoria." --daemon

# Reiniciar
ai-voice-interconnector daemon restart

# Detener
ai-voice-interconnector daemon stop

# Confirmar que se detuvo
ai-voice-interconnector daemon status
```

## 9. Casos de error esperados

```bash
# Voz inexistente — debe mostrar mensaje en español con sugerencia
ai-voice-interconnector speech say --text "Prueba." --voice voz_que_no_existe

# Eliminar voz inexistente — debe indicar que no fue encontrada
ai-voice-interconnector voice remove --name voz_que_no_existe

# Etiqueta de locución inexistente — debe indicar que no fue encontrada
ai-voice-interconnector speech play --label etiqueta_que_no_existe
```
