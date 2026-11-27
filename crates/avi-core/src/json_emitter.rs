// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

use avi_term::report::{self, ContextValue, Snapshot, StageOutcome};
use serde_json::{json, Map, Value};
use std::io::Write;
use std::sync::atomic::Ordering;

/// Versión del envelope `--json` de la **CLI**; el valor vigente es `"7"`.
///
/// Retirar o renombrar una clave del envelope, o cambiar su significado, obliga a
/// subir la versión; añadir una clave nueva no.
///
/// La versión 7 es un cambio incompatible: todo envelope lleva ahora las claves
/// comunes `context`, `total_ms`, `stages` y `warnings` (desde el registro de
/// ejecución), `speech synthesize` emite `status: "synthesized"` en vez de
/// `success` y `speech transcribe` emite `status: "transcribed"`, `speech denoise
/// --json` emite un único envelope en vez de líneas JSON por evento, y el
/// envelope de error añade el contexto y las etapas a `error` y `reason`.
pub const CLI_SCHEMA_VERSION: &str = "7";

/// Versión del **protocolo del daemon** (NDJSON y cabecera `x-schema-version`); el
/// valor vigente es `"8"`.
///
/// Es un contrato independiente del envelope de la CLI y sube por sus propios
/// cambios incompatibles, con la misma regla: retirar o renombrar una clave, o
/// cambiar su significado, obliga a subirla; añadir una clave nueva no.
///
/// La versión 8 es un cambio incompatible: `/transcribe` responde con un stream
/// NDJSON (`started`, `progress`, `result` o `error`) en vez de un JSON único.
pub const DAEMON_SCHEMA_VERSION: &str = "8";

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

/// Milisegundos de una duración, saturando en `u64::MAX`.
fn millis(elapsed: std::time::Duration) -> u64 {
    u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
}

/// Objeto `{stage, phase, start_ms, end_ms}` de una etapa del registro, con las
/// marcas de su desenlace: `failed`, `failed` + `interrupted`, `skipped` o
/// `transient`.
fn stage_object(snapshot: &Snapshot, record: &report::StageRecord) -> Value {
    let end = record.end.unwrap_or(snapshot.taken_at);
    let mut object = Map::new();
    object.insert("stage".into(), json!(record.label));
    object.insert("phase".into(), json!(record.phase));
    object.insert(
        "start_ms".into(),
        json!(millis(snapshot.relative(record.start))),
    );
    object.insert("end_ms".into(), json!(millis(snapshot.relative(end))));
    match &record.outcome {
        StageOutcome::Ok => {}
        StageOutcome::Failed => {
            object.insert("failed".into(), json!(true));
        }
        StageOutcome::Interrupted => {
            object.insert("failed".into(), json!(true));
            object.insert("interrupted".into(), json!(true));
        }
        StageOutcome::Skipped(_) => {
            object.insert("skipped".into(), json!(true));
        }
        StageOutcome::Transient => {
            object.insert("transient".into(), json!(true));
        }
    }
    Value::Object(object)
}

/// Añade al envelope las claves comunes que salen del registro, sin pisar las
/// propias del comando:
/// - `context`: los pares tipados de la cabecera (`{}` si hay cabecera sin
///   pares; se omite si no hay cabecera);
/// - `total_ms` y `stages`: solo si el registro tiene al menos una etapa;
/// - `warnings`: siempre que haya etapas (aunque sea `[]`) y, sin etapas, solo si
///   hay al menos un aviso registrado.
fn merge_report(val: Value, snapshot: &Snapshot) -> Value {
    let mut map = envelope_object(val);
    let mut add = |key: &str, value: Value| {
        map.entry(key.to_string()).or_insert(value);
    };
    if let Some(context) = &snapshot.context {
        let pairs: Map<String, Value> = context
            .pairs
            .iter()
            .map(|(key, value)| {
                let value = match value {
                    ContextValue::Text(text) => json!(text),
                    ContextValue::Int(number) => json!(number),
                    ContextValue::Bool(flag) => json!(flag),
                };
                (key.clone(), value)
            })
            .collect();
        add("context", Value::Object(pairs));
    }
    if !snapshot.stages.is_empty() {
        add("total_ms", json!(millis(snapshot.total())));
        add(
            "stages",
            Value::Array(
                snapshot
                    .stages
                    .iter()
                    .map(|record| stage_object(snapshot, record))
                    .collect(),
            ),
        );
    }
    if !snapshot.stages.is_empty() || !snapshot.warnings.is_empty() {
        add(
            "warnings",
            Value::Array(
                snapshot
                    .warnings
                    .iter()
                    .map(|warning| json!(warning.text))
                    .collect(),
            ),
        );
    }
    Value::Object(map)
}

/// Reserva la salida del envelope de este proceso: devuelve `true` solo la
/// primera vez. Así sale exactamente un envelope por proceso, aunque un Ctrl+C
/// se cruce con el final normal del comando.
fn claim_envelope() -> bool {
    report::ENVELOPE_EMITTED
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
}

/// Imprime el envelope `--json` de la CLI por stdout, con la versión de la CLI
/// y las claves comunes del registro de ejecución. Equivale a
/// [`emit_raw_json_with`] con una copia del registro tomada ahora.
///
/// Un fallo al escribir o vaciar stdout no tiene recuperación posible y aborta
/// con un mensaje explícito, igual que `println!`.
pub fn emit_raw_json(val: Value) {
    emit_raw_json_with(val, &report::snapshot());
}

/// Como [`emit_raw_json`], con una copia del registro dada: no vuelve a tomar
/// el cerrojo del registro, así que puede llamarlo quien ya lo tiene (el
/// manejador de Ctrl+C). Si ya salió un envelope en este proceso, no escribe
/// nada.
pub fn emit_raw_json_with(val: Value, snapshot: &Snapshot) {
    if !claim_envelope() {
        return;
    }
    let rendered = render_envelope(merge_report(val, snapshot), CLI_SCHEMA_VERSION);
    let mut out = std::io::stdout().lock();
    writeln!(out, "{rendered}")
        .and_then(|()| out.flush())
        .expect("no se pudo escribir el envelope JSON en stdout");
}

#[cfg(test)]
mod tests {
    use super::*;
    use avi_term::line;
    use avi_term::progress::PhaseProgress;
    use avi_term::stage;

    /// Sin etapas solo puede salir el `context`, y solo si hay cabecera.
    #[test]
    fn without_stages_only_the_context_is_merged() {
        let _g = report::test_guard();
        let bare = merge_report(json!({ "status": "ok" }), &report::snapshot());
        assert_eq!(bare, json!({ "status": "ok" }));

        line::context("Cabecera sin pares");
        let merged = merge_report(json!({ "status": "ok" }), &report::snapshot());
        assert_eq!(merged, json!({ "status": "ok", "context": {} }));
    }

    /// Sin etapas, un aviso registrado sí llega a `warnings`; `stages` y
    /// `total_ms` siguen ausentes.
    #[test]
    fn warnings_are_merged_even_without_stages() {
        let _g = report::test_guard();
        line::warn("pidfile descartado");
        let merged = merge_report(json!({ "status": "ok" }), &report::snapshot());
        assert_eq!(
            merged,
            json!({ "status": "ok", "warnings": ["pidfile descartado."] })
        );
    }

    /// Un `ContextValue::Bool` se serializa como booleano JSON.
    #[test]
    fn boolean_context_values_serialize_as_json_booleans() {
        let _g = report::test_guard();
        line::context_with(
            "Daemon",
            &[
                ("auto_restart", ContextValue::Bool(true)),
                ("warm_voice", ContextValue::Bool(false)),
            ],
        );
        let merged = merge_report(json!({}), &report::snapshot());
        assert_eq!(
            merged["context"],
            json!({ "auto_restart": true, "warm_voice": false })
        );
    }

    /// Con etapas se funden `context` (tipado), `total_ms`, `stages` y
    /// `warnings`, sin pisar las claves propias del comando.
    #[test]
    fn merging_adds_the_common_keys_without_overwriting_the_commands_own() {
        let _g = report::test_guard();
        line::context_with(
            "Doblaje",
            &[
                ("voice", ContextValue::Text("default".into())),
                ("max_secs", ContextValue::Int(40)),
            ],
        );
        let progress = PhaseProgress::new();
        progress.phase(&stage::TRANSCRIBING);
        line::warn("audio muy bajo");
        progress.end_stage();
        let merged = merge_report(
            json!({ "status": "ok", "warnings": ["propio"], "context": "propio" }),
            &report::snapshot(),
        );
        assert_eq!(merged["status"], "ok");
        assert_eq!(merged["warnings"], json!(["propio"]));
        assert_eq!(merged["context"], "propio");
        assert!(merged["total_ms"].is_u64());
        let stages = merged["stages"].as_array().unwrap();
        assert_eq!(stages.len(), 1);
        assert_eq!(stages[0]["stage"], "transcribing");
        assert_eq!(stages[0]["phase"], Value::Null);
        assert!(stages[0]["start_ms"].is_u64() && stages[0]["end_ms"].is_u64());

        let own_free = merge_report(json!({ "status": "ok" }), &report::snapshot());
        assert_eq!(
            own_free["context"],
            json!({ "voice": "default", "max_secs": 40 })
        );
        assert_eq!(own_free["warnings"], json!(["audio muy bajo."]));
        drop(progress);
    }

    /// Cada desenlace deja su forma en `stages`.
    #[test]
    fn each_outcome_has_its_stage_shape() {
        let _g = report::test_guard();
        let progress = PhaseProgress::new();
        progress.phase(&stage::VALIDATING);
        progress.phase(&stage::READING);
        progress.end_stage();
        progress.skip(&stage::SAVING, "ya existía");
        progress.phase(&stage::TRANSCRIBING);
        progress.fail_current();
        progress.phase(&stage::SYNTHESIZING);
        report::lock().interrupt_open_stages();
        let merged = merge_report(json!({}), &report::snapshot());
        let stages = merged["stages"].as_array().unwrap();
        assert_eq!(stages[0]["transient"], true);
        assert!(stages[1].get("failed").is_none() && stages[1].get("transient").is_none());
        assert_eq!(stages[2]["skipped"], true);
        assert_eq!(stages[2]["start_ms"], stages[2]["end_ms"]);
        assert_eq!(stages[3]["failed"], true);
        assert!(stages[3].get("interrupted").is_none());
        assert_eq!(stages[4]["failed"], true);
        assert_eq!(stages[4]["interrupted"], true);
        drop(progress);
    }

    /// Solo la primera reserva gana: sale un único envelope por proceso.
    #[test]
    fn only_the_first_claim_emits_an_envelope() {
        let _g = report::test_guard();
        assert!(claim_envelope());
        assert!(!claim_envelope());
        assert!(!claim_envelope());
    }
}
