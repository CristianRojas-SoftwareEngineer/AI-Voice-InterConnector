//! `xtask doctor`: diagnóstico solo-lectura del entorno de desarrollo (§10.2).
//!
//! La tabla de §10.1 se declara una sola vez aquí: los pines (Rust, ONNX
//! Runtime, MSYS2…) se leen de `packaging/pins.json` vía `avi-shared`, y de
//! ahí los consumen `doctor`, `bootstrap` y la CI. Por cada requisito informa
//! si está correcto, falta o tiene una versión distinta de la fijada; cuando
//! algo falta, da el comando exacto de instalación para el gestor detectado
//! (apt, dnf, pacman o zypper; `xcode-select` o brew; winget o el `pacman` de
//! MSYS2). Informa además de la deriva del entorno: motor TTS desactualizado
//! respecto de sus fuentes, ONNX Runtime distinto del fijado y modelos sin
//! provisionar. Termina con éxito solo si están todos los obligatorios.
//! Admite `--json`. No modifica nada.

use anyhow::Result;
use avi_shared::pins::Pins;
use std::path::{Path, PathBuf};

/// Filas de la tabla de §10.1, en su orden. `sccache` es la única opcional.
pub(crate) const ROW_IDS: &[&str] = &[
    "rust",
    "c-compiler",
    "cmake",
    "libclang",
    "alsa",
    "engine-toolchain",
    "onnx",
    "sccache",
];

/// Estado de un requisito: correcto, ausente o con versión distinta.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Status {
    Ok,
    Missing,
    Mismatch,
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
    pub engine_drift: Option<String>,
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

/// Salida podada de `programa args…`; `None` si no se pudo lanzar o falló.
fn probe_output(program: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new(program)
        .args(args)
        .output()
        .ok()?;
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

/// Primera sonda que produce salida, probando cada candidato en orden. Los
/// candidatos absolutos (`/sbin/ldconfig`) se comprueban tal cual: `ldconfig`
/// vive en `sbin`, fuera del PATH de shells restringidos, y buscarlo solo por
/// nombre lo declara ausente aunque esté instalado.
fn probe_first(candidates: &[&str], args: &[&str]) -> Option<String> {
    candidates.iter().find_map(|c| probe_output(c, args))
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

/// Versión instalada de un paquete MSYS2 (`pacman -Q nombre` → `nombre
/// versión`); `None` si no está instalado o no se pudo consultar.
fn msys2_package_version(pacman: &Path, package: &str) -> Option<String> {
    let out = std::process::Command::new(pacman)
        .args(["-Q", package])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let version = text.strip_prefix(package)?.trim().to_string();
    if version.is_empty() {
        None
    } else {
        Some(version)
    }
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
    let mut parts = found
        .split(['.', '-'])
        .filter_map(|p| p.parse::<u64>().ok());
    match (parts.next(), parts.next()) {
        (Some(major), Some(minor)) => (major, minor) >= (min_major, min_minor),
        _ => false,
    }
}

/// Sonda Rust: `rustc --version` contra el pin.
fn check_rust(pins: &Pins) -> Row {
    let label = "Rust (versión fijada)";
    let want = Some(pins.rust.clone());
    let Some(out) = probe_output("rustc", &["--version"]) else {
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
            let found = probe_output("cl", &[]).and_then(|o| version_token(&o));
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
        let vswhere =
            Path::new(r"C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe");
        if vswhere.is_file() {
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
    // Linux: gcc/g++.
    let found = probe_output("gcc", &["--version"]).and_then(|o| version_token(&o));
    if found.is_some() {
        return Row {
            id: "c-compiler",
            label,
            mandatory: true,
            status: Status::Ok,
            found,
            want: Some("gcc/g++".to_string()),
            hint: None,
        };
    }
    let hint = detect_linux_manager()
        .map(|m| linux_install_command(m, "gcc g++"))
        .or_else(|| Some("instala gcc y g++ con el gestor de tu distribución".to_string()));
    Row {
        id: "c-compiler",
        label,
        mandatory: true,
        status: Status::Missing,
        found: None,
        want: Some("gcc/g++".to_string()),
        hint,
    }
}

/// Sonda CMake ≥ 3.20.
fn check_cmake() -> Row {
    let label = "CMake ≥ 3.20";
    let found = probe_output("cmake", &["--version"]).and_then(|o| version_token(&o));
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

/// Sonda libclang (bindgen de `ct2rs`).
fn check_libclang() -> Row {
    let label = "libclang (bindgen de ct2rs)";
    if cfg!(windows) {
        if command_exists("clang") {
            let found = probe_output("clang", &["--version"]).and_then(|o| version_token(&o));
            return Row {
                id: "libclang",
                label,
                mandatory: true,
                status: Status::Ok,
                found,
                want: Some("LLVM".to_string()),
                hint: None,
            };
        }
        return Row {
            id: "libclang",
            label,
            mandatory: true,
            status: Status::Missing,
            found: None,
            want: Some("LLVM".to_string()),
            hint: Some("winget install LLVM.LLVM".to_string()),
        };
    }
    if cfg!(target_os = "macos") {
        // Lo cubren las Xcode Command Line Tools.
        return Row {
            id: "libclang",
            label,
            mandatory: true,
            status: if probe_output("xcode-select", &["-p"]).is_some() {
                Status::Ok
            } else {
                Status::Missing
            },
            found: Some("Xcode Command Line Tools".to_string()),
            want: Some("Xcode Command Line Tools".to_string()),
            hint: Some("xcode-select --install".to_string()),
        };
    }
    // Linux: `clang --version` o la librería visible en `ldconfig -p`.
    let clang = probe_output("clang", &["--version"]).and_then(|o| version_token(&o));
    let in_ldconfig = probe_output("ldconfig", &["-p"])
        .is_some_and(|o| o.lines().any(|l| l.contains("libclang")));
    if clang.is_some() || in_ldconfig {
        return Row {
            id: "libclang",
            label,
            mandatory: true,
            status: Status::Ok,
            found: clang.or(Some("libclang en ldconfig".to_string())),
            want: Some("libclang-dev o equivalente".to_string()),
            hint: None,
        };
    }
    let hint = detect_linux_manager()
        .map(|m| {
            let packages = match m {
                "apt" => "libclang-dev",
                "dnf" => "clang-devel",
                "pacman" => "clang",
                _ => "clang-devel o equivalente",
            };
            linux_install_command(m, packages)
        })
        .or_else(|| Some("instala libclang-dev o equivalente con tu gestor".to_string()));
    Row {
        id: "libclang",
        label,
        mandatory: true,
        status: Status::Missing,
        found: None,
        want: Some("libclang-dev o equivalente".to_string()),
        hint,
    }
}

/// Sonda pkg-config y cabeceras de ALSA (solo Linux; en el resto no aplica y
/// la fila queda correcta por construcción).
fn check_alsa() -> Row {
    let label = "pkg-config y cabeceras de ALSA";
    if !cfg!(target_os = "linux") {
        return Row {
            id: "alsa",
            label,
            mandatory: true,
            status: Status::Ok,
            found: Some("no aplica en este SO".to_string()),
            want: Some("—".to_string()),
            hint: None,
        };
    }
    let has_pkg_config = command_exists("pkg-config") || command_exists("pkgconf");
    let has_alsa = probe_output("pkg-config", &["--exists", "alsa"]).is_some()
        || probe_output("pkgconf", &["--exists", "alsa"]).is_some();
    // `pkg-config --exists` no imprime nada al tener éxito: basta con que el
    // proceso exista… pero `probe_output` devuelve `None` con salida vacía.
    // Se reevalúa por código de salida lanzando de nuevo de forma barata.
    let has_alsa = has_alsa || alsa_present_by_status();
    if has_pkg_config && has_alsa {
        return Row {
            id: "alsa",
            label,
            mandatory: true,
            status: Status::Ok,
            found: Some("pkg-config + alsa".to_string()),
            want: Some("pkg-config, libasound2-dev o equivalentes".to_string()),
            hint: None,
        };
    }
    let hint = detect_linux_manager()
        .map(|m| {
            let packages = match m {
                "apt" => "pkg-config libasound2-dev",
                "dnf" => "pkgconf alsa-lib-devel",
                "pacman" => "pkgconf alsa-lib",
                _ => "pkgconf alsa-devel o equivalentes",
            };
            linux_install_command(m, packages)
        })
        .or_else(|| Some("instala pkg-config y las cabeceras de ALSA".to_string()));
    Row {
        id: "alsa",
        label,
        mandatory: true,
        status: Status::Missing,
        found: None,
        want: Some("pkg-config, libasound2-dev o equivalentes".to_string()),
        hint,
    }
}

/// `true` si `pkg-config --exists alsa` (o `pkgconf`) sale con éxito,
/// aunque no imprima nada.
fn alsa_present_by_status() -> bool {
    for prog in ["pkg-config", "pkgconf"] {
        if std::process::Command::new(prog)
            .args(["--exists", "alsa"])
            .output()
            .is_ok_and(|o| o.status.success())
        {
            return true;
        }
    }
    false
}

/// Sonda de la toolchain del motor TTS según el SO, con los pines de MSYS2
/// en Windows.
fn check_engine_toolchain(pins: &Pins) -> Row {
    let label = "Toolchain del motor TTS";
    if cfg!(windows) {
        let pacman = msys2_root().join("usr").join("bin").join("pacman.exe");
        if !pacman.is_file() {
            return Row {
                id: "engine-toolchain",
                label,
                mandatory: true,
                status: Status::Missing,
                found: None,
                want: Some(format!(
                    "MSYS2 UCRT64 (gcc {}, OpenBLAS {}, make {})",
                    pins.msys2_gcc, pins.msys2_openblas, pins.msys2_make
                )),
                hint: Some(format!(
                    "instala MSYS2 (base {}) y ejecuta en su shell: pacman -S mingw-w64-ucrt-x86_64-gcc mingw-w64-ucrt-x86_64-openblas mingw-w64-ucrt-x86_64-make",
                    pins.msys2_base
                )),
            };
        }
        let wants = [
            ("mingw-w64-ucrt-x86_64-gcc", pins.msys2_gcc.as_str()),
            (
                "mingw-w64-ucrt-x86_64-openblas",
                pins.msys2_openblas.as_str(),
            ),
            ("mingw-w64-ucrt-x86_64-make", pins.msys2_make.as_str()),
        ];
        let mut stale = Vec::new();
        let mut details = Vec::new();
        for (package, pinned) in wants {
            match msys2_package_version(&pacman, package) {
                Some(v) if pin_covers(&v, pinned) => details.push(format!("{package} {v}")),
                Some(v) => stale.push(format!("{package}: instalado {v}, fijado {pinned}")),
                None => stale.push(format!("{package}: no instalado (fijado {pinned})")),
            }
        }
        if stale.is_empty() {
            return Row {
                id: "engine-toolchain",
                label,
                mandatory: true,
                status: Status::Ok,
                found: Some(details.join(", ")),
                want: Some("MSYS2 UCRT64 con los pines de pins.json".to_string()),
                hint: None,
            };
        }
        return Row {
            id: "engine-toolchain",
            label,
            mandatory: true,
            status: Status::Mismatch,
            found: Some(stale.join("; ")),
            want: Some(format!(
                "gcc {}, OpenBLAS {}, make {}",
                pins.msys2_gcc, pins.msys2_openblas, pins.msys2_make
            )),
            hint: Some(
                "en la shell de MSYS2 UCRT64: pacman -S mingw-w64-ucrt-x86_64-gcc mingw-w64-ucrt-x86_64-openblas mingw-w64-ucrt-x86_64-make".to_string(),
            ),
        };
    }
    if cfg!(target_os = "macos") {
        let make = probe_output("make", &["--version"]).and_then(|o| version_token(&o));
        let clang = probe_output("clang", &["--version"]).and_then(|o| version_token(&o));
        if make.is_some() && clang.is_some() {
            return Row {
                id: "engine-toolchain",
                label,
                mandatory: true,
                status: Status::Ok,
                found: Some(format!(
                    "make {}, clang {} + Accelerate",
                    make.unwrap_or_default(),
                    clang.unwrap_or_default()
                )),
                want: Some("make, clang y Accelerate (Xcode CLT)".to_string()),
                hint: None,
            };
        }
        return Row {
            id: "engine-toolchain",
            label,
            mandatory: true,
            status: Status::Missing,
            found: None,
            want: Some("make, clang y Accelerate (Xcode CLT)".to_string()),
            hint: Some("xcode-select --install".to_string()),
        };
    }
    // Linux: make, gcc y OpenBLAS.
    let make = probe_output("make", &["--version"]).and_then(|o| version_token(&o));
    let gcc = probe_output("gcc", &["--version"]).and_then(|o| version_token(&o));
    let openblas = probe_first(
        &["ldconfig", "/sbin/ldconfig", "/usr/sbin/ldconfig"],
        &["-p"],
    )
    .is_some_and(|o| o.lines().any(|l| l.contains("openblas")));
    if make.is_some() && gcc.is_some() && openblas {
        return Row {
            id: "engine-toolchain",
            label,
            mandatory: true,
            status: Status::Ok,
            found: Some("make + gcc + OpenBLAS".to_string()),
            want: Some("make, gcc y OpenBLAS".to_string()),
            hint: None,
        };
    }
    let hint = detect_linux_manager()
        .map(|m| {
            let packages = match m {
                "apt" => "make gcc libopenblas-dev",
                "dnf" => "make gcc openblas-devel",
                "pacman" => "make gcc openblas",
                _ => "make gcc openblas-devel o equivalentes",
            };
            linux_install_command(m, packages)
        })
        .or_else(|| Some("instala make, gcc y OpenBLAS con tu gestor".to_string()));
    Row {
        id: "engine-toolchain",
        label,
        mandatory: true,
        status: Status::Missing,
        found: None,
        want: Some("make, gcc y OpenBLAS".to_string()),
        hint,
    }
}

/// Sonda ONNX Runtime: `ort-bundle/` fresco para el pin (marcador más
/// librería del SO). Solo lectura: descargarlo es tarea de `bootstrap`.
fn check_onnx(root: &Path, pins: &Pins) -> Row {
    let label = "ONNX Runtime (versión fijada)";
    let want = Some(pins.ort.clone());
    let bundle = root.join("ort-bundle");
    let lib = if cfg!(windows) {
        "onnxruntime.dll"
    } else if cfg!(target_os = "macos") {
        "libonnxruntime.dylib"
    } else {
        "libonnxruntime.so"
    };
    if !bundle.join(lib).is_file() {
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
    let marker = bundle.join(".ort-version");
    if marker.is_file() {
        let pinned = std::fs::read_to_string(&marker).unwrap_or_default();
        if pinned.trim() != pins.ort {
            return Row {
                id: "onnx",
                label,
                mandatory: true,
                status: Status::Mismatch,
                found: Some(format!(
                    "ort-bundle {} (fijado {})",
                    pinned.trim(),
                    pins.ort
                )),
                want,
                hint: Some("cargo xtask bootstrap".to_string()),
            };
        }
    }
    Row {
        id: "onnx",
        label,
        mandatory: true,
        status: Status::Ok,
        found: Some(format!("ort-bundle {}", pins.ort)),
        want,
        hint: None,
    }
}

/// Sonda sccache (opcional): cualquier versión vale; si falta es solo aviso.
fn check_sccache() -> Row {
    let label = "sccache";
    let found = probe_output("sccache", &["--version"]).and_then(|o| version_token(&o));
    if found.is_some() {
        return Row {
            id: "sccache",
            label,
            mandatory: false,
            status: Status::Ok,
            found,
            want: Some("cualquiera".to_string()),
            hint: None,
        };
    }
    Row {
        id: "sccache",
        label,
        mandatory: false,
        status: Status::Missing,
        found: None,
        want: Some("cualquiera".to_string()),
        hint: Some("cargo install sccache o el gestor de tu sistema".to_string()),
    }
}

/// Nombre del binario del motor según la plataforma.
fn engine_bin_name() -> &'static str {
    if cfg!(windows) {
        "qwen_tts.exe"
    } else {
        "qwen_tts"
    }
}

/// ¿Está el motor desactualizado? `None` = al día; `Some(motivo)` = deriva.
/// Pura sobre mtimes ya leídas, así se testea sin disco.
pub(crate) fn engine_drift_status(
    bin_mtime: Option<std::time::SystemTime>,
    newest_source_mtime: Option<std::time::SystemTime>,
) -> Option<String> {
    let Some(bin) = bin_mtime else {
        return Some("motor sin compilar (recompila con build-engine)".to_string());
    };
    match newest_source_mtime {
        Some(newest) if newest > bin => Some(
            "motor desactualizado respecto de sus fuentes (recompila con build-engine)".to_string(),
        ),
        _ => None,
    }
}

/// mtime más reciente entre las fuentes del motor (`.c`, `.h`, `Makefile`);
/// `None` si no hay fuentes.
fn newest_engine_source(root: &Path) -> Option<std::time::SystemTime> {
    let dir = root.join("vendor").join("qwen3-tts");
    let mut newest: Option<std::time::SystemTime> = None;
    let mut stack = vec![dir];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                stack.push(path);
                continue;
            }
            let interesting = path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| matches!(e, "c" | "h" | "cpp" | "hpp"))
                || entry.file_name().to_string_lossy() == "Makefile";
            if !interesting {
                continue;
            }
            if let Ok(meta) = path.metadata() {
                if let Ok(mtime) = meta.modified() {
                    newest = Some(newest.map_or(mtime, |n: std::time::SystemTime| n.max(mtime)));
                }
            }
        }
    }
    newest
}

/// `true` si el motor está compilado y al día (convergencia de `bootstrap`).
pub(crate) fn engine_is_fresh(root: &Path) -> bool {
    probe_engine_drift(root).is_none()
}

/// Deriva del motor: binario ausente o más viejo que sus fuentes.
fn probe_engine_drift(root: &Path) -> Option<String> {
    let bin = root
        .join("vendor")
        .join("qwen3-tts")
        .join(engine_bin_name());
    let bin_mtime = bin
        .metadata()
        .ok()
        .filter(|m| m.is_file())
        .and_then(|m| m.modified().ok());
    if bin_mtime.is_none() {
        return Some(
            "motor sin compilar (ejecuta `cargo xtask bootstrap` o `build-engine`)".to_string(),
        );
    }
    engine_drift_status(bin_mtime, newest_engine_source(root)).map(|_| {
        "motor desactualizado respecto de sus fuentes (ejecuta `cargo xtask bootstrap` o `build-engine`)".to_string()
    })
}

/// Deriva de ONNX: marcador de `ort-bundle/` distinto del pin.
fn probe_onnx_drift(root: &Path, pins: &Pins) -> Option<String> {
    let marker = root.join("ort-bundle").join(".ort-version");
    if !root.join("ort-bundle").is_dir() {
        return Some(format!(
            "ONNX Runtime sin descargar (fijado {}; lo provee `cargo xtask bootstrap`)",
            pins.ort
        ));
    }
    if marker.is_file() {
        let pinned = std::fs::read_to_string(&marker).unwrap_or_default();
        if pinned.trim() != pins.ort {
            return Some(format!(
                "ONNX Runtime {} distinto del fijado {} (ejecuta `cargo xtask bootstrap`)",
                pinned.trim(),
                pins.ort
            ));
        }
        return None;
    }
    None
}

/// `true` si los modelos ya están provisionados (convergencia de
/// `bootstrap` con `--models`). Solo lectura sobre la caché.
pub(crate) fn models_are_provisioned() -> bool {
    probe_models_drift().is_empty()
}

/// Deriva de modelos: snapshots ausentes y derivados CT2 sin provisionar.
/// Solo lectura sobre la caché.
fn probe_models_drift() -> Vec<String> {
    let mut out = Vec::new();
    let models = avi_shared::paths::models_cache_dir();
    for (name, repo, revision) in avi_shared::paths::MODEL_REVISIONS {
        let snapshot = models
            .join(format!("models--{}", repo.replace('/', "--")))
            .join("snapshots")
            .join(revision);
        if !snapshot.is_dir() {
            out.push(format!(
                "modelo {name} sin provisionar (ejecuta `cargo xtask bootstrap --models`)"
            ));
        }
    }
    for pair in ["es-en", "en-es"] {
        if !avi_shared::paths::is_ct2_provisioned(pair) {
            out.push(format!(
                "derivado CT2 opus-mt-{pair} sin provisionar (ejecuta `cargo xtask bootstrap --models`)"
            ));
        }
    }
    out
}

/// Ejecuta todas las sondas sobre `root` (raíz del repositorio).
pub(crate) fn check(root: &Path, pins: &Pins) -> Report {
    let rows = vec![
        check_rust(pins),
        check_c_compiler(),
        check_cmake(),
        check_libclang(),
        check_alsa(),
        check_engine_toolchain(pins),
        check_onnx(root, pins),
        check_sccache(),
    ];
    debug_assert_eq!(
        rows.iter().map(|r| r.id).collect::<Vec<_>>(),
        ROW_IDS,
        "la tabla cubre cada fila de §10.1 en orden"
    );
    Report {
        rows,
        engine_drift: probe_engine_drift(root),
        onnx_drift: probe_onnx_drift(root, pins),
        models_drift: probe_models_drift(),
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
    }
}

/// Punto de entrada de `xtask doctor`.
pub fn run(json: bool) -> Result<()> {
    let root = std::env::current_dir()?;
    let pins = load_pins(&root)?;
    let report = check(&root, &pins);
    if json {
        print_json(&report, &pins);
    } else {
        print_human(&report);
    }
    if report.ok() {
        Ok(())
    } else {
        anyhow::bail!("faltan requisitos obligatorios (ver arriba el comando exacto para cada uno)")
    }
}

fn print_human(report: &Report) {
    println!("Requisitos del entorno de desarrollo (§10.1):");
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
        .engine_drift
        .iter()
        .chain(report.onnx_drift.iter())
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

fn print_json(report: &Report, pins: &Pins) {
    let rows: Vec<serde_json::Value> = report
        .rows
        .iter()
        .map(|r| {
            serde_json::json!({
                "id": r.id,
                "requisito": r.label,
                "obligatorio": r.mandatory,
                "estado": status_text(&r.status),
                "encontrada": r.found,
                "esperada": r.want,
                "comando": r.hint,
            })
        })
        .collect();
    let out = serde_json::json!({
        "ok": report.ok(),
        "pines": {
            "rust": pins.rust,
            "ort": pins.ort,
            "sccache": pins.sccache,
            "msys2_base": pins.msys2_base,
            "msys2_gcc": pins.msys2_gcc,
            "msys2_openblas": pins.msys2_openblas,
            "msys2_make": pins.msys2_make,
            "ninja": pins.ninja,
        },
        "requisitos": rows,
        "deriva": {
            "motor": report.engine_drift,
            "onnx": report.onnx_drift,
            "modelos": report.models_drift,
        },
    });
    println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pines de juguete (los reales viven en `packaging/pins.json`).
    fn toy_pins() -> Pins {
        Pins {
            rust: "1.96.0".to_string(),
            ort: "1.28.0".to_string(),
            sccache: "0.8.2".to_string(),
            msys2_base: "2026-06-11".to_string(),
            msys2_gcc: "16.2.0".to_string(),
            msys2_openblas: "0.3.34-1".to_string(),
            msys2_make: "4.4.1-5".to_string(),
            ninja: "1.13.2".to_string(),
        }
    }

    /// La tabla cubre cada fila de §10.1 sin duplicados ni filas de más.
    #[test]
    fn table_covers_spec_rows_once() {
        let mut ids = ROW_IDS.to_vec();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), ROW_IDS.len(), "sin duplicados");
        assert_eq!(
            ids,
            [
                "alsa",
                "c-compiler",
                "cmake",
                "engine-toolchain",
                "libclang",
                "onnx",
                "rust",
                "sccache"
            ]
        );
    }

    /// Los pines que consume la tabla son los de `pins.json`, sin réplica:
    /// el JSON de juguete coincide con el canónico en estructura.
    #[test]
    fn pins_match_json_source() {
        let text = include_str!("../../../packaging/pins.json");
        let pins = avi_shared::pins::parse_text(text).expect("pins.json válido");
        assert_eq!(pins.rust, "1.96.0");
        assert_eq!(pins.ort, "1.28.0");
        for field in [
            &pins.sccache,
            &pins.msys2_base,
            &pins.msys2_gcc,
            &pins.msys2_openblas,
            &pins.msys2_make,
            &pins.ninja,
        ] {
            assert!(!field.trim().is_empty());
        }
    }

    /// `check` sobre fixtures produce las 8 filas en orden y `ok()` solo
    /// exige las obligatorias (`sccache` no bloquea).
    #[test]
    fn check_yields_all_rows_in_order() {
        let root = std::env::temp_dir().join(format!("xtask_doctor_{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let report = check(&root, &toy_pins());
        std::fs::remove_dir_all(&root).ok();
        let ids: Vec<&str> = report.rows.iter().map(|r| r.id).collect();
        assert_eq!(ids, ROW_IDS);
        let optional: Vec<&str> = report
            .rows
            .iter()
            .filter(|r| !r.mandatory)
            .map(|r| r.id)
            .collect();
        assert_eq!(optional, ["sccache"]);
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

    #[test]
    fn engine_drift_predicate() {
        use std::time::{Duration, SystemTime};
        let base = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let older = base - Duration::from_secs(60);
        // Sin binario: deriva (falta compilar).
        assert!(engine_drift_status(None, None).is_some());
        assert!(engine_drift_status(None, Some(base)).is_some());
        // Binario más viejo que las fuentes: deriva.
        assert!(engine_drift_status(Some(older), Some(base)).is_some());
        // Al día: sin deriva.
        assert!(engine_drift_status(Some(base), Some(older)).is_none());
        assert!(engine_drift_status(Some(base), None).is_none());
    }

    #[test]
    fn onnx_row_reports_missing_bundle() {
        let root = std::env::temp_dir().join(format!("xtask_doctor_onnx_{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let row = check_onnx(&root, &toy_pins());
        std::fs::remove_dir_all(&root).ok();
        assert_eq!(row.id, "onnx");
        assert_eq!(row.status, Status::Missing);
        assert_eq!(row.want, Some("1.28.0".to_string()));
        assert!(row.hint.unwrap().contains("bootstrap"));
    }

    #[test]
    fn probe_first_falls_back_to_absolute_candidate() {
        // `ldconfig` puede no estar en el PATH aunque exista en `/sbin`: el
        // primer candidato que produce salida gana, sea por nombre o absoluto.
        // `--list` lo acepta el propio arnés libtest en todas las plataformas.
        let missing = "programa-que-no-existe-xtask-doctor";
        assert!(probe_first(&[missing], &["--list"]).is_none());
        let exe = std::env::current_exe().expect("ejecutable de pruebas");
        let found = probe_first(&[missing, &exe.to_string_lossy()], &["--list"]);
        assert!(
            found.is_some(),
            "el candidato absoluto debe probarse tal cual"
        );
    }
}
