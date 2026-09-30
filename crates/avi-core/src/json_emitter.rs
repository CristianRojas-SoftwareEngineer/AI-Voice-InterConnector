use serde_json::Value;
use std::io::Write;

/// Versión del sobre `--json` de la **CLI**.
///
/// Sube a `"4"` por un cambio incompatible: el ciclo de vida retira y renombra claves
/// del reporte de `doctor` (`data_dir`, `hf_cache`, `base_status` e `issues` salen, y la
/// información pasa dentro de `install` y de `models`) y añade otras siete entradas
/// al sobre. Retirar una clave obliga a subir la versión; añadirla no.
pub const CLI_SCHEMA_VERSION: &str = "4";

/// Versión del **protocolo del daemon** (NDJSON y cabecera `x-schema-version`).
///
/// Es un contrato independiente del sobre de la CLI y sube por sus propios cambios
/// incompatibles. Pasó a `"4"` cuando `/transcribe` empezó a señalar sus errores con el
/// estado HTTP (400 y 500 en lugar de 200) y los `reason` de audio del daemon se
/// alinearon con el contrato (`usage_error` e `invalid_audio`).
pub const DAEMON_SCHEMA_VERSION: &str = "4";

/// Inserta `schema_version` en un `Value` sin imprimirlo.
///
/// La versión la recibe **el llamante** y no se lee de una constante única: el sobre de
/// la CLI y el del protocolo del daemon son dos contratos con dos versiones, y quien
/// llama es quien sabe cuál de los dos está estampando.
pub fn with_schema_version(val: Value, version: &str) -> Value {
    let mut map = match val {
        Value::Object(m) => m,
        _ => serde_json::Map::new(),
    };
    map.insert(
        "schema_version".to_string(),
        Value::String(version.to_string()),
    );
    Value::Object(map)
}

/// Imprime el sobre `--json` de la CLI por stdout, con la versión de la CLI.
pub fn emit_raw_json(val: Value) {
    let val = with_schema_version(val, CLI_SCHEMA_VERSION);
    if let Ok(json_str) = serde_json::to_string_pretty(&val) {
        println!("{}", json_str);
        let _ = std::io::stdout().flush();
    }
}
