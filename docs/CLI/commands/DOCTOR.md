# `doctor`

Diagnóstico de entorno sin efectos secundarios: no descarga, no instala, no
inicia procesos. Verifica que los modelos pinneados estén provisionados, que
el directorio de datos exista y que el almacén de voces sea legible. Emite un
veredicto (texto o JSON) y termina con exit code `1` si algún chequeo
obligatorio falló.

Implementación: `handle_doctor` (`src/main.rs:2935`), apoyado en `avi-store`
(`crates/avi-store/src/lib.rs`: `ModelStore::is_provisioned`, `is_ct2_provisioned`,
`hf_cache_dir`, `data_dir`) y en `VoiceStore::list` (`crates/avi-store/src/lib.rs:115`).

---

## Superficie CLI

```
ai-voice-interconnector doctor [--json]
```

`Doctor` es una variante sin campos del enum `Commands` (`src/main.rs:209`): no
admite subcomandos ni flags propias. Solo hereda el flag global `--json`
(`src/main.rs:102-104`); los flags globales `--daemon`/`--no-daemon` no aplican
porque `doctor` nunca dialoga con el daemon (`src/main.rs:555`
`Some(Commands::Doctor) => handle_doctor(json_mode)`, llamada síncrona sin
`daemon_mode`).

---

## Flujo de chequeos

```
handle_doctor
    │
    ▼
data_dir() existe                              ← issue si falta (src/main.rs:2944-2948)
    │
    ▼
is_provisioned("qwen3-tts-0.6b")               ← issue si falta (src/main.rs:2951-2952)
is_provisioned("parakeet-tdt-v3")              ← issue si falta (src/main.rs:2954-2955)
is_provisioned("marian-es-en")
    │  false → issue "no provisionado"
    │  true  → is_ct2_provisioned("es-en")      ← issue con ficheros faltantes si incompleto (src/main.rs:2957-2960)
is_provisioned("marian-en-es")
    │  false → issue "no provisionado"
    │  true  → is_ct2_provisioned("en-es")      ← issue con ficheros faltantes si incompleto (src/main.rs:2962-2965)
    │
    ▼
is_provisioned("qwen3-tts-0.6b-base")          ← opt-in, NUNCA genera issue (src/main.rs:2968-2973)
    │  true  → base_status = "ready"
    │  false → base_status = "missing_opt_in"
    │
    ▼
voice_store.list()                             ← issue "Error al listar voces" si falla (src/main.rs:2976-2978)
    │
    ▼
Salida (JSON o texto) + Ok(()) si issues vacío, Err(CliError) si no
```

Son **6 chequeos obligatorios** (directorio de datos, TTS, STT, CT2 es→en, CT2
en→es, listado de voces) más **1 chequeo advisory** (modelo Base de clonado,
opt-in). Solo los obligatorios acumulan en `issues`; el Base nunca lo hace, ni
siquiera cuando falta.

A diferencia de `setup`, `doctor` nunca llama a `ensure_downloaded` ni a
`convert_marian_to_ct2`: solo lee el estado ya provisionado con las mismas
funciones de verificación que usa `setup` para decidir si saltarse un modelo
(`is_provisioned`, `is_ct2_provisioned`).

---

## Chequeo CT2 de traducción

`is_ct2_provisioned(pair)` (`crates/avi-store/src/lib.rs:575`) exige el
derivado completo, no solo el snapshot Marian: `model.bin` más un tokenizador
válido (`tokenizer.json`, o el par `source.spm`+`target.spm` que produce
`convert_marian_to_ct2`). Un `model.bin` huérfano sin tokenizador cuenta como
incompleto. El mensaje de issue incluye la lista exacta de ficheros faltantes
vía `ct2_archivos_faltantes(pair)` (`crates/avi-store/src/lib.rs:572`), por
ejemplo:

```
Modelo CT2 es→en incompleto en 'hf_cache_dir/ct2/opus-mt-es-en' (exige model.bin
más tokenizer.json o source.spm+target.spm) — ejecuta setup
```

Nótese que este chequeo depende del snapshot Marian correspondiente: si
`marian-es-en` no está provisionado, la issue reportada es la de snapshot
ausente, no la de CT2 incompleto (son ramas mutuamente excluyentes por
dirección, `src/main.rs:2957-2960`).

---

## Contrato `--json`

Con `--json`, `handle_doctor` emite un único objeto vía `emit_raw_json`
(`src/main.rs:2980-2988`), que inyecta `schema_version` automáticamente
(`crates/avi-core/src/json_emitter.rs:7-25`):

| Clave | Tipo | Significado |
|---|---|---|
| `schema_version` | string | Inyectada por `emit_raw_json`/`with_schema_version` |
| `status` | string | `"ok"` si `issues` está vacío, `"failed"` en otro caso |
| `data_dir` | string | Ruta de `store::data_dir()` |
| `hf_cache` | string | Ruta de `store::hf_cache_dir()` |
| `issues` | array de strings | Mensajes de los chequeos obligatorios fallidos (vacío si todo pasa) |
| `base_status` | string | `"ready"` o `"missing_opt_in"` — nunca afecta `status` ni exit code |

No hay array `checks` con entradas `{status, name, detail}`, ni contadores
`passed`/`failed`, ni campos `platform`/`python`: el contrato real es plano,
con `issues` como única fuente de detalle por chequeo.

**Particularidad cuando hay fallos:** `handle_doctor` retorna `Err(CliError)`
además de haber emitido su propio JSON a stdout. El bucle de `main`
(`src/main.rs:559-570`) trata ese error igual que el de cualquier otro
comando: con `--json` imprime un **segundo** objeto JSON en stdout,
`{"error": "...", "reason": "doctor_checks_failed"}` (también con
`schema_version` inyectado), antes de salir con `exit(1)`. Un consumidor de
`--json doctor` que falla debe esperar **dos objetos JSON concatenados** en
stdout, no uno solo.

---

## Salida en texto

Sin `--json` y sin issues:

```
Diagnóstico: todo correcto.
Cache HF: <ruta>
```

o, si el modelo Base de clonado no está provisionado:

```
Diagnóstico: todo correcto. [WARN] Modelo Base de clonado no provisionado (usa setup --with-voice-cloning).
Cache HF: <ruta>
```

Con issues, cada una se imprime en stderr con prefijo `✗`, seguida del WARN
del Base si aplica (prefijo `⚠`) y la línea de cache HF; luego `main` imprime
`Error: Chequeos de entorno fallaron` en stderr y sale con `exit(1)`
(`src/main.rs:3005-3018`).

---

## Errores

| Reason | Código | Causa |
|---|---|---|
| `doctor_checks_failed` | Error (1) | Al menos un chequeo obligatorio (directorio de datos, TTS, STT, CT2 es→en, CT2 en→es, listado de voces) falló |

`doctor` no define reasons propias adicionales: cualquier fallo obligatorio,
sin importar cuál, colapsa al mismo `reason` con el detalle en `issues`
(JSON) o en stderr (texto).

---

## Ejemplos

```bash
ai-voice-interconnector doctor                # reporte en texto, exit 0 o 1
ai-voice-interconnector --json doctor         # payload legible por máquina
```
