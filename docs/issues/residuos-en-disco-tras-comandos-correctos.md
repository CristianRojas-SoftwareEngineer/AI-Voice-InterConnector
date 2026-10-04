# Comandos que terminan bien dejan residuos en disco

| Campo | Valor |
|---|---|
| Estado | abierto |
| Severidad | baja |
| Tipo | funcional |
| Componente | CLI (`src/main.rs`), daemon (`crates/avi-daemon`) |
| Versión detectada | 0.25.0 |
| Plataforma | Windows 11 con PowerShell 5.1 |
| Reproducibilidad | siempre |
| Detectado en | prueba E2E de v0.25.0, 2026-09-28 |

## Resumen

Varios comandos que terminan con el resultado correcto dejan en disco ficheros que ya no
sirven: WAV temporales.
Todo queda a cargo de `cleanup`, cuando el criterio debería ser que quien crea un
fichero lo borre.

## Entorno

Binario v0.25.0 instalado desde cero en Windows 11 con PowerShell 5.1, con todos los
modelos provisionados y el daemon en el puerto por defecto.

## Precondiciones

Las propias de cada síntoma.

## Pasos para reproducir

Se detallan en cada síntoma, dentro de «Análisis de causa».

## Resultado observado

Se detalla en cada síntoma.

## Resultado esperado

Cada comando retira lo que crea al terminar, también si falla.

## Impacto y workaround

Los residuos ocupan disco y ensucian `doctor`. Workaround: `cleanup` barre los
temporales.

## Análisis de causa

### `speech say` y `speech dub` dejan WAV en `%TEMP%` o informan de uno ya borrado

- **Síntoma:** los dos comandos sintetizan a un WAV temporal para reproducirlo y
  devuelven su ruta en `audio_path` (en modo humano, en «Reproduciendo: …» y «Doblaje
  reproducido: …»). Según la vía:
  - `speech say` por la vía directa deja `avi_say_<pid>.wav` en el directorio temporal;
  - `speech say` por la vía daemon lo borra, pero después de emitir su ruta, así que
    `audio_path` apunta a un fichero que ya no existe; si la reproducción falla, sale
    antes de borrarlo y el WAV queda en disco;
  - `speech dub` deja `avi_dub_<pid>.wav` en todas sus vías: directa, daemon y la
    composición para daemons sin `/dub`.

  Los que quedan solo desaparecen con `cleanup`.
- **Causa (confirmada):** `src/main.rs` escribe los temporales en `std::env::temp_dir()`
  y cada vía gestiona su vida por su cuenta: solo `say_via_daemon` borra el fichero, y lo
  hace al final, sin cubrir el fallo de la reproducción ni retirar la ruta del JSON.
- **Esperado:** quien crea el temporal lo borra al terminar la reproducción, también si
  falla, en todas las vías, y el JSON no informa de una ruta que no va a existir.
- **Criterio:** tras `speech say` y `speech dub`, por todas sus vías y también cuando
  falla la reproducción, `%TEMP%` no contiene `avi_say_*.wav` ni `avi_dub_*.wav`, y el
  JSON no contiene una ruta a un fichero inexistente.

## Diagnóstico sugerido

El síntoma tiene corrección local.

## Criterio de aceptación

Se cumple el criterio del síntoma.

## Relacionados

- Documentación de `cleanup` y de `doctor` (`pending_artifacts`).
