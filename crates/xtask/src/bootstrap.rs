//! `xtask bootstrap`: prepara o pone al día el entorno de desarrollo (§10.3).
//!
//! Convergente: la misma orden prepara el entorno la primera vez y lo pone al
//! día tras un `git pull`; si nada cambió, no hace trabajo (criterio 24). Los
//! 6 pasos se ejecutan en orden y cada uno es no-op cuando su estado ya
//! converge. `--system` es la única vía que ejecuta gestores con `sudo`/UAC,
//! y solo por petición explícita (flag más confirmación). La versión de ONNX
//! sale de los pines y su aseguramiento reutiliza `ensure_ort_bundle` de
//! `package.rs` (decisión (d)): `bootstrap` no implementa su propia descarga.
//! `--models` provisiona con `setup` del build local. Sin `rustup` (caso WSL),
//! el paso 2 indica la instalación antes de fallar. Sin red en los tests:
//! las decisiones son funciones puras sobre fixtures.

use anyhow::{bail, Result};

/// Componentes de Rust que aseguran las puertas (`fmt --check`, clippy). La
/// versión ya la aplica `rust-toolchain.toml`.
const REQUIRED_COMPONENTS: &[&str] = &["clippy", "rustfmt"];

/// Una acción del sistema: ejecutable por proceso o solo manual (prosa que
/// ningún gestor puede correr, como instalar rustup a mano).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SystemAction {
    Run { program: String, args: Vec<String> },
    Manual(String),
}

/// Planifica los comandos del sistema a partir de las pistas de `doctor`:
/// las que empiezan por un ejecutable conocido se ejecutan, la prosa se
/// informa. Puro: sin `--system` nada se ejecuta (lo fija el llamador).
pub(crate) fn plan_system_commands(report: &super::doctor::Report) -> Vec<(String, SystemAction)> {
    let mut out = Vec::new();
    for row in &report.rows {
        if row.status == super::doctor::Status::Ok {
            continue;
        }
        let Some(hint) = &row.hint else { continue };
        let action = executable_hint(hint).map_or_else(
            || SystemAction::Manual(hint.clone()),
            |(program, args)| SystemAction::Run { program, args },
        );
        out.push((row.id.to_string(), action));
    }
    out
}

/// `Some((programa, args))` si la pista es un comando ejecutable (`sudo …`,
/// `winget …`, `brew …`, `xcode-select …`, `pacman …`, `cargo …`); `None` si
/// es prosa manual o una delegación a este mismo comando (`cargo xtask …`).
fn executable_hint(hint: &str) -> Option<(String, Vec<String>)> {
    let mut parts = hint.split_whitespace();
    let first = parts.next()?;
    let executable = matches!(
        first,
        "sudo" | "winget" | "brew" | "xcode-select" | "pacman" | "cargo"
    );
    if !executable || hint.starts_with("cargo xtask") {
        return None;
    }
    Some((first.to_string(), parts.map(str::to_string).collect()))
}

/// Puerta de `--system`: exige petición explícita (flag) y confirmación
/// (`--yes` o TTY). Pura sobre flags ya leídos, testeable sin procesos.
pub(crate) fn check_system_gate(system: bool, yes: bool, is_tty: bool) -> Result<bool> {
    if !system {
        return Ok(false);
    }
    if !yes && !is_tty {
        bail!("--system sin TTY: confirma con --yes (solo entonces se ejecuta con sudo/UAC)");
    }
    Ok(true)
}

/// Componentes ausentes en la salida de `rustup component list --installed`.
/// Pura: la lista instalada entra como texto (fixture en tests).
pub(crate) fn missing_components(installed: &str, wanted: &[&str]) -> Vec<String> {
    wanted
        .iter()
        .filter(|w| {
            !installed
                .lines()
                .any(|l| l.trim_start().starts_with(&format!("{w}-")))
        })
        .map(|w| (*w).to_string())
        .collect()
}

/// Punto de entrada de `xtask bootstrap`.
pub fn run(system: bool, models: bool, yes: bool) -> Result<()> {
    use std::io::{IsTerminal, Write};
    let root = std::env::current_dir()?;
    let pins = super::doctor::load_pins(&root)?;
    let mut worked = false;

    // Paso 1. `doctor`: imprime los comandos que falten; con `--system` y
    // confirmación explícita, los ejecuta (única vía con sudo/UAC).
    let report = super::doctor::check(&root, &pins);
    let plan = plan_system_commands(&report);
    if plan.is_empty() {
        println!("Paso 1/6 (doctor): requisitos del sistema al día.");
    } else {
        println!("Paso 1/6 (doctor): faltan requisitos del sistema:");
        for (id, action) in &plan {
            match action {
                SystemAction::Run { program, args } => {
                    println!("  [{id}] {program} {}", args.join(" "));
                }
                SystemAction::Manual(text) => println!("  [{id}] {text}"),
            }
        }
        if check_system_gate(system, yes, std::io::stdin().is_terminal())? {
            if !yes {
                print!("¿Ejecutar los comandos ejecutables con sudo/UAC? [s/N]: ");
                std::io::stdout().flush()?;
                let mut input = String::new();
                std::io::stdin().read_line(&mut input)?;
                let t = input.trim().to_lowercase();
                if !matches!(t.as_str(), "s" | "si" | "sí" | "y" | "yes") {
                    println!("Cancelado: el sistema queda sin tocar.");
                    return Ok(());
                }
            }
            for (id, action) in &plan {
                match action {
                    SystemAction::Run { program, args } => {
                        println!("  Ejecutando [{id}]: {program} {}", args.join(" "));
                        let status = std::process::Command::new(program)
                            .args(args)
                            .status()
                            .map_err(|e| anyhow::anyhow!("no se pudo ejecutar {program}: {e}"))?;
                        if !status.success() {
                            bail!(
                                "el comando del sistema [{id}] falló (exit {:?}): corrígelo a mano y repite `cargo xtask bootstrap`",
                                status.code()
                            );
                        }
                        worked = true;
                    }
                    SystemAction::Manual(text) => {
                        println!("  Manual [{id}]: {text}");
                    }
                }
            }
        } else if !system {
            println!("  (con --system se ejecutan los comandos ejecutables; el resto es manual)");
        }
    }

    // Paso 2. Componentes de Rust. Sin `rustup`, indicar la instalación
    // antes de fallar (caso WSL sin rustup).
    match rustup_installed_list()? {
        None => {
            println!("Paso 2/6 (Rust): sin `rustup` en el PATH.");
            bail!(
                "instala rustup primero (https://rustup.rs) y repite `cargo xtask bootstrap`: rust-toolchain.toml aplicará la versión {} sola",
                pins.rust
            );
        }
        Some(installed) => {
            let missing = missing_components(&installed, REQUIRED_COMPONENTS);
            if missing.is_empty() {
                println!("Paso 2/6 (Rust): componentes al día (clippy, rustfmt).");
            } else {
                println!("Paso 2/6 (Rust): añadiendo {} …", missing.join(", "));
                let status = std::process::Command::new("rustup")
                    .arg("component")
                    .arg("add")
                    .args(&missing)
                    .status()?;
                if !status.success() {
                    bail!("`rustup component add` falló (exit {:?})", status.code());
                }
                worked = true;
            }
        }
    }

    // Paso 3. ONNX Runtime en la versión fijada (reutiliza si está fresco).
    let triple = super::package::host_triple()?;
    super::package::ensure_ort_bundle(&root, triple, &pins.ort)?;
    println!(
        "Paso 3/6 (ONNX Runtime {}): asegurado en ort-bundle/.",
        pins.ort
    );

    // Paso 4. Motor TTS si falta o está desactualizado.
    if super::doctor::engine_is_fresh(&root) {
        println!("Paso 4/6 (motor TTS): al día, se omite la compilación.");
    } else {
        println!("Paso 4/6 (motor TTS): compilando (build-engine --self-test) …");
        let xtask = std::env::current_exe()?;
        let status = std::process::Command::new(&xtask)
            .args(["build-engine", "--self-test"])
            .status()?;
        if !status.success() {
            bail!("la compilación del motor falló (exit {:?})", status.code());
        }
        worked = true;
    }

    // Paso 5. Modelos solo con `--models`, vía `setup` del build local.
    if !models {
        println!("Paso 5/6 (modelos): omitido (pídelo con --models).");
    } else if super::doctor::models_are_provisioned() {
        println!("Paso 5/6 (modelos): ya provisionados, se omite `setup`.");
    } else {
        println!("Paso 5/6 (modelos): ejecutando `setup` del build local …");
        let status = std::process::Command::new("cargo")
            .args(["run", "--", "setup"])
            .current_dir(&root)
            .status()?;
        if !status.success() {
            bail!("`setup` falló (exit {:?})", status.code());
        }
        worked = true;
    }

    // Paso 6. Resumen del estado alcanzado.
    if worked {
        println!("Paso 6/6: entorno puesto al día.");
    } else {
        println!("Paso 6/6: sin trabajo, el entorno ya estaba convergente.");
    }
    Ok(())
}

/// Salida de `rustup component list --installed`; `None` si no hay `rustup`.
fn rustup_installed_list() -> Result<Option<String>> {
    let out = std::process::Command::new("rustup")
        .args(["component", "list", "--installed"])
        .output();
    let Ok(out) = out else {
        return Ok(None);
    };
    if !out.status.success() {
        return Ok(None);
    }
    Ok(Some(String::from_utf8_lossy(&out.stdout).to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doctor::{Report, Row, Status};

    /// Informe de juguete: una fila ejecutable, una manual y una correcta.
    fn toy_report() -> Report {
        Report {
            rows: vec![
                Row {
                    id: "cmake",
                    label: "CMake",
                    mandatory: true,
                    status: Status::Missing,
                    found: None,
                    want: Some("≥ 3.20".to_string()),
                    hint: Some("sudo apt install cmake".to_string()),
                },
                Row {
                    id: "rust",
                    label: "Rust",
                    mandatory: true,
                    status: Status::Missing,
                    found: None,
                    want: Some("1.96.0".to_string()),
                    hint: Some(
                        "instala rustup (https://rustup.rs): rust-toolchain.toml aplica la versión sola"
                            .to_string(),
                    ),
                },
                Row {
                    id: "onnx",
                    label: "ONNX",
                    mandatory: true,
                    status: Status::Missing,
                    found: None,
                    want: Some("1.28.0".to_string()),
                    hint: Some("cargo xtask bootstrap".to_string()),
                },
                Row {
                    id: "sccache",
                    label: "sccache",
                    mandatory: false,
                    status: Status::Ok,
                    found: Some("0.8.2".to_string()),
                    want: Some("cualquiera".to_string()),
                    hint: None,
                },
            ],
            engine_drift: None,
            onnx_drift: None,
            models_drift: Vec::new(),
        }
    }

    /// El plan distingue ejecutables, manuales y delegaciones propias, y
    /// omite las filas correctas: sin `--system` nada se ejecuta.
    #[test]
    fn plan_classifies_without_executing() {
        let plan = plan_system_commands(&toy_report());
        assert_eq!(plan.len(), 3);
        assert_eq!(
            plan[0],
            (
                "cmake".to_string(),
                SystemAction::Run {
                    program: "sudo".to_string(),
                    args: ["apt", "install", "cmake"]
                        .iter()
                        .map(|s| (*s).to_string())
                        .collect(),
                }
            )
        );
        assert!(matches!(plan[1].1, SystemAction::Manual(_)));
        // `cargo xtask …` es delegación propia: manual, nunca ejecutable.
        assert!(matches!(plan[2].1, SystemAction::Manual(_)));
        // La fila correcta (sccache) no entra en el plan.
        assert!(!plan.iter().any(|(id, _)| id == "sccache"));
    }

    /// `--system` exige petición explícita y confirmación (o TTY).
    #[test]
    fn system_gate_requires_explicit_request() {
        // Sin --system: no se ejecuta nada, sin error.
        assert!(!check_system_gate(false, false, false).unwrap());
        assert!(!check_system_gate(false, true, true).unwrap());
        // Con --system y --yes: adelante.
        assert!(check_system_gate(true, true, false).unwrap());
        // Con --system y TTY: adelante (confirmará en terminal).
        assert!(check_system_gate(true, false, true).unwrap());
        // Con --system, sin --yes y sin TTY: falla antes de tocar nada.
        assert!(check_system_gate(true, false, false).is_err());
    }

    /// Convergencia de componentes: la segunda ejecución no añade nada.
    #[test]
    fn missing_components_is_convergent() {
        let installed = "cargo-x86_64-pc-windows-msvc\n\
             clippy-x86_64-pc-windows-msvc\n\
             rustfmt-x86_64-pc-windows-msvc\n";
        assert!(missing_components(installed, REQUIRED_COMPONENTS).is_empty());
        let installed = "cargo-x86_64-pc-windows-msvc\nclippy-x86_64-pc-windows-msvc\n";
        assert_eq!(
            missing_components(installed, REQUIRED_COMPONENTS),
            ["rustfmt".to_string()]
        );
    }

    /// Pistas no ejecutables (prosa, delegaciones) quedan como manuales.
    #[test]
    fn executable_hint_parsing() {
        assert!(executable_hint("instala rustup a mano").is_none());
        assert!(executable_hint("cargo xtask bootstrap").is_none());
        let (program, args) = executable_hint("winget install Kitware.CMake").unwrap();
        assert_eq!(program, "winget");
        assert_eq!(args, ["install", "Kitware.CMake"]);
    }
}
