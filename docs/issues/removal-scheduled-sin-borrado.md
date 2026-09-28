# Self uninstall responde removal_scheduled pero el directorio persiste

Estado: abierto. Detectado dos veces en la E2E completa de v0.24.0 en Windows.

## Conducta observada

```
ai-voice-interconnector self uninstall --yes --json
{"dry_run": false, "path_reverted": true, "reason": null, "removed": [...data...],
 "schema_version": "4", "status": "removal_scheduled"}
exit 0
```

Tras salir el proceso (cero procesos propios corriendo), el directorio del
programa sigue íntegro en disco. No hay tarea programada ni entrada de
borrado al reinicio que lo reclame; se retiró a mano en ambas ocasiones para
alcanzar residuo cero.

## Conducta esperada

`removal_scheduled` termina con el directorio borrado sin intervención:
o el proceso diferido lo borra, o el estado miente y debe decir qué falta.

## Pista

El mecanismo diferido de Windows no deja rastro observable (ni tarea, ni
RunOnce, ni renombre pendiente) y no completa. Revisar cómo se programa el
borrado del directorio en uso y qué condición lo cancela en silencio.

## Alcance

Sin tocar hasta ciclo propio. El resto del uninstall (recibo, datos, PATH
con tipo conservado) sí queda en cero.
