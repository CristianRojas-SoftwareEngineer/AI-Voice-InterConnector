// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! `xtask doctor`: diagnóstico solo-lectura del entorno de desarrollo.
//!
//! La tabla de requisitos se declara una sola vez aquí: los pines (Rust, ONNX
//! Runtime, FFmpeg, el motor TTS…) se leen de `packaging/pins.json` vía `avi-shared`, y de
//! ahí los consumen `doctor`, `bootstrap` y la CI. Por cada requisito informa
//! si está correcto, falta o tiene una versión distinta de la fijada; cuando
//! algo falta, da el comando exacto de instalación para el gestor detectado
//! (apt, dnf, pacman o zypper; `xcode-select` o brew; winget). Informa además
//! de la deriva del entorno: ONNX Runtime distinto del fijado y modelos sin
//! provisionar. Termina con éxito solo si están todos los obligatorios.
//! Admite `--json`. No modifica nada.

use anyhow::Result;
use avi_shared::pins::Pins;
use std::path::{Path, PathBuf};

/// Filas de la tabla de requisitos, en su orden. `sccache` y
/// `target-hygiene` son las únicas opcionales.
pub(crate) const ROW_IDS: &[&str] = &[
    "rust",
    "c-compiler",
    "cmake",
    "onnx",
    "ffmpeg",
    "qwen-tts-engine",
    "sccache",
    "target-hygiene",
];

/// Estado de un requisito: correcto, ausente, con versión distinta o con una
/// sonda que no se pudo ejecutar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Status {
    Ok,
    Missing,
    Mismatch,
    Error,
}

/// Una fila ya sondada: estado, versión encontrada y esperada, y comando
/// exacto de instalación cuando falta (vacío si no aplica).
pub(crate) struct Row {
    pub id: &'static str,
    pub label: &'static str,
    pub mandatory: bool,
    pub status: Status,
    pub found: Option<String>,
    pub want: Option<String>,
    pub hint: Option<String>,
}

/// Informe completo: filas de la tabla más deriva del entorno.
pub(crate) struct Report {
    pub rows: Vec<Row>,
    pub onnx_drift: Option<String>,
    pub models_drift: Vec<String>,
}

impl Report {
    /// `true` si todos los requisitos obligatorios están correctos.
    pub(crate) fn ok(&self) -> bool {
        self.rows
            .iter()
            .filter(|r| r.mandatory)
            .all(|r| r.status == Status::Ok)
    }
}

/// ¿Existe `name` en el PATH? Recorrido manual (sin lanzar procesos): en
/// Windows también valen `.exe`, `.cmd` y `.bat`.
pub(crate) fn command_exists(name: &str) -> bool {
    find_on_path(name).is_some()
}

/// Localiza `name` en el PATH (lo reutiliza `clean` para el canal instalado).
pub(crate) fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let mut candidates = vec![name.to_string()];
    if cfg!(windows) {
        for ext in ["exe", "cmd", "bat"] {
            candidates.push(format!("{name}.{ext}"));
        }
    }
    for dir in std::env::split_paths(&path) {
        for candidate in &candidates {
            let full = dir.join(candidate);
            if full.is_file() {
                return Some(full);
            }
        }
    }
    None
}

/// Comando de sonda sin ventana de consola propia en Windows: un invocador sin
/// consola vería abrirse y cerrarse una ventana por cada sonda.
pub(crate) fn probe_command(program: impl AsRef<std::ffi::OsStr>) -> std::process::Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let mut cmd = std::process::Command::new(program);
        cmd.creation_flags(avi_process::CREATE_NO_WINDOW);
        cmd
    }
    #[cfg(not(windows))]
    {
        std::process::Command::new(program)
    }
}

/// Salida podada de `programa args…` para una sonda de la que solo importa si
/// responde: `None` si no se pudo lanzar, falló o no imprimió nada. Es válida
/// solo cuando la ausencia del programa y su fallo son la misma respuesta
/// (`xcode-select -p`, `gcc -print-file-name`); las sondas de
/// versión usan `probe_version`, que distingue «no instalado» de «no se pudo
/// ejecutar».
fn probe_output(program: &str, args: &[&str]) -> Option<String> {
    let out = probe_command(program).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let trimmed = text.trim().to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

/// Sonda de versión de `programa args…`. `Ok(None)` si el programa no está
/// instalado (`NotFound` al lanzarlo) o salió con error o sin salida;
/// `Err(motivo)` si no se pudo ejecutar por otra causa (permisos, ejecutable
/// dañado…), que el chequeo informa como tal y no como «falta».
fn probe_version(program: &str, args: &[&str]) -> Result<Option<String>, String> {
    let out = match probe_command(program).args(args).output() {
        Ok(out) => out,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("no se pudo ejecutar `{program}`: {e}")),
    };
    if !out.status.success() {
        return Ok(None);
    }
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let trimmed = text.trim();
    Ok((!trimmed.is_empty()).then(|| trimmed.to_string()))
}

/// Como `probe_version`, reducida al token de versión de la salida.
fn probe_version_token(program: &str, args: &[&str]) -> Result<Option<String>, String> {
    Ok(probe_version(program, args)?.and_then(|o| version_token(&o)))
}

/// Fila de un requisito cuya sonda no se pudo ejecutar: ni correcto ni ausente.
fn probe_error_row(
    id: &'static str,
    label: &'static str,
    mandatory: bool,
    want: Option<String>,
    reason: String,
) -> Row {
    Row {
        id,
        label,
        mandatory,
        status: Status::Error,
        found: Some(reason),
        want,
        hint: None,
    }
}

/// Primer token con pinta de versión (`1.96.0`, `3.28`, `16.2.0-1`) en un
/// texto libre como `rustc 1.96.0 (…).`
pub(crate) fn version_token(text: &str) -> Option<String> {
    for token in text.split(|c: char| c.is_whitespace() || c == ',') {
        let token = token.trim_matches(|c: char| !c.is_alphanumeric() && c != '.' && c != '-');
        let token = token.trim_end_matches(|c: char| !c.is_alphanumeric());
        if token.contains('.')
            && token.chars().next().is_some_and(|c| c.is_ascii_digit())
            && token.chars().any(|c| c.is_ascii_digit())
        {
            return Some(token.to_string());
        }
    }
    None
}

/// Gestor de paquetes de la distribución Linux: el primero disponible en el
/// PATH entre apt-get, dnf, pacman y zypper.
pub(crate) fn detect_linux_manager() -> Option<&'static str> {
    for (bin, name) in [
        ("apt-get", "apt"),
        ("dnf", "dnf"),
        ("pacman", "pacman"),
        ("zypper", "zypper"),
    ] {
        if command_exists(bin) {
            return Some(name);
        }
    }
    None
}

/// Comando exacto de instalación de `packages` para el gestor dado. Solo
/// construye el texto (puro, testeable); ejecutarlo es tarea de `bootstrap
/// --system`.
pub(crate) fn linux_install_command(manager: &str, packages: &str) -> String {
    match manager {
        "apt" => format!("sudo apt install {packages}"),
        "dnf" => format!("sudo dnf install {packages}"),
        "pacman" => format!("sudo pacman -S {packages}"),
        "zypper" => format!("sudo zypper install {packages}"),
        _ => format!("instala con tu gestor: {packages}"),
    }
}

/// Raíz de MSYS2 (`MSYS2_ROOT`, por defecto `C:\msys64`).
pub(crate) fn msys2_root() -> PathBuf {
    std::env::var("MSYS2_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| Path::new(r"C:\msys64").to_path_buf())
}

/// `true` si la versión instalada cubre el pin: igualdad tras recortar el
/// sufijo de empaquetado (`-1`, `-5`) en ambos lados, porque los pines de
/// MSYS2 lo traen (`0.3.34-1`) y `pacman -Q` también (`16.2.0-1`).
pub(crate) fn pin_covers(installed: &str, pinned: &str) -> bool {
    fn base(v: &str) -> &str {
        let v = v.trim();
        match v.rsplit_once('-') {
            Some((head, tail))
                if head.contains('.') && tail.chars().all(|c| c.is_ascii_digit()) =>
            {
                head
            }
            _ => v,
        }
    }
    base(installed) == base(pinned)
}

/// Compara `major.minor` de `found` contra el mínimo exigido.
pub(crate) fn version_at_least(found: &str, min_major: u64, min_minor: u64) -> bool {
    let mut parts = found.split(['.', '-']);
    let mut next_number = || parts.next()?.parse::<u64>().ok();
    match (next_number(), next_number()) {
        (Some(major), Some(minor)) => (major, minor) >= (min_major, min_minor),
        _ => false,
    }
}

/// Sonda Rust: `rustc --version` contra el pin.
fn check_rust(pins: &Pins) -> Row {
    let label = "Rust (versión fijada)";
    let want = Some(pins.rust.clone());
    let out = match probe_version("rustc", &["--version"]) {
        Err(reason) => return probe_error_row("rust", label, true, want, reason),
        Ok(out) => out,
    };
    let Some(out) = out else {
        return Row {
            id: "rust",
            label,
            mandatory: true,
            status: Status::Missing,
            found: None,
            want,
            hint: Some(
                "instala rustup (https://rustup.rs): rust-toolchain.toml aplica la versión sola"
                    .to_string(),
            ),
        };
    };
    let found = version_token(&out);
    let status = match &found {
        Some(v) if v == &pins.rust => Status::Ok,
        _ => Status::Mismatch,
    };
    Row {
        id: "rust",
        label,
        mandatory: true,
        status,
        found,
        want,
        hint: None,
    }
}

/// Sonda del compilador C/C++ del workspace (CTranslate2) según el SO.
fn check_c_compiler() -> Row {
    let label = "Compilador C/C++ del workspace (CTranslate2)";
    if cfg!(windows) {
        if command_exists("cl") {
            let found = match probe_version_token("cl", &[]) {
                Ok(found) => found,
                Err(reason) => {
                    return probe_error_row(
                        "c-compiler",
                        label,
                        true,
                        Some("Visual Studio Build Tools (C++)".to_string()),
                        reason,
                    )
                }
            };
            return Row {
                id: "c-compiler",
                label,
                mandatory: true,
                status: Status::Ok,
                found,
                want: Some("Visual Studio Build Tools (C++)".to_string()),
                hint: None,
            };
        }
        // `vswhere` delata una instalación de Visual Studio aunque `cl` no
        // esté en el PATH de esta terminal.
        if Path::new(crate::package::VSWHERE_PATH).is_file() {
            return Row {
                id: "c-compiler",
                label,
                mandatory: true,
                status: Status::Ok,
                found: Some("instalado (vswhere)".to_string()),
                want: Some("Visual Studio Build Tools (C++)".to_string()),
                hint: None,
            };
        }
        return Row {
            id: "c-compiler",
            label,
            mandatory: true,
            status: Status::Missing,
            found: None,
            want: Some("Visual Studio Build Tools (C++)".to_string()),
            hint: Some(
                "winget install Microsoft.VisualStudio.2022.BuildTools (carga de trabajo C++)"
                    .to_string(),
            ),
        };
    }
    if cfg!(target_os = "macos") {
        if probe_output("xcode-select", &["-p"]).is_some() {
            return Row {
                id: "c-compiler",
                label,
                mandatory: true,
                status: Status::Ok,
                found: Some("Xcode Command Line Tools".to_string()),
                want: Some("Xcode Command Line Tools".to_string()),
                hint: None,
            };
        }
        return Row {
            id: "c-compiler",
            label,
            mandatory: true,
            status: Status::Missing,
            found: None,
            want: Some("Xcode Command Line Tools".to_string()),
            hint: Some("xcode-select --install".to_string()),
        };
    }
    // Linux: gcc/g++ y las variantes estáticas de libgomp y libstdc++. El
    // binario de la app las pide estáticas (runtime OpenMP de CTranslate2 y
    // oneDNN, y librería de C++ de los crates C++), y sin sus `.a` el enlace
    // falla.
    let want = Some("gcc/g++ y libgomp y libstdc++ estáticas".to_string());
    let found = match probe_version_token("gcc", &["--version"]) {
        Ok(found) => found,
        Err(reason) => return probe_error_row("c-compiler", label, true, want, reason),
    };
    if found.is_some() && gcc_static_library("libgomp.a") && gcc_static_library("libstdc++.a") {
        return Row {
            id: "c-compiler",
            label,
            mandatory: true,
            status: Status::Ok,
            found: found.map(|v| format!("gcc {v} + libgomp.a + libstdc++.a")),
            want,
            hint: None,
        };
    }
    let gcc_major = gcc_major_version();
    let hint = detect_linux_manager()
        .map(|m| linux_install_command(m, &c_compiler_packages(m, &gcc_major)))
        .or_else(|| {
            Some(
                "instala gcc, g++ y las librerías estáticas de libgomp y libstdc++ con el gestor de tu distribución"
                    .to_string(),
            )
        });
    Row {
        id: "c-compiler",
        label,
        mandatory: true,
        status: Status::Missing,
        found: None,
        want,
        hint,
    }
}

/// Sonda CMake ≥ 3.20.
fn check_cmake() -> Row {
    let label = "CMake ≥ 3.20";
    let found = match probe_version_token("cmake", &["--version"]) {
        Ok(found) => found,
        Err(reason) => {
            return probe_error_row("cmake", label, true, Some("≥ 3.20".to_string()), reason)
        }
    };
    match found {
        Some(v) if version_at_least(&v, 3, 20) => Row {
            id: "cmake",
            label,
            mandatory: true,
            status: Status::Ok,
            found: Some(v),
            want: Some("≥ 3.20".to_string()),
            hint: None,
        },
        found => {
            let hint = if cfg!(windows) {
                Some("winget install Kitware.CMake".to_string())
            } else if cfg!(target_os = "macos") {
                Some("brew install cmake".to_string())
            } else {
                detect_linux_manager()
                    .map(|m| linux_install_command(m, "cmake"))
                    .or_else(|| Some("instala cmake con el gestor de tu distribución".to_string()))
            };
            Row {
                id: "cmake",
                label,
                mandatory: true,
                status: if found.is_some() {
                    Status::Mismatch
                } else {
                    Status::Missing
                },
                found,
                want: Some("≥ 3.20".to_string()),
                hint,
            }
        }
    }
}

/// Si `gcc` encuentra la librería estática `name` en su directorio.
fn gcc_static_library(name: &str) -> bool {
    gcc_static_library_found(probe_output("gcc", &[&format!("-print-file-name={name}")]))
}

/// Si la salida de `gcc -print-file-name=<lib>` nombra una librería que
/// existe. Sin la librería, gcc imprime el nombre tal cual, sin ruta.
fn gcc_static_library_found(printed: Option<String>) -> bool {
    printed.is_some_and(|p| Path::new(&p).is_absolute() && Path::new(&p).is_file())
}

/// Versión mayor del gcc instalado, vacía si no se pudo leer. Los paquetes
/// `-<N>-dev` deben ser los de esa versión: gcc solo busca las librerías
/// estáticas en su propio directorio de versión.
fn gcc_major_version() -> String {
    probe_output("gcc", &["-dumpversion"])
        .and_then(|v| v.split('.').next().map(str::to_string))
        .unwrap_or_default()
}

/// Paquetes del compilador C/C++ del workspace en Linux, con las variantes
/// estáticas de `libgomp` y `libstdc++` que pide el binario de la app.
fn c_compiler_packages(manager: &str, gcc_major: &str) -> String {
    match manager {
        "apt" if !gcc_major.is_empty() => {
            format!("gcc g++ libgcc-{gcc_major}-dev libstdc++-{gcc_major}-dev")
        }
        "apt" => "gcc g++".to_string(),
        "dnf" => "gcc gcc-c++ libstdc++-static".to_string(),
        "pacman" => "gcc".to_string(),
        _ => "gcc g++ libstdc++-static o equivalentes".to_string(),
    }
}

/// Sonda ONNX Runtime: `ort-bundle/` fresco para el pin (marcador más
/// librería del SO). Solo lectura: descargarlo es tarea de `bootstrap`.
fn check_onnx(root: &Path, pins: &Pins) -> Row {
    let label = "ONNX Runtime (versión fijada)";
    let want = Some(pins.ort.clone());
    let bundle = super::package::ort_bundle_dir(root);
    let lib = match super::package::host_triple()
        .and_then(|triple| super::package::ort_dylib_path(&bundle, triple))
    {
        Ok(lib) => lib,
        Err(e) => return probe_error_row("onnx", label, true, want, e.to_string()),
    };
    if !lib.is_file() {
        return Row {
            id: "onnx",
            label,
            mandatory: true,
            status: Status::Missing,
            found: None,
            want,
            hint: Some("cargo xtask bootstrap".to_string()),
        };
    }
    match read_ort_marker(&bundle) {
        OrtMarker::Version(version) if version == pins.ort => Row {
            id: "onnx",
            label,
            mandatory: true,
            status: Status::Ok,
            found: Some(format!("ort-bundle {}", pins.ort)),
            want,
            hint: None,
        },
        other => Row {
            id: "onnx",
            label,
            mandatory: true,
            status: Status::Mismatch,
            found: Some(other.describe(&pins.ort)),
            want,
            hint: Some("cargo xtask bootstrap".to_string()),
        },
    }
}

/// Sonda FFmpeg: `ffmpeg-bundle/` fresco para el SHA-256 fijado del triple
/// (ejecutable no vacío y marcador coincidente). Solo lectura: descargarlo es
/// tarea de `bootstrap`.
fn check_ffmpeg(root: &Path, pins: &Pins) -> Row {
    let label = "FFmpeg (versión fijada)";
    let want = Some(pins.ffmpeg.clone());
    let bundle = super::package::ffmpeg_bundle_dir(root);
    let row = |status: Status, found: Option<String>| Row {
        id: "ffmpeg",
        label,
        mandatory: true,
        hint: (status != Status::Ok).then(|| "cargo xtask bootstrap".to_string()),
        status,
        found,
        want: want.clone(),
    };
    if !bundle.is_dir() {
        return row(Status::Missing, None);
    }
    let fresh = super::package::host_triple().and_then(|triple| {
        let sha = super::package::pinned_ffmpeg_sha256(pins, triple)?;
        super::package::ffmpeg_bundle_is_fresh(&bundle, triple, sha)
    });
    let dir = super::package::FFMPEG_BUNDLE_REL;
    match fresh {
        Ok(true) => row(Status::Ok, Some(format!("{dir} {}", pins.ffmpeg))),
        Ok(false) => row(
            Status::Mismatch,
            Some(format!(
                "{dir} incompleto o de otro artefacto (fijado {})",
                pins.ffmpeg
            )),
        ),
        Err(e) => row(Status::Error, Some(e.to_string())),
    }
}

/// Sonda del motor TTS: `engine-bundle/` fresco para el SHA-256 fijado del
/// triple (ejecutable no vacío y marcador coincidente). El motor no declara su
/// versión por sí mismo, así que la frescura se decide por el binario y el
/// marcador, nunca ejecutándolo. Solo lectura: descargarlo es tarea de
/// `bootstrap`.
fn check_engine_bundle(root: &Path, pins: &Pins) -> Row {
    let label = "Motor TTS qwen-tts-engine (bundle fijado)";
    let want = Some(pins.qwen_tts_engine.clone());
    let bundle = super::package::engine_bundle_dir(root);
    let row = |status: Status, found: Option<String>| Row {
        id: "qwen-tts-engine",
        label,
        mandatory: true,
        hint: (status != Status::Ok).then(|| "cargo xtask bootstrap".to_string()),
        status,
        found,
        want: want.clone(),
    };
    if !bundle.is_dir() {
        return row(Status::Missing, None);
    }
    let fresh = super::package::host_triple().and_then(|triple| {
        let sha = super::package::pinned_engine_sha256(pins, triple)?;
        super::package::engine_bundle_is_fresh(&bundle, triple, sha)
    });
    let dir = super::package::ENGINE_BUNDLE_REL;
    match fresh {
        Ok(true) => row(Status::Ok, Some(format!("{dir} {}", pins.qwen_tts_engine))),
        Ok(false) => row(
            Status::Mismatch,
            Some(format!(
                "{dir} incompleto o de otro artefacto (fijado {})",
                pins.qwen_tts_engine
            )),
        ),
        Err(e) => row(Status::Error, Some(e.to_string())),
    }
}

/// Contenido del marcador `.ort-version` de un `ort-bundle/`.
enum OrtMarker {
    /// El marcador no existe: la versión del bundle es desconocida.
    Absent,
    /// El marcador existe pero no se pudo leer (motivo del error).
    Unreadable(String),
    /// Versión escrita en el marcador, sin espacios sobrantes.
    Version(String),
}

impl OrtMarker {
    /// Texto de lo hallado en el bundle, contrastado con el pin.
    fn describe(&self, pinned: &str) -> String {
        match self {
            Self::Absent => {
                format!(
                    "ort-bundle sin marcador .ort-version (versión desconocida; fijado {pinned})"
                )
            }
            Self::Unreadable(reason) => {
                format!("marcador .ort-version ilegible: {reason} (fijado {pinned})")
            }
            Self::Version(version) => format!("ort-bundle {version} (fijado {pinned})"),
        }
    }
}

/// Lee el marcador de versión de `bundle`; un marcador ausente no se confunde
/// con uno ilegible.
fn read_ort_marker(bundle: &Path) -> OrtMarker {
    match std::fs::read_to_string(bundle.join(super::package::ORT_MARKER)) {
        Ok(text) => OrtMarker::Version(text.trim().to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => OrtMarker::Absent,
        Err(e) => OrtMarker::Unreadable(e.to_string()),
    }
}

/// Sonda sccache (opcional): la instalada debe cubrir el pin; si falta es
/// solo aviso. La comparación tolera el sufijo de empaquetado como el resto.
fn check_sccache(pins: &Pins) -> Row {
    let label = "sccache";
    let want = Some(pins.sccache.clone());
    let found = match probe_version_token("sccache", &["--version"]) {
        Ok(found) => found,
        Err(reason) => return probe_error_row("sccache", label, false, want, reason),
    };
    match &found {
        Some(version) if pin_covers(version, &pins.sccache) => Row {
            id: "sccache",
            label,
            mandatory: false,
            status: Status::Ok,
            found,
            want,
            hint: None,
        },
        Some(_) => Row {
            id: "sccache",
            label,
            mandatory: false,
            status: Status::Mismatch,
            found,
            want,
            hint: None,
        },
        None => Row {
            id: "sccache",
            label,
            mandatory: false,
            status: Status::Missing,
            found: None,
            want,
            hint: Some("cargo install sccache o el gestor de tu sistema".to_string()),
        },
    }
}

/// Umbral de aviso de `target/`: por encima, `doctor` sugiere la poda
/// fina. Comparte valor con el hook `post-merge` (ver `CONTRIBUTING.md`).
/// Son 10 GB decimales (10 000 000 000 bytes).
pub(crate) const TARGET_HYGIENE_MAX_GB: u64 = 10;

/// Núcleo puro del aviso: hay que avisar si `target/` supera el umbral o el
/// hook de poda no está activo. Pura sobre valores ya leídos, así se testea
/// sin disco.
pub(crate) fn target_hygiene_warn(size_bytes: u64, hook_active: Option<bool>) -> bool {
    const LIMIT: u64 = TARGET_HYGIENE_MAX_GB * 1000 * 1000 * 1000;
    size_bytes > LIMIT || hook_active == Some(false)
}

/// ¿Apunta `core.hooksPath` a `.githooks`? `None` si git no responde: no
/// penalizar un entorno sin git por un aviso de higiene.
fn probe_hooks_path(root: &Path) -> Option<bool> {
    let out = probe_command("git")
        .current_dir(root)
        .args(["config", "core.hooksPath"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(hooks_path_is_githooks(
        root,
        String::from_utf8_lossy(&out.stdout).trim(),
    ))
}

/// ¿El valor de `core.hooksPath` designa el `.githooks` del repositorio? Git
/// acepta una ruta relativa a la raíz o una absoluta, así que se compara la
/// ruta resuelta en disco y no el texto: `.githooks` y
/// `C:\…\repo\.githooks` son el mismo hook.
fn hooks_path_is_githooks(root: &Path, configured: &str) -> bool {
    if configured.is_empty() {
        return false;
    }
    let configured = root.join(configured);
    match (
        configured.canonicalize(),
        root.join(".githooks").canonicalize(),
    ) {
        (Ok(configured), Ok(expected)) => configured == expected,
        _ => false,
    }
}

/// Sonda de higiene de `target/` (opcional): tamaño frente al umbral y
/// estado del hook de poda. Solo lectura; nunca bloquea `ok()`.
fn check_target_hygiene(root: &Path) -> Row {
    let measured = super::clean::size_of(&root.join("target"));
    let size = measured.bytes;
    let hook = probe_hooks_path(root);
    let warn = target_hygiene_warn(size, hook);
    let hook_text = match hook {
        Some(true) => "hook activo",
        Some(false) => "hook inactivo",
        None => "hook desconocido",
    };
    let mut hints = Vec::new();
    if size > TARGET_HYGIENE_MAX_GB * 1000 * 1000 * 1000 {
        hints.push(
            "libera con `cargo xtask clean --prune --dry-run` (`--yes` para borrar)".to_string(),
        );
    }
    if hook == Some(false) {
        hints.push("activa el hook: `git config core.hooksPath .githooks`".to_string());
    }
    Row {
        id: "target-hygiene",
        label: "higiene de target",
        mandatory: false,
        status: if warn { Status::Mismatch } else { Status::Ok },
        found: Some(format!("{} ({hook_text})", measured.text())),
        want: Some(format!("máx. {TARGET_HYGIENE_MAX_GB} GB")),
        hint: if hints.is_empty() {
            None
        } else {
            Some(hints.join("; "))
        },
    }
}

/// Deriva de ONNX: `ort-bundle/` ausente o con marcador ausente, ilegible o
/// distinto del pin.
fn probe_onnx_drift(root: &Path, pins: &Pins) -> Option<String> {
    let bundle = super::package::ort_bundle_dir(root);
    if !bundle.is_dir() {
        return Some(format!(
            "ONNX Runtime sin descargar (fijado {}; lo provee `cargo xtask bootstrap`)",
            pins.ort
        ));
    }
    match read_ort_marker(&bundle) {
        OrtMarker::Version(version) if version == pins.ort => None,
        other => Some(format!(
            "ONNX Runtime: {} (ejecuta `cargo xtask bootstrap`)",
            other.describe(&pins.ort)
        )),
    }
}

/// `true` si los modelos ya están provisionados según la integridad mínima
/// que exige `avi_store::ModelStore`.
pub(crate) fn models_are_provisioned() -> bool {
    models_drift().is_empty()
}

/// Deriva de modelos obligatorios, validando snapshots y ficheros requeridos.
fn models_drift() -> Vec<String> {
    let models = match avi_shared::paths::models_cache_dir() {
        Ok(models) => models,
        Err(e) => return vec![format!("no se pudo resolver la caché de modelos: {e}")],
    };
    let store = avi_store::ModelStore::at(models);
    model_drift_for_store(&store)
}

fn model_drift_for_store(store: &avi_store::ModelStore) -> Vec<String> {
    avi_shared::paths::MODEL_REVISIONS
        .iter()
        .filter_map(|pin| match store.provisioning_status(pin.name) {
            Ok(true) => None,
            Ok(false) => Some(format!(
                "modelo {} incompleto o sin provisionar (ejecuta `cargo xtask bootstrap --models`)",
                pin.name
            )),
            Err(error) => Some(format!(
                "no se pudo comprobar el modelo {}: {error}",
                pin.name
            )),
        })
        .collect()
}

/// Ejecuta las sondas integrales del entorno de desarrollo sobre `root`.
pub(crate) fn check(root: &Path, pins: &Pins) -> Report {
    let rows = vec![
        check_rust(pins),
        check_c_compiler(),
        check_cmake(),
        check_onnx(root, pins),
        check_ffmpeg(root, pins),
        check_engine_bundle(root, pins),
        check_sccache(pins),
        check_target_hygiene(root),
    ];
    debug_assert_eq!(
        rows.iter().map(|r| r.id).collect::<Vec<_>>(),
        ROW_IDS,
        "la tabla cubre cada fila de requisitos en orden"
    );
    Report {
        rows,
        onnx_drift: probe_onnx_drift(root, pins),
        models_drift: models_drift(),
    }
}

/// Lee los pines desde la raíz del repositorio.
pub(crate) fn load_pins(root: &Path) -> Result<Pins> {
    avi_shared::pins::load_from_root(root).map_err(|e| anyhow::anyhow!("{e}"))
}

fn status_text(status: &Status) -> &'static str {
    match status {
        Status::Ok => "correcto",
        Status::Missing => "falta",
        Status::Mismatch => "versión distinta",
        Status::Error => "no se pudo comprobar",
    }
}

/// Punto de entrada de `xtask doctor`.
pub fn run(json: bool) -> Result<()> {
    let root = std::env::current_dir()?;
    let pins = load_pins(&root)?;
    let report = check(&root, &pins);
    if json {
        print_json(&report, &pins)?;
    } else {
        print_human(&report);
    }
    if report.ok() {
        Ok(())
    } else {
        anyhow::bail!("faltan requisitos obligatorios (ver arriba el comando exacto para cada uno)")
    }
}

/// Verifica que estén completos los recursos fijados que asegura `bootstrap`
/// (ONNX Runtime, FFmpeg y el motor TTS), los modelos, la ausencia de
/// `.incomplete` y la de residuos, sin las comprobaciones del ciclo de vida.
pub fn verify_resources(json: bool) -> Result<()> {
    use serde_json::json;

    let root = std::env::current_dir()?;
    let pins = load_pins(&root)?;
    let triple = super::package::host_triple()?;
    let models = models_drift();
    let super::package::PinnedBundles {
        ort,
        ffmpeg,
        engine,
    } = super::package::pinned_bundles_status(&root, triple, &pins);
    let incomplete = find_incomplete_files(&models_cache_root()?);
    let pending = find_pending_resources(&root);
    let mut errors = Vec::new();
    let ort_ok = match ort {
        Ok(ok) => ok,
        Err(error) => {
            errors.push(format!("ONNX Runtime: {error}"));
            false
        }
    };
    let ffmpeg_ok = match ffmpeg {
        Ok(ok) => ok,
        Err(error) => {
            errors.push(format!("FFmpeg: {error}"));
            false
        }
    };
    let engine_ok = match engine {
        Ok(ok) => ok,
        Err(error) => {
            errors.push(format!("Motor TTS qwen-tts-engine: {error}"));
            false
        }
    };
    let incomplete = match incomplete {
        Ok(paths) => paths,
        Err(error) => {
            errors.push(format!(
                "no se pudieron comprobar archivos incompletos: {error}"
            ));
            Vec::new()
        }
    };
    let pending = match pending {
        Ok(paths) => paths,
        Err(error) => {
            errors.push(format!("no se pudieron comprobar residuos: {error}"));
            Vec::new()
        }
    };
    let ok = models.is_empty()
        && ort_ok
        && ffmpeg_ok
        && engine_ok
        && incomplete.is_empty()
        && pending.is_empty()
        && errors.is_empty();
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "ok": ok,
                "target": triple,
                "models": { "ok": models.is_empty(), "missing": models },
                "onnx_runtime": { "ok": ort_ok, "version": pins.ort },
                "ffmpeg": { "ok": ffmpeg_ok, "version": pins.ffmpeg },
                "qwen_tts_engine": { "ok": engine_ok, "version": pins.qwen_tts_engine },
                "incomplete": incomplete,
                "pending": pending,
                "errors": errors,
            }))?
        );
    } else {
        println!("Recursos: {}", if ok { "completos" } else { "incompletos" });
        for model in models {
            println!("  - {model}");
        }
        println!(
            "  ONNX Runtime: {}",
            if ort_ok {
                "correcto"
            } else {
                "falta o está incompleto"
            }
        );
        println!(
            "  FFmpeg: {}",
            if ffmpeg_ok {
                "correcto"
            } else {
                "falta o está incompleto"
            }
        );
        println!(
            "  Motor TTS qwen-tts-engine: {}",
            if engine_ok {
                "correcto"
            } else {
                "falta o está incompleto"
            }
        );
        println!("  Archivos .incomplete: {}", incomplete.len());
        println!("  Residuos pendientes: {}", pending.len());
        for error in &errors {
            println!("  Error de lectura: {error}");
        }
    }
    if ok {
        Ok(())
    } else {
        anyhow::bail!("faltan recursos requeridos; consulta el informe")
    }
}

fn find_pending_resources(root: &Path) -> Result<Vec<String>> {
    let mut pending = Vec::new();
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        if path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().ends_with(".removal-failed.log"))
        {
            pending.push(path.display().to_string());
        }
    }
    pending.sort();
    Ok(pending)
}

fn models_cache_root() -> Result<PathBuf> {
    Ok(avi_shared::paths::models_cache_dir()?)
}

fn find_incomplete_files(root: &Path) -> Result<Vec<String>> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(anyhow::anyhow!(
                    "no se pudo leer {}: {error}",
                    dir.display()
                ))
            }
        };
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                stack.push(path);
            } else if path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().ends_with(".incomplete"))
            {
                found.push(path.display().to_string());
            }
        }
    }
    found.sort();
    Ok(found)
}

fn print_human(report: &Report) {
    println!("Requisitos del entorno de desarrollo:");
    for row in &report.rows {
        let state = status_text(&row.status);
        let detail = match (&row.found, &row.want) {
            (Some(found), Some(want)) => format!(" [{found} | fijado: {want}]"),
            (Some(found), None) => format!(" [{found}]"),
            (None, Some(want)) => format!(" [fijado: {want}]"),
            (None, None) => String::new(),
        };
        let optional = if row.mandatory { "" } else { " (opcional)" };
        println!(
            "  [{}] {}{}: {}{}",
            state, row.label, optional, row.id, detail
        );
        if let Some(hint) = &row.hint {
            if row.status != Status::Ok {
                println!("        → {hint}");
            }
        }
    }
    let drifts: Vec<&str> = report
        .onnx_drift
        .iter()
        .map(|s| s.as_str())
        .chain(report.models_drift.iter().map(|s| s.as_str()))
        .collect();
    if drifts.is_empty() {
        println!("Deriva del entorno: ninguna.");
    } else {
        println!("Deriva del entorno:");
        for drift in drifts {
            println!("  - {drift}");
        }
    }
    if report.ok() {
        println!("Entorno listo.");
    }
}

fn print_json(report: &Report, pins: &Pins) -> Result<()> {
    let rows: Vec<serde_json::Value> = report
        .rows
        .iter()
        .map(|r| {
            serde_json::json!({
                "id": r.id,
                "requirement": r.label,
                "mandatory": r.mandatory,
                "status": status_text(&r.status),
                "found": r.found,
                "expected": r.want,
                "command": r.hint,
            })
        })
        .collect();
    let out = serde_json::json!({
        "ok": report.ok(),
        "pins": {
            "rust": pins.rust,
            "ort": pins.ort,
            "sccache": pins.sccache,
            "ninja": pins.ninja,
        },
        "requirements": rows,
        "drift": {
            "onnx": report.onnx_drift,
            "models": report.models_drift,
        },
    });
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pines de fixture (los reales viven en `packaging/pins.json`).
    fn fixture_pins() -> Pins {
        Pins {
            rust: "1.96.0".to_string(),
            ort: "1.28.0".to_string(),
            ort_sha256: std::collections::BTreeMap::new(),
            ffmpeg: "9.0.2".to_string(),
            ffmpeg_sha256: std::collections::BTreeMap::new(),
            ffmpeg_source_sha256: std::collections::BTreeMap::new(),
            qwen_tts_engine: "1.0.0".to_string(),
            qwen_tts_engine_sha256: std::collections::BTreeMap::new(),
            qwen_tts_engine_source_sha256: "b".repeat(64),
            sccache: "0.8.2".to_string(),
            ninja: "1.13.2".to_string(),
            bats: "1.14.0".to_string(),
            pwsh: "7.6.6".to_string(),
            pester: "5.8.0".to_string(),
            nextest: "0.9.148".to_string(),
            windows_image: "2026.10.1".to_string(),
        }
    }

    /// La tabla cubre cada fila de requisitos sin duplicados ni filas de más.
    #[test]
    fn table_covers_requirement_rows_once() {
        let mut ids = ROW_IDS.to_vec();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), ROW_IDS.len(), "sin duplicados");
        assert_eq!(
            ids,
            [
                "c-compiler",
                "cmake",
                "ffmpeg",
                "onnx",
                "qwen-tts-engine",
                "rust",
                "sccache",
                "target-hygiene"
            ]
        );
    }

    /// Los pines que consume la tabla se cargan de `packaging/pins.json` con el
    /// mismo cargador que usa `doctor`, y ninguno viene vacío.
    #[test]
    fn pins_match_json_source() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
        let pins = avi_shared::pins::load_from_root(&root).expect("pins.json válido");
        for field in [
            &pins.rust,
            &pins.ort,
            &pins.sccache,
            &pins.ninja,
            &pins.pester,
        ] {
            assert!(!field.trim().is_empty());
        }
    }

    /// `check` sobre fixtures produce las filas declaradas en orden y `ok()` solo
    /// exige las obligatorias (`sccache` y `target-hygiene` no bloquean).
    #[test]
    fn check_yields_all_rows_in_order() {
        let root_guard = tempfile::Builder::new()
            .prefix("xtask_doctor_")
            .tempdir()
            .unwrap();
        let report = check(root_guard.path(), &fixture_pins());
        let ids: Vec<&str> = report.rows.iter().map(|r| r.id).collect();
        assert_eq!(ids, ROW_IDS);
        let optional: Vec<&str> = report
            .rows
            .iter()
            .filter(|r| !r.mandatory)
            .map(|r| r.id)
            .collect();
        assert_eq!(optional, ["sccache", "target-hygiene"]);
    }

    /// El aviso salta por tamaño sobre el umbral o por hook inactivo; un
    /// hook desconocido (sin git) no penaliza cuando el tamaño está bien.
    #[test]
    fn target_hygiene_predicate() {
        const GB: u64 = 1000 * 1000 * 1000;
        assert!(!target_hygiene_warn(0, Some(true)));
        assert!(!target_hygiene_warn(10 * GB, Some(true)));
        assert!(!target_hygiene_warn(0, None));
        assert!(target_hygiene_warn(10 * GB + 1, Some(true)));
        assert!(target_hygiene_warn(0, Some(false)));
        assert!(target_hygiene_warn(10 * GB + 1, None));
    }

    /// `core.hooksPath` cuenta como activo tanto relativo (`.githooks`) como
    /// absoluto hacia el mismo directorio; otro directorio o un valor vacío no.
    #[test]
    fn hooks_path_matches_relative_and_absolute_githooks() {
        let root = tempfile::tempdir().expect("directorio temporal");
        std::fs::create_dir(root.path().join(".githooks")).unwrap();
        std::fs::create_dir(root.path().join("otros-hooks")).unwrap();
        let absolute = root.path().join(".githooks");
        assert!(hooks_path_is_githooks(root.path(), ".githooks"));
        assert!(hooks_path_is_githooks(
            root.path(),
            absolute.to_str().unwrap()
        ));
        assert!(!hooks_path_is_githooks(root.path(), "otros-hooks"));
        assert!(!hooks_path_is_githooks(root.path(), ""));
    }

    #[test]
    fn version_token_parses_tool_outputs() {
        assert_eq!(
            version_token("rustc 1.96.0 (ab123 2026-05-25)"),
            Some("1.96.0".to_string())
        );
        assert_eq!(
            version_token("cmake version 3.28.3"),
            Some("3.28.3".to_string())
        );
        assert_eq!(version_token("sccache 0.8.2"), Some("0.8.2".to_string()));
        assert_eq!(version_token("sin versión aquí"), None);
    }

    #[test]
    fn pin_covers_strips_packaging_suffix() {
        assert!(pin_covers("16.2.0-1", "16.2.0"));
        assert!(pin_covers("0.3.34-1", "0.3.34-1"));
        assert!(!pin_covers("15.1.0-1", "16.2.0"));
    }

    #[test]
    fn version_at_least_compares_minor() {
        assert!(version_at_least("3.28.3", 3, 20));
        assert!(version_at_least("3.20", 3, 20));
        assert!(!version_at_least("3.19.9", 3, 20));
        assert!(!version_at_least("desconocida", 3, 20));
        // Un componente no numérico no se salta: `3.x.25` no es `3.25`.
        assert!(!version_at_least("3.x.25", 3, 20));
    }

    /// Crea un `ort-bundle/` con la librería del SO y, si se indica, el marcador.
    fn onnx_fixture(marker: Option<&str>) -> tempfile::TempDir {
        let root = tempfile::Builder::new()
            .prefix("xtask_doctor_onnx_")
            .tempdir()
            .unwrap();
        let bundle = super::super::package::ort_bundle_dir(root.path());
        std::fs::create_dir(&bundle).unwrap();
        let triple = super::super::package::host_triple().unwrap();
        let lib = super::super::package::ort_dylib_path(&bundle, triple).unwrap();
        std::fs::write(lib, b"").unwrap();
        if let Some(text) = marker {
            std::fs::write(bundle.join(super::super::package::ORT_MARKER), text).unwrap();
        }
        root
    }

    /// Sin marcador la versión del bundle es desconocida: fila `Mismatch` con la
    /// pista de `bootstrap` y deriva informada.
    #[test]
    fn onnx_without_marker_is_a_mismatch() {
        let root = onnx_fixture(None);
        let row = check_onnx(root.path(), &fixture_pins());
        assert_eq!(row.status, Status::Mismatch);
        assert!(row.hint.unwrap().contains("cargo xtask bootstrap"));
        assert!(probe_onnx_drift(root.path(), &fixture_pins()).is_some());
    }

    #[test]
    fn onnx_with_matching_marker_is_ok_and_other_version_drifts() {
        let pins = fixture_pins();
        let good = onnx_fixture(Some("1.28.0\n"));
        assert_eq!(check_onnx(good.path(), &pins).status, Status::Ok);
        assert!(probe_onnx_drift(good.path(), &pins).is_none());
        let stale = onnx_fixture(Some("1.20.0"));
        assert_eq!(check_onnx(stale.path(), &pins).status, Status::Mismatch);
        assert!(probe_onnx_drift(stale.path(), &pins).is_some());
    }

    /// Un marcador que existe pero no se puede leer (aquí, un directorio) se
    /// informa como ilegible y no como versión vacía.
    #[test]
    fn onnx_unreadable_marker_is_reported() {
        let root = onnx_fixture(None);
        std::fs::create_dir(
            super::super::package::ort_bundle_dir(root.path())
                .join(super::super::package::ORT_MARKER),
        )
        .unwrap();
        let row = check_onnx(root.path(), &fixture_pins());
        assert_eq!(row.status, Status::Mismatch);
        assert!(row.found.unwrap().contains("ilegible"));
    }

    #[test]
    fn linux_hints_use_exact_manager_command() {
        assert_eq!(
            linux_install_command("apt", "cmake"),
            "sudo apt install cmake"
        );
        assert_eq!(
            linux_install_command("dnf", "cmake"),
            "sudo dnf install cmake"
        );
        assert_eq!(
            linux_install_command("pacman", "cmake"),
            "sudo pacman -S cmake"
        );
        assert_eq!(
            linux_install_command("zypper", "cmake"),
            "sudo zypper install cmake"
        );
    }

    /// Una ruta absoluta a un fichero existente cuenta como presente; el
    /// nombre suelto que gcc imprime sin la librería, o ninguna salida, no.
    #[test]
    fn gcc_static_library_requires_existing_absolute_path() {
        let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        assert!(gcc_static_library_found(Some(
            file.to_string_lossy().into_owned()
        )));
        assert!(!gcc_static_library_found(Some("libgomp.a".to_string())));
        assert!(!gcc_static_library_found(None));
    }

    /// El compilador del workspace propone los paquetes de `libgomp.a` y
    /// `libstdc++.a`. Con la versión de gcc, apt pide los paquetes de
    /// desarrollo de esa versión.
    #[test]
    fn linux_packages_follow_compiler_static_libraries() {
        let apt = c_compiler_packages("apt", "14");
        for package in ["g++", "libgcc-14-dev", "libstdc++-14-dev"] {
            assert!(apt.contains(package), "{apt}");
        }
        assert!(c_compiler_packages("dnf", "").contains("libstdc++-static"));
        assert!(!c_compiler_packages("apt", "14").contains("gfortran"));
    }

    #[test]
    fn onnx_row_reports_missing_bundle() {
        let root_guard = tempfile::Builder::new()
            .prefix("xtask_doctor_onnx_")
            .tempdir()
            .unwrap();
        let row = check_onnx(root_guard.path(), &fixture_pins());
        assert_eq!(row.id, "onnx");
        assert_eq!(row.status, Status::Missing);
        assert_eq!(row.want, Some("1.28.0".to_string()));
        assert!(row.hint.unwrap().contains("bootstrap"));
    }

    /// Pines de fixture con el SHA-256 de FFmpeg fijado para el triple del host.
    fn ffmpeg_fixture_pins(sha: &str) -> Pins {
        let mut pins = fixture_pins();
        let triple = super::super::package::host_triple().unwrap();
        pins.ffmpeg_sha256
            .insert(triple.to_string(), sha.to_string());
        pins
    }

    /// Escribe en `ffmpeg-bundle/` un ejecutable no vacío y el marcador dado.
    fn write_ffmpeg_bundle(root: &Path, marker: &str) {
        let bundle = super::super::package::ffmpeg_bundle_dir(root);
        std::fs::create_dir_all(&bundle).unwrap();
        let triple = super::super::package::host_triple().unwrap();
        let name = super::super::package::ffmpeg_binary_name(triple).unwrap();
        std::fs::write(bundle.join(name), b"ffmpeg").unwrap();
        std::fs::write(bundle.join(super::super::package::FFMPEG_MARKER), marker).unwrap();
    }

    /// Sin bundle la fila FFmpeg falta; con marcador de otro artefacto es una
    /// versión distinta; con ejecutable y marcador del hash fijado es correcta.
    /// Solo la correcta prescinde de la pista de `bootstrap`.
    #[test]
    fn ffmpeg_row_reports_missing_mismatch_and_ok() {
        let sha = "a".repeat(64);
        let pins = ffmpeg_fixture_pins(&sha);
        let root = tempfile::tempdir().unwrap();
        let row = check_ffmpeg(root.path(), &pins);
        assert_eq!(row.id, "ffmpeg");
        assert!(row.mandatory);
        assert_eq!(row.status, Status::Missing);
        assert_eq!(row.want, Some("9.0.2".to_string()));
        assert!(row.hint.unwrap().contains("cargo xtask bootstrap"));
        write_ffmpeg_bundle(root.path(), &"b".repeat(64));
        let row = check_ffmpeg(root.path(), &pins);
        assert_eq!(row.status, Status::Mismatch);
        assert!(row.hint.is_some());
        write_ffmpeg_bundle(root.path(), &sha);
        let row = check_ffmpeg(root.path(), &pins);
        assert_eq!(row.status, Status::Ok);
        assert!(row.hint.is_none());
    }

    /// Un bundle presente sin hash de FFmpeg fijado para el triple no se puede
    /// contrastar: la fila es un error, no una ausencia ni una coincidencia.
    #[test]
    fn ffmpeg_row_without_pin_is_an_error() {
        let root = tempfile::tempdir().unwrap();
        write_ffmpeg_bundle(root.path(), &"a".repeat(64));
        let row = check_ffmpeg(root.path(), &fixture_pins());
        assert_eq!(row.status, Status::Error);
        assert!(row.found.unwrap().contains("FFmpeg"));
    }

    /// Pines de fixture con el SHA-256 del motor fijado para el triple del host.
    fn engine_fixture_pins(sha: &str) -> Pins {
        let mut pins = fixture_pins();
        let triple = super::super::package::host_triple().unwrap();
        pins.qwen_tts_engine_sha256
            .insert(triple.to_string(), sha.to_string());
        pins
    }

    /// Escribe en `engine-bundle/` un ejecutable no vacío y el marcador dado.
    fn write_engine_bundle(root: &Path, marker: &str) {
        let bundle = super::super::package::engine_bundle_dir(root);
        std::fs::create_dir_all(&bundle).unwrap();
        let triple = super::super::package::host_triple().unwrap();
        let name = super::super::package::engine_binary_name(triple).unwrap();
        std::fs::write(bundle.join(name), b"qwen_tts").unwrap();
        std::fs::write(bundle.join(super::super::package::ENGINE_MARKER), marker).unwrap();
    }

    /// Sin bundle la fila del motor falta; con marcador de otro artefacto es
    /// una versión distinta; con ejecutable y marcador del hash fijado es
    /// correcta. Solo la correcta prescinde de la pista de `bootstrap`.
    #[test]
    fn engine_row_reports_missing_mismatch_and_ok() {
        let sha = "a".repeat(64);
        let pins = engine_fixture_pins(&sha);
        let root = tempfile::tempdir().unwrap();
        let row = check_engine_bundle(root.path(), &pins);
        assert_eq!(row.id, "qwen-tts-engine");
        assert!(row.mandatory);
        assert_eq!(row.status, Status::Missing);
        assert_eq!(row.want, Some("1.0.0".to_string()));
        assert!(row.hint.unwrap().contains("cargo xtask bootstrap"));
        write_engine_bundle(root.path(), &"b".repeat(64));
        let row = check_engine_bundle(root.path(), &pins);
        assert_eq!(row.status, Status::Mismatch);
        assert!(row.hint.is_some());
        write_engine_bundle(root.path(), &sha);
        let row = check_engine_bundle(root.path(), &pins);
        assert_eq!(row.status, Status::Ok);
        assert!(row.hint.is_none());
    }

    /// Un bundle presente sin hash del motor fijado para el triple no se puede
    /// contrastar: la fila es un error, no una ausencia ni una coincidencia.
    #[test]
    fn engine_row_without_pin_is_an_error() {
        let root = tempfile::tempdir().unwrap();
        write_engine_bundle(root.path(), &"a".repeat(64));
        let row = check_engine_bundle(root.path(), &fixture_pins());
        assert_eq!(row.status, Status::Error);
        assert!(row.found.unwrap().contains("qwen-tts-engine"));
    }

    /// Un programa inexistente es «no instalado»; uno que no se puede ejecutar
    /// por otra causa (aquí, un directorio) es un error, no una ausencia.
    #[test]
    fn probe_version_distinguishes_missing_from_unrunnable() {
        let missing = "program-that-does-not-exist-xtask-doctor";
        assert_eq!(probe_version(missing, &["--version"]), Ok(None));
        let dir = tempfile::tempdir().unwrap();
        assert!(probe_version(&dir.path().to_string_lossy(), &["--version"]).is_err());
        let exe = std::env::current_exe().expect("ejecutable de pruebas");
        assert!(probe_version(&exe.to_string_lossy(), &["--list"])
            .unwrap()
            .is_some());
    }

    #[test]
    fn model_drift_uses_store_integrity_contract() {
        let root = tempfile::tempdir().unwrap();
        let store = avi_store::ModelStore::at(root.path().to_path_buf());
        assert_eq!(
            model_drift_for_store(&store).len(),
            avi_shared::paths::MODEL_REVISIONS.len()
        );
    }

    #[test]
    fn resource_gate_components_reject_truncated_bundles_and_report_missing_models() {
        let root = tempfile::tempdir().unwrap();
        let triple = if cfg!(windows) {
            "x86_64-pc-windows-msvc"
        } else if cfg!(target_os = "macos") {
            "aarch64-apple-darwin"
        } else {
            "x86_64-unknown-linux-gnu"
        };
        let pins = fixture_pins();
        let models = avi_store::ModelStore::at(root.path().to_path_buf());
        assert_eq!(
            model_drift_for_store(&models).len(),
            avi_shared::paths::MODEL_REVISIONS.len()
        );

        let ort = root.path().join("ort");
        std::fs::create_dir_all(&ort).unwrap();
        let ort_lib = super::super::package::ort_dylib_path(&ort, triple).unwrap();
        std::fs::write(ort_lib, b"").unwrap();
        assert!(!super::super::package::ort_bundle_is_fresh(&ort, triple, &pins.ort).unwrap());

        let ffmpeg = root.path().join("ffmpeg");
        std::fs::create_dir_all(&ffmpeg).unwrap();
        std::fs::write(
            ffmpeg.join(if triple.contains("windows") {
                "ffmpeg.exe"
            } else {
                "ffmpeg"
            }),
            b"",
        )
        .unwrap();
        assert!(
            !super::super::package::ffmpeg_bundle_is_fresh(&ffmpeg, triple, &"a".repeat(64))
                .unwrap()
        );
    }

    #[test]
    fn incomplete_scanner_finds_nested_files_and_propagates_access_errors() {
        let root = tempfile::tempdir().unwrap();
        let nested = root.path().join("models--repo").join("blobs");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("weight.incomplete"), b"partial").unwrap();
        assert_eq!(find_incomplete_files(root.path()).unwrap().len(), 1);
        let file = tempfile::NamedTempFile::new().unwrap();
        assert!(find_incomplete_files(file.path()).is_err());
    }
}
