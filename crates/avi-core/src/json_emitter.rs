use serde::Serialize;
use serde_json::Value;
use std::io::Write;

pub const SCHEMA_VERSION: &str = "3";

#[derive(Serialize)]
pub struct JsonEnvelope<T: Serialize> {
    pub schema_version: &'static str,
    #[serde(flatten)]
    pub data: T,
}

pub fn emit_json<T: Serialize>(payload: T) {
    let envelope = JsonEnvelope {
        schema_version: SCHEMA_VERSION,
        data: payload,
    };
    if let Ok(json_str) = serde_json::to_string_pretty(&envelope) {
        println!("{}", json_str);
        let _ = std::io::stdout().flush();
    }
}

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
