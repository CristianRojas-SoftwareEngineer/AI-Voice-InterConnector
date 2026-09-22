use serde_json::Value;
use std::io::Write;

pub const SCHEMA_VERSION: &str = "3";

/// Inserta `schema_version` en un `Value` sin imprimirlo (uso compartido CLI/daemon).
pub fn with_schema_version(val: Value) -> Value {
    let mut map = match val {
        Value::Object(m) => m,
        _ => serde_json::Map::new(),
    };
    map.insert(
        "schema_version".to_string(),
        Value::String(SCHEMA_VERSION.to_string()),
    );
    Value::Object(map)
}

pub fn emit_raw_json(val: Value) {
    let val = with_schema_version(val);
    if let Ok(json_str) = serde_json::to_string_pretty(&val) {
        println!("{}", json_str);
        let _ = std::io::stdout().flush();
    }
}
