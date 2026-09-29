# `--audio` con un archivo inexistente sale con 10 en vez de 3

| Campo | Valor |
|---|---|
| Estado | abierto |
| Severidad | media |
| Tipo | contrato |
| Componente | binario principal (`src/main.rs`, `speech transcribe` y `speech dub`) |
| Versión detectada | 0.25.0 |
| Plataforma | Windows 11. La causa no depende de la plataforma |
| Reproducibilidad | siempre |
| Detectado en | prueba E2E de v0.25.0, 2026-09-28 |

## Resumen

Si se pasa a `speech transcribe` o `speech dub` un `--audio` que no existe, el binario
sale con 10 (`TranscriptionFailed`, «el pipeline de transcripción falló con el modelo
ya cargado») en vez de 3 (`NotFound`). Un script no puede distinguir una ruta mal
escrita de un fallo del modelo.

## Entorno

- Binario: v0.25.0 instalado, modelos provisionados.
- SO: Windows 11.
- Reproducido con `--no-daemon`. Por la causa, también afecta a la vía daemon.

## Precondiciones

Modelo de transcripción provisionado. Si faltara, el chequeo de modelo (exit 4) corre
antes y oculta este defecto.

## Pasos para reproducir

```powershell
ai-voice-interconnector speech transcribe --no-daemon --audio nofile.wav --source-language es-latam
echo $LASTEXITCODE
ai-voice-interconnector speech dub --no-daemon --audio nofile.wav --source-language es-latam --target-language en
echo $LASTEXITCODE
```

## Resultado observado

Exit 10, `reason` `transcription_error`. El mensaje incluye el error del sistema de
archivos (`os error 2`, archivo no encontrado).

## Resultado esperado

El contrato de la CLI lo dice en la sección de `speech transcribe`: «Un `--audio`
inexistente sale con 3 (`ExitCode::NotFound`)». La sección de `speech dub` lista
«**3** (`--audio` inexistente)» entre sus códigos. La tabla de códigos define 3 como
«el recurso nombrado no existe».

## Impacto y workaround

Los scripts que reaccionan al código de salida clasifican una ruta errónea como fallo
del modelo, y pueden reintentar o pedir que se reinstale el modelo sin motivo. El
mensaje sí permite al humano deducir la causa.

Workaround: comprobar que el archivo existe antes de invocar la CLI.

## Evidencia

En `src/main.rs`, las cuatro llamadas a `avi_audio::load_wav_16k_mono_pcm` (transcribe
directo, transcribe por daemon en `transcribe_via_daemon`, dub directo y dub por
daemon) convierten **cualquier** error de carga en
`CliError::new(ExitCode::TranscriptionFailed, "transcription_error", …)`. Antes de esas
llamadas no se comprueba que el archivo exista; el único chequeo previo es el de modelo
ausente (exit 4).

## Análisis de causa

- **Confirmada.** Falta el chequeo de existencia de `--audio` y el error de carga se
  mapea a un único código. El error `NotFound` del sistema de archivos se pierde dentro
  de `transcription_error`.
- Descartado: no es un problema del daemon. En la vía daemon el audio se carga en el
  cliente antes de enviar las muestras, así que el fallo ocurre en el mismo punto.

## Diagnóstico sugerido

Validar la existencia de `--audio` una sola vez, junto a las demás validaciones de uso y
antes del chequeo de modelo si así lo dicta el orden del contrato. Si no existe, salir
con exit 3 y un `reason` de recurso no encontrado. Así los cuatro puntos de carga no
necesitan distinguir errores.

Queda por decidir qué hacer con un archivo que existe pero no es un WAV válido. Hoy
sale con 10. El contrato reserva 10 para fallos «con el modelo ya cargado», así que
podría encajar mejor como uso inválido (2); conviene decidirlo y anotarlo en el
contrato.

## Criterio de aceptación

- `speech transcribe --audio <inexistente>` y `speech dub --audio <inexistente>` salen
  con 3 en los tres modos de despacho, sin cargar modelos.
- Pruebas golden de la CLI para los dos subcomandos.

## Relacionados

- Contrato de la CLI: secciones de `speech transcribe` y `speech dub`, y tabla de
  códigos de salida.
