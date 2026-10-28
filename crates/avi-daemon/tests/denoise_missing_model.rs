// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

#![cfg(feature = "native-denoise")]

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::Engine;
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

use avi_daemon::{build_router_with_state, DaemonState};
use avi_store::{SpeechStore, VoiceStore};

#[tokio::test]
async fn denoise_true_without_model_returns_model_missing() {
    let cache = tempfile::tempdir().expect("sandbox de cache");
    let cache_path = cache.path().to_path_buf();
    unsafe { std::env::set_var("AVI_CACHE_DIR", &cache_path) };
    let scratch = avi_shared::paths::scratch_root().expect("raíz scratch del sandbox");
    std::fs::create_dir_all(cache_path.join(avi_shared::paths::MODELS_SUBDIR))
        .expect("crear modelos vacíos");
    std::fs::create_dir_all(&scratch).expect("crear scratch vacío");

    let mut state = DaemonState::for_contract_tests(
        VoiceStore::at(cache_path.join("data/voices")),
        SpeechStore::at(cache_path.join("data/speech")),
    )
    .expect("estado contractual del daemon");
    state.tts_engine.base_model_dir = Some(cache_path.join("base-model"));
    let state = Arc::new(state);

    let request = Request::builder()
        .method("POST")
        .uri("/voices/clone")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({
                "name": "missing_model",
                "force": true,
                "audio_b64": base64::engine::general_purpose::STANDARD.encode(minimal_wav()),
                "denoise": true,
            })
            .to_string(),
        ))
        .expect("construir petición");
    let response = build_router_with_state(state.clone())
        .oneshot(request)
        .await
        .expect("respuesta del router");
    assert_eq!(response.status(), StatusCode::OK);

    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("consumir stream completo")
        .to_bytes();
    let events: Vec<Value> = std::str::from_utf8(&bytes)
        .expect("NDJSON UTF-8")
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("evento JSON"))
        .collect();
    assert_eq!(events.last().unwrap()["event"], "error");
    assert_eq!(events.last().unwrap()["reason"], "model_missing");
    assert_eq!(
        events
            .iter()
            .filter(|event| event["event"] == "started")
            .count(),
        1
    );
    assert!(!cache_path.join("data/voices/missing_model.qvoice").exists());
    assert!(
        std::fs::read_dir(avi_process::scratch::dir().expect("scratch del proceso"))
            .expect("leer temporales de la petición")
            .next()
            .is_none()
    );
}

fn minimal_wav() -> Vec<u8> {
    let samples = vec![0u8; 32_000];
    let mut wav = Vec::with_capacity(44 + samples.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + samples.len() as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&16_000u32.to_le_bytes());
    wav.extend_from_slice(&32_000u32.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(samples.len() as u32).to_le_bytes());
    wav.extend_from_slice(&samples);
    wav
}
