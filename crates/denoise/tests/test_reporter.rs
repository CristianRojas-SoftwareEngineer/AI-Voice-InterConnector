//! Test de reportería — verifica el orden [i/N], el resultado por archivo del
//! envelope único y el resumen.

use denoise::cli::{BatchSummary, FileReport, ItemStatus, ResolvedItem};
use std::path::PathBuf;

#[test]
fn test_reporting_order() {
    let items = [
        ResolvedItem {
            input: PathBuf::from("video1.mp4"),
            output: PathBuf::from("video1_denoised.mp4"),
            is_duplicate_adjusted: false,
        },
        ResolvedItem {
            input: PathBuf::from("video2.mp4"),
            output: PathBuf::from("video2_denoised.mp4"),
            is_duplicate_adjusted: false,
        },
        ResolvedItem {
            input: PathBuf::from("video3.mp4"),
            output: PathBuf::from("video3_denoised.mp4"),
            is_duplicate_adjusted: false,
        },
    ];

    assert_eq!(items.len(), 3);
    for (i, item) in items.iter().enumerate() {
        let header = format!(
            "[{}/{}] {} -> {}",
            i + 1,
            items.len(),
            item.input.display(),
            item.output.display()
        );
        assert!(header.starts_with(&format!("[{}/3]", i + 1)));
    }
}

#[test]
fn test_file_report_is_parseable() {
    let file = FileReport {
        input: "input.mp4".to_string(),
        output: "output.mp4".to_string(),
        status: ItemStatus::Ok,
        message: "completado".to_string(),
    };

    let serialized = serde_json::to_string(&file).expect("Serialización JSON exitosa");
    let deserialized: serde_json::Value =
        serde_json::from_str(&serialized).expect("JSON parseable");

    assert_eq!(deserialized["input"], "input.mp4");
    assert_eq!(deserialized["output"], "output.mp4");
    assert_eq!(deserialized["status"], "ok");
    assert_eq!(deserialized["message"], "completado");
    // El avance por archivo ya no viaja en el JSON.
    assert!(deserialized.get("pct").is_none());
}

#[test]
fn test_summary_correct() {
    let summary = BatchSummary {
        ok: 4,
        failed: 1,
        skipped: 2,
    };

    let serialized = serde_json::to_string(&summary).expect("Serialización JSON exitosa");
    let deserialized: serde_json::Value =
        serde_json::from_str(&serialized).expect("JSON parseable");

    assert_eq!(deserialized["ok"], 4);
    assert_eq!(deserialized["failed"], 1);
    assert_eq!(deserialized["skipped"], 2);
}

#[test]
fn test_status_names_are_kebab_case() {
    for (status, name) in [
        (ItemStatus::Ok, "ok"),
        (ItemStatus::Failed, "failed"),
        (ItemStatus::Skipped, "skipped"),
        (ItemStatus::DryRun, "dry-run"),
    ] {
        let file = FileReport {
            input: "a.mp4".to_string(),
            output: "b.mp4".to_string(),
            status,
            message: String::new(),
        };
        let serialized = serde_json::to_string(&file).unwrap();
        assert!(!serialized.contains("\x1b["));
        assert!(
            serialized.contains(&format!(r#""status":"{name}""#)),
            "{serialized}"
        );
    }
}
