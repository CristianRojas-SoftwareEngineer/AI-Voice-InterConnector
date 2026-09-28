# Translate por daemon devuelve 404

Estado: abierto. Detectado en la E2E completa de v0.24.0 en Windows.

## Conducta observada

Con el daemon en `running` y `warm`, el modo automático de `translate`
delega al daemon y este responde 404:

```
ai-voice-interconnector translate --text 'Hola' --from es --to en --json
{"error": "error del daemon (HTTP 404 Not Found)", "reason": "daemon_error", "schema_version": "4"}
exit 1
```

En directo traduce bien:

```
ai-voice-interconnector translate --text 'Hola' --from es --to en --no-daemon --json
{"schema_version": "4", "source": "es", "target": "en", "translated": "Hello"}
exit 0
```

El passthrough `es → es` también sale con 0.

## Conducta esperada

El modo automático con daemon corriendo devuelve la traducción, como hace
`synthesize` por la misma vía.

## Pista

El router del daemon no sirve la ruta de traducción en esta compilación
(solo se comprobó el binario de release de Windows). Candidatos: ruta no
registrada o tras un flag de compilación que el release no activa.

## Alcance

Sin tocar hasta ciclo propio: la E2E cerró con este hallazgo anotado y el
modo directo cubre la función.
