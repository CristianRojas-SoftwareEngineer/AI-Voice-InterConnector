Ya tengo el mapa completo y la evidencia. Aquí está la evaluación con crítica adversarial-constructiva.

---

# Mapa de la documentación

**24 documentos versionados** (excluyo `.venv/`, `.claude/skills/`, `.pytest_cache/` — son dependencias/tooling, no documentación del proyecto).

**Raíz (10):**
| Doc                                          | Rol                          | Estado                                                 |
| -------------------------------------------- | ---------------------------- | ------------------------------------------------------ |
| `README.md`                                  | Portada / entrada            | Correcto                                               |
| `USAGE.md` (1274 líneas)                     | Manual de usuario            | Correcto                                               |
| `CONTRIBUTING.md`                            | Flujo de contribución        | Correcto                                               |
| `SECURITY.md`                                | Política + modelo de amenaza | Correcto                                               |
| `CHANGELOG.md`                               | Historial                    | Correcto                                               |
| `SOURCE-OFFER.md`, `THIRD-PARTY-LICENSES.md` | Compliance GPL               | Correcto (generados)                                   |
| `AGENTS.md` / `CLAUDE.md`                    | Guía de agentes              | **Hard link** (mismo inodo) — dualidad intencional, OK |
| `.github/PULL_REQUEST_TEMPLATE.md`           | Plantilla                    | Correcto                                               |

**`docs/` (13):** GOAL, ROADMAP, DESIGN, ARCHITECTURE, CLI-CONTRACT, DAEMON-MODE, DISTRIBUTION, SELF-HOSTED-INSTALL, BUILD, RELEASING, PARITY, MANUAL-VALIDATION, CLAUDE-CODE-INTEGRATION.

> **Nota de seguimiento:** los hallazgos **#6 y #7 ya están resueltos** (ver marcas ✅ abajo). El conteo de `docs/` refleja el estado posterior a la fusión de #6 (14 → 13).

En general la documentación es **excepcionalmente rigurosa** y la separación *especificación ideal* (GOAL) / *estado vivo* (ROADMAP/PARITY) / *contrato normativo* (CLI-CONTRACT) es un acierto deliberado y bien ejecutado. Los problemas son concretos y acotados.

---

# Hallazgos, de mayor a menor severidad

## 🔴 1. `ARCHITECTURE.md` es un subconjunto obsoleto de `DESIGN.md` — un doc debería absorber al otro

Es el hallazgo más fuerte. Los dos comparten **casi todo**: el diagrama ASCII de arquitectura (idéntico), «Estructura del proyecto», «Flujo de síntesis», «Modelo/Clonación de voces», «Extensibilidad» (literalmente los mismos 3 pasos), «Modelos disponibles».

Y `ARCHITECTURE.md` está **desactualizado**. Verifiqué el paquete real:

```
audio_writer, compute_backend, conditionals, engine, exceptions,
exit_codes, model_loader, paths, synthesis, synthetic_speech, voices...
```

- `DESIGN.md` documenta `engine.py` como «Façade / composition root» + `compute_backend`, `audio_writer`, `synthesis`, `model_loader`, `conditionals` → refleja el refactor.
- `ARCHITECTURE.md` aún dice `engine.py # Wrapper de ChatterboxTTS` y no menciona ninguno de esos módulos → estado **pre-refactor**.

Lo único con valor propio en `ARCHITECTURE.md` es la sección **«El entry point `bin/tts-sidecar`»** (racional del shebang y la ausencia de extensión `.py`), que `DESIGN.md` no tiene.

**Recomendación:** mover esa única sección a `DESIGN.md` y **eliminar `ARCHITECTURE.md`**. Hoy mantienes dos documentos con el mismo cometido y uno miente. `README.md` enlaza ambos por separado («Diseño técnico» / «Arquitectura del sistema»), una distinción que no se sostiene al leerlos.

## 🔴 2. El árbol de estructura del proyecto está triplicado y los tres divergen

`GOAL.md`, `DESIGN.md` y `ARCHITECTURE.md` contienen cada uno un árbol `src/tts_sidecar/…`, y **los tres son distintos y ninguno está completo** (faltan `exceptions.py`, `exit_codes.py`, `synthetic_speech.py` en todos; `ARCHITECTURE` además omite el refactor entero). Triple mantenimiento garantiza deriva.

**Recomendación:** un solo árbol canónico (en `DESIGN.md`), y que `GOAL.md` lo referencie en vez de copiarlo. La estructura del proyecto no es «especificación ideal» — es estado, no pertenece a GOAL.

## 🟠 3. El «Plan técnico … EJECUTADO (v0.6.0)» infla `ROADMAP.md` con historia terminada

De las 354 líneas de `ROADMAP.md`, ~280 son un plan de implementación prescriptivo de la desinstalación multiplataforma, **marcado como ya ejecutado** y conservado «como registro del diseño implementado». Un roadmap describe lo *pendiente*; un plan ya ejecutado es historia y su hogar natural es git/`CHANGELOG`. Adversarialmente: el estado real que ese documento debería comunicar («no queda trabajo pendiente salvo firma de código») cabe en 30 líneas; las otras 250 son ruido que hay que saltar para llegar a la señal.

**Recomendación:** recortar el plan ejecutado a un párrafo de cierre con puntero al commit/CHANGELOG, o archivarlo. Mismo patrón afecta a `SELF-HOSTED-INSTALL.md` (ver #5).

## 🟠 4. `SELF-HOSTED-INSTALL.md` es un plan mayormente ejecutado que ahora duplica a `DISTRIBUTION.md` + `PARITY.md`

Está redactado como plan (secciones «Entregable», «Tests», «Cierre», «Orden de implementación» por pieza), pero las cinco piezas ya están shipeadas. Su contenido de estado (one-liners, checksum, `--uninstall`, MOTW) ya vive —y mejor contextualizado— en `DISTRIBUTION.md` (canales) y `PARITY.md` (brechas). Hoy hay tres documentos que cuentan la misma historia de instalación desde ángulos que se solapan.

**Recomendación:** decidir su rol. O bien se convierte en el **doc de diseño de referencia** de los instaladores (y `DISTRIBUTION`/`PARITY` lo referencian sin repetir), o se archiva ahora que el plan se cumplió. Tal como está, es el tercer lugar donde se explica lo mismo.

## 🟡 5. La explicación de MOTW / SmartScreen / Gatekeeper está repetida ~6 veces

El mismo razonamiento (el navegador sella `ZoneId=3`, la descarga por CLI no, la firma de código es el fondo diferido) aparece casi verbatim en: `README.md`, `SECURITY.md`, `DISTRIBUTION.md`, `SELF-HOSTED-INSTALL.md`, `BUILD.md`, `PARITY.md` y `GOAL.md`. Cada copia es correcta, pero son 7 lugares a mantener sincronizados; un matiz que cambie obliga a editar los siete.

**Recomendación:** designar **una** sede canónica (candidato natural: `SECURITY.md` §«Artefactos sin firmar») y que el resto la enlace con una frase de una línea, en vez de reexplicar el mecanismo.

## 🟡 6. `CLAUDE-CODE-PLUGIN.md` y `NARRATION-INTEGRATION.md`: dos docs de ~67 líneas sobre el mismo tema que se apuntan mutuamente ✅ RESUELTO

Ambos tratan del plugin `tts-sidecar-narrator` (externo), se cruzan referencias entre sí, y juntos suman ~135 líneas. `CLAUDE-CODE-PLUGIN.md` se declara a sí mismo «hoy es un puntero». La segmentación no aporta: un lector que llega a uno necesita el otro.

**Recomendación:** fusionar en un único `NARRATION-INTEGRATION.md` (el contrato + el puntero al repo externo + el resumen de qué es el plugin). Es segmentación excesiva de un tema pequeño.

**Resolución:** fusionados en `docs/CLAUDE-CODE-INTEGRATION.md` (nombre elegido por concreción: hoy documenta un plugin de Claude Code y no procede generalizar). Se rescató el contrato de integración intacto y el resumen del plugin; el detalle interno del plugin queda en su repo. Eliminados los dos originales. `docs/` pasa de 14 a 13.

## 🟢 7. `DESIGN.md` duplica referencia de CLI sin valor de diseño ✅ RESUELTO

Las secciones «Comandos CLI» e «Invocación desde otros lenguajes» de `DESIGN.md` repiten lo que ya están en `README.md` y `USAGE.md` (esta última como manual normativo). En un doc de *diseño* no aportan; su lugar es el manual.

**Recomendación (menor):** adelgazar esas secciones a un puntero a `USAGE.md`/`CLI-CONTRACT.md`.

**Resolución:** ambas secciones reemplazadas por un único párrafo-puntero a `USAGE.md`, `CLI-CONTRACT.md` y `README.md#invocación-desde-cualquier-lenguaje`; se corrigió el ancla del README y se limpió la tabla de contenidos. DESIGN.md pierde ~55 líneas de referencia duplicada.

---

# Ubicación de archivos

- **Raíz:** todo correcto y convencional. `USAGE.md` en raíz (y no en `docs/`) es defendible por ser el manual de usuario primario, análogo a README.
- **`docs/`:** 14 archivos planos que mezclan cinco géneros (spec ideal, estado vivo, contrato normativo, proceso, planes). No es un problema de ubicación *incorrecta* —todos pertenecen a `docs/`— pero si crece, una subdivisión ligera (`docs/design/`, `docs/ops/`) ayudaría. No es urgente.
- No encontré ningún documento fuera de lugar ni huérfano.

---

# Resumen accionable priorizado

| #   | Acción                                                                                                 | Tipo                          |
| --- | ------------------------------------------------------------------------------------------------------ | ----------------------------- |
| 1   | Fusionar `ARCHITECTURE.md`→`DESIGN.md` (rescatar sección entry point) y **eliminar** `ARCHITECTURE.md` | Absorción                     |
| 2   | Árbol de estructura único en `DESIGN.md`; `GOAL.md` lo referencia                                      | De-duplicar + corregir deriva |
| 3   | Recortar el «Plan técnico EJECUTADO» de `ROADMAP.md`                                                   | Podar historia                |
| 4   | Resolver el rol de `SELF-HOSTED-INSTALL.md` (referencia de diseño o archivo)                           | Solapamiento                  |
| 5   | Sede única para MOTW/SmartScreen; los demás enlazan                                                    | De-duplicar                   |
| 6 ✅ | Fusionar `CLAUDE-CODE-PLUGIN.md` + `NARRATION-INTEGRATION.md` → `CLAUDE-CODE-INTEGRATION.md`           | Sobre-segmentación            |
| 7 ✅ | Adelgazar «Comandos CLI» de `DESIGN.md`                                                                | Menor                         |

Ninguno de estos cambios toca código ni contratos; son reorganización documental. Los hallazgos #1, #2 y #6 son los de mayor retorno (eliminan deriva activa y un documento que ya miente).

¿Quieres que ejecute alguno? Sugiero empezar por **#1+#2** (elimina el documento obsoleto y la deriva del árbol de una sola vez). Dime cuáles apruebas y procedo.