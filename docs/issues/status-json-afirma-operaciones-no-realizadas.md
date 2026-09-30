# El `status` JSON afirma operaciones que no se han realizado

| Campo | Valor |
|---|---|
| Estado | abierto |
| Severidad | baja |
| Tipo | contrato |
| Componente | CLI (`src/main.rs`, rama de `daemon stop`) y ciclo de vida (`self uninstall` y `cleanup`) |
| Versión detectada | 0.25.0 |
| Plataforma | Windows 11 con PowerShell 5.1; ninguna de las causas depende de la plataforma |
| Reproducibilidad | siempre |
| Detectado en | prueba E2E de v0.25.0, 2026-09-28 |

## Resumen

Tres comandos devuelven un `status` que describe una operación que no ha ocurrido:
`daemon stop` sin daemon dice que envió el apagado, `self uninstall --dry-run` dice que
desinstaló y `cleanup --dry-run` dice que limpió. El código de salida es correcto en los
tres, pero un consumidor que lea solo el `status` saca una conclusión falsa.

## Entorno

Binario v0.25.0 instalado desde cero en Windows 11 con PowerShell 5.1, con todos los
modelos provisionados.

## Precondiciones

Las propias de cada síntoma.

## Pasos para reproducir

Se detallan en cada síntoma, dentro de «Análisis de causa».

## Resultado observado

Se detalla en cada síntoma.

## Resultado esperado

El `status` describe lo que ha pasado, no lo que habría pasado ni lo que se pidió.

## Impacto y workaround

Afecta a los scripts que leen el `status`. Workaround: en `daemon stop`, consultar antes
`daemon status`; en los simulacros, leer también `dry_run`.

## Evidencia

Recogida en cada síntoma.

## Análisis de causa

### 1. `daemon stop` sin daemon dice «Señal de apagado enviada»

- **Síntoma:** con el daemon detenido, `daemon stop` imprime «Señal de apagado enviada
  al daemon en 127.0.0.1:8765.» y el JSON devuelve `status: "shutdown_sent"`,
  `daemon: "stopped"`.
- **Causa (confirmada):** en la rama de `daemon stop` de `src/main.rs`, el caso
  «ni activo ni vivo» borra el pidfile y reutiliza el mensaje del apagado real. El exit
  0 es correcto porque la operación es idempotente.
- **Esperado:** un mensaje del tipo «El daemon no estaba en ejecución» y un `status`
  propio (por ejemplo `not_running`).
- **Criterio:** texto y `status` distintos para «no había daemon» y «se apagó».

### 2. `self uninstall --dry-run --json` devuelve `status: "uninstalled"`

- **Síntoma:** el simulacro no modifica nada (correcto), pero el sobre dice
  `status: "uninstalled"` junto a `dry_run: true`.
- **Causa (confirmada por la salida):** el `status` describe el resultado que tendría
  la operación, no lo que ha pasado.
- **Esperado:** un `status` que no afirme el hecho, por ejemplo `planned` o
  `would_uninstall`.
- **Criterio:** prueba golden del dry-run con el `status` decidido.

### 3. `cleanup --dry-run --json` devuelve `status: "cleanup_complete"`

- **Síntoma:** el simulacro no modifica el disco (correcto), pero el sobre dice
  `status: "cleanup_complete"` junto a `dry_run: true`, y la lista `removed` contiene
  las rutas que se borrarían, no las borradas.
- **Causa (confirmada por lectura del código):** `simulate`
  (`crates/avi-lifecycle/src/cleanup.rs`) construye el mismo resultado que la limpieza
  real, con el mismo `status` y las mismas claves, para que el simulacro y la ejecución
  se puedan comparar. El único rastro de que no pasó nada es `dry_run: true`.
- **Esperado:** el mismo criterio que en el síntoma 2: un `status` que no afirme el
  hecho, conservando claves comparables con la ejecución real.
- **Criterio:** prueba golden del dry-run con el `status` decidido.

## Diagnóstico sugerido

Las causas están confirmadas y las correcciones son locales, pero las tres cambian un
contrato de máquina. Antes de implementarlas hay que decidir los valores nuevos de
`status`, si el cambio exige subir `schema_version` y cómo se anuncia en el CHANGELOG.
Conviene decidir las tres a la vez para que sigan el mismo criterio, y los dos
simulacros con un mismo valor.

## Criterio de aceptación

Decisión registrada en el contrato JSON y se cumplen los criterios de los tres síntomas.

## Relacionados

- Contrato JSON de la CLI: valores de `status` y `schema_version`.
