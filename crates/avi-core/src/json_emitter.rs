// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

use serde_json::{Map, Value};
use std::io::Write;

/// Versión del envelope `--json` de la **CLI**; el valor vigente es `"5"`.
///
/// Retirar o renombrar una clave del envelope, o cambiar su significado, obliga a
/// subir la versión; añadir una clave nueva no.
pub const CLI_SCHEMA_VERSION: &str = "5";

/// Versión del **protocolo del daemon** (NDJSON y cabecera `x-schema-version`); el
/// valor vigente es `"5"`.
///
/// Es un contrato independiente del envelope de la CLI y sube por sus propios
/// cambios incompatibles, con la misma regla: retirar o renombrar una clave, o
/// cambiar su significado, obliga a subirla; añadir una clave nueva no.
pub const DAEMON_SCHEMA_VERSION: &str = "5";

/// Inserta `schema_version` en un objeto JSON sin imprimirlo.
///
/// La versión la recibe **el llamante** y no se lee de una constante única: el envelope de
/// la CLI y el del protocolo del daemon son dos contratos con dos versiones, y quien
/// llama es quien sabe cuál de los dos está estampando.
pub fn with_schema_version(mut map: Map<String, Value>, version: &str) -> Value {
    map.insert(
        "schema_version".to_string(),
        Value::String(version.to_string()),
    );
    Value::Object(map)
}

/// Extrae el objeto de un envelope. Un envelope que no es un objeto JSON es un
/// error de programación del emisor, no un dato que recuperar.
pub fn envelope_object(val: Value) -> Map<String, Value> {
    match val {
        Value::Object(map) => map,
        other => panic!("un envelope JSON debe ser un objeto, no: {other}"),
    }
}

/// Serializa el envelope con su `schema_version`. Un `Value` siempre se
/// serializa (sus claves son cadenas), de modo que la serialización no puede
/// fallar.
pub(crate) fn render_envelope(val: Value, version: &str) -> String {
    serde_json::to_string_pretty(&with_schema_version(envelope_object(val), version))
        .expect("un `Value` siempre se serializa: sus claves son cadenas")
}

/// Imprime el envelope `--json` de la CLI por stdout, con la versión de la CLI.
///
/// Un fallo al escribir o vaciar stdout no tiene recuperación posible y aborta
/// con un mensaje explícito, igual que `println!`.
pub fn emit_raw_json(val: Value) {
    let rendered = render_envelope(val, CLI_SCHEMA_VERSION);
    let mut out = std::io::stdout().lock();
    writeln!(out, "{rendered}")
        .and_then(|()| out.flush())
        .expect("no se pudo escribir el envelope JSON en stdout");
}
