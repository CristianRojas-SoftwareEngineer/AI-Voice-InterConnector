# `--text` de más de 5000 caracteres no se rechaza con exit 2

| Campo | Valor |
|---|---|
| Estado | abierto |
| Severidad | media |
| Tipo | contrato |
| Componente | binario principal (`src/main.rs`, validación de `speech synthesize` / `speech say`) y `avi-daemon` |
| Versión detectada | 0.25.0 |
| Plataforma | Windows 11. La causa no depende de la plataforma |
| Reproducibilidad | siempre |
| Detectado en | prueba E2E de v0.25.0, 2026-09-28 |

## Resumen

El contrato fija un tope de 5000 caracteres para `--text` y exige rechazarlo con exit 2
antes de despachar. El binario no tiene ese tope: envía el texto completo al motor TTS,
que no responde a tiempo, y la operación falla unos 32 s después con un error de socket
y exit 1. El usuario espera medio minuto para recibir un error que no explica la causa.

## Entorno

- Binario: v0.25.0 instalado, modelos provisionados.
- SO: Windows 11.
- Reproducido con `--no-daemon`. Por la causa, la vía daemon tampoco valida.

## Precondiciones

Modelos TTS provisionados (si faltan, el fallo sería `model_missing` y ocultaría este).

## Pasos para reproducir

```powershell
$t = 'a' * 5001
ai-voice-interconnector speech synthesize --no-daemon --text $t -o largo.wav
echo $LASTEXITCODE
```

Mismo resultado con `speech say`.

## Resultado observado

- El comando intenta sintetizar. Tras ~32 s falla con un error de E/S de socket
  (`os error 10060`, timeout de conexión en Windows).
- Exit 1.

## Resultado esperado

Contrato de la CLI, reglas de validación, regla 4: «`--text` no excede
`MAX_TEXT_LENGTH` (5000). Se valida en el cliente antes de cualquier despacho, con el
mismo código por ambas vías; el tope del daemon es defensa en profundidad». Las cinco
reglas de esa sección salen con exit 2. Por tanto:

- exit 2, inmediato, sin cargar el motor;
- mensaje que indique el tope y la longitud recibida;
- el mismo resultado con `--daemon`, `--no-daemon` y el modo automático;
- el daemon rechaza también un texto largo recibido por HTTP.

## Impacto y workaround

Quien pase por error un documento entero espera ~30 s y recibe un error de red que
apunta a un problema de conectividad y no a la longitud. Los scripts que distinguen el
uso inválido (2) de un fallo de ejecución reciben la categoría equivocada.

Workaround: partir el texto en fragmentos de hasta 5000 caracteres.

## Evidencia

- No existe ninguna constante `MAX_TEXT_LENGTH` ni un tope equivalente en los crates.
  La única validación de `--text` en `src/main.rs` es `text.trim().is_empty()` (regla 3).
- `crates/avi-tts/src/lib.rs`: la petición al servidor del motor (`POST /v1/tts` vía
  `http_exchange`) usa un timeout de 30 s. Encaja con los ~32 s observados: arranque
  del motor más el timeout.

## Análisis de causa

- **Confirmada.** La regla 4 venía de la implementación anterior en Python, donde
  existía `MAX_TEXT_LENGTH`, y no se portó a la implementación Rust. El contrato siguió
  describiéndola.
- **Confirmada.** Sin tope, un texto largo llega al motor, que no termina dentro del
  timeout de 30 s de `http_exchange`. El error de socket sube como fallo genérico
  (exit 1).
- **Por verificar.** Si el valor 5000 sigue siendo el adecuado para el motor Rust
  actual, y cuánto tarda de verdad con textos cercanos al tope. Un texto de 5000
  caracteres que tampoco termine en 30 s dejaría el tope sin cumplir su objetivo.

## Diagnóstico sugerido

1. Añadir la constante (en `avi-shared`, para que la compartan el cliente y el daemon)
   y validar en el cliente junto a la regla 3, con exit 2 y un `reason` de uso inválido.
2. Añadir el mismo chequeo en los handlers de síntesis del daemon, con 400.
3. Medir la síntesis de un texto de 5000 caracteres. Si supera el timeout de 30 s,
   decidir entre bajar el tope o ampliar el timeout en función de la longitud, y ajustar
   el contrato.

## Criterio de aceptación

- `speech synthesize` y `speech say` con 5001 caracteres salen con exit 2 en menos de
  un segundo, en los tres modos de despacho, sin lanzar el motor.
- Con exactamente 5000 caracteres, la validación pasa.
- El daemon responde 400 a una petición de síntesis con un texto que supera el tope.
- Pruebas golden de la CLI para 5000 y 5001 caracteres, y una prueba del handler del
  daemon.

## Relacionados

- Contrato de la CLI, reglas de validación, regla 4.
