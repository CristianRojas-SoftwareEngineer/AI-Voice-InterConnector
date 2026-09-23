use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};
use regex::Regex;
use std::path::{Path, PathBuf};

const GITHUB_REPO: &str = "CristianRojas-SoftwareEngineer/AI-Voice-InterConnector";
const CASK_NAME: &str = "ai-voice-interconnector";

const CASK_TEMPLATE: &str = r#"cask "{cask_name}" do
  version "{cask_version}"
  sha256 "{cask_sha256}"

  url "https://github.com/{repo}/releases/download/v#{version}/ai-voice-interconnector-#{version}-arm64-macos.tar.gz"
  name "AI Voice InterConnector"
  desc "Motor de síntesis de voz (TTS) offline con clonación de voz en español latinoamericano"
  homepage "https://github.com/{repo}"

  livecheck do
    url :url
    strategy :github_latest
  end

  depends_on macos: ">= :ventura"

  binary "ai-voice-interconnector"

  zap trash: [
    "~/Library/Application Support/ai-voice-interconnector",
    "~/.cache/huggingface/hub/models--Qwen--Qwen3-TTS-12Hz-0.6B-CustomVoice",
    "~/.cache/huggingface/hub/models--Qwen--Qwen3-TTS-12Hz-0.6B-Base",
    "~/.cache/huggingface/hub/models--istupakov--parakeet-tdt-0.6b-v3-onnx",
    "~/.cache/huggingface/hub/models--Helsinki-NLP--opus-mt-es-en",
    "~/.cache/huggingface/hub/models--Helsinki-NLP--opus-mt-en-es",
    "~/.cache/huggingface/xet",
  ]

  caveats <<~EOS
    Los modelos de voz (es-mx-latam + en, ~6 GB en total) no vienen incluidos:
    descargalos una sola vez con:
      ai-voice-interconnector setup

    Licencia: GPL-3.0-or-later. La oferta de codigo fuente (GPLv3 seccion 6)
    y las atribuciones de terceros viajan dentro del archivo instalado:
      #{staged_path}/SOURCE-OFFER.md
      #{staged_path}/THIRD-PARTY-LICENSES.md
  EOS
end
"#;

const SOURCE_OFFER_TEMPLATE: &str = r#"# Oferta de código fuente (GPLv3 §6)

**AI Voice InterConnector {version}** se distribuye bajo la licencia
**GPL-3.0-or-later** (ver `LICENSE`). Conforme a la sección 6 de la GPLv3,
este binario va acompañado de una oferta de acceso al código fuente completo
correspondiente a esta versión exacta:

- **Código fuente (tarball del tag):**
  <https://github.com/{repo}/archive/refs/tags/v{version}.tar.gz>
- **Release v{version} (artefactos y notas):**
  <https://github.com/{repo}/releases/tag/v{version}>
- **Repositorio:** <https://github.com/{repo}>

Las atribuciones de las dependencias redistribuidas están en
`THIRD-PARTY-LICENSES.md`, junto a este archivo. Los pesos de los modelos no se
empaquetan en el binario: se descargan con `setup` y conservan sus licencias
(Qwen3-TTS MIT/Apache-2.0, opus-mt CC-BY-4.0).

Si recibiste este binario sin acceso a las URLs anteriores, puedes solicitar
el código fuente abriendo un issue en el repositorio o contactando al
mantenedor del proyecto, Cristián Rojas Arredondo.
"#;

#[derive(Parser)]
#[command(name = "xtask", about = "Tareas de desarrollo")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Genera el Cask de Homebrew
    Cask {
        #[arg(long)]
        tag: String,
        #[arg(long, value_name = "FILE")]
        sums_file: PathBuf,
        #[arg(long, value_name = "FILE")]
        out: PathBuf,
    },
    /// Genera SOURCE-OFFER.md
    SourceOffer {
        #[arg(long)]
        check: bool,
    },
    /// Verifica THIRD-PARTY-LICENSES.md vs Cargo.lock
    Licenses {
        #[arg(long)]
        check: bool,
    },
    /// Corta una release: bump de versión + promoción de la sección [No publicado] del CHANGELOG
    Release {
        #[arg(value_name = "X.Y.Z")]
        version: String,
    },
    /// Verifica la sección del CHANGELOG para la versión actual
    Changelog {
        #[arg(long)]
        check: bool,
    },
    /// Compila el motor TTS nativo (qwen_tts) desde vendor/qwen3-tts
    BuildEngine {
        /// Ejecuta `<bin> --self-test` tras compilar (oráculo de kernels)
        #[arg(long)]
        self_test: bool,
        /// Política SIMD pass-through al Makefile (default "auto")
        #[arg(long)]
        simd: Option<String>,
        /// Paralelismo del build (-jN); default = paralelismo del host
        #[arg(long)]
        jobs: Option<usize>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Cask {
            tag,
            sums_file,
            out,
        } => {
            let sums_text = std::fs::read_to_string(&sums_file)
                .map_err(|e| anyhow!("no se pudo leer {}: {}", sums_file.display(), e))?;
            let cask = render_cask_from_tag(&tag, &sums_text)?;
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&out, cask)?;
            eprintln!("Cask generado: {}", out.display());
        }
        Commands::SourceOffer { check } => {
            let version = get_version()?;
            if check {
                check_source_offer(&version)?;
                println!("SOURCE-OFFER.md en sincronía");
            } else {
                print!("{}", render_source_offer(&version));
            }
        }
        Commands::Licenses { check: _ } => {
            check_licenses_gate()?;
            println!("THIRD-PARTY-LICENSES.md está en sincronía con Cargo.lock");
        }
        Commands::Release { version } => {
            let version = version.trim();
            if !Regex::new(r"^\d+\.\d+\.\d+$").unwrap().is_match(version) {
                anyhow::bail!(
                    "versión inválida '{}': debe ser X.Y.Z (ej. 0.14.0)",
                    version
                );
            }
            // Pre-validación atómica: abortar antes de mutar si las licencias están
            // desincronizadas, si el árbol está sucio o si no hay commits nuevos
            // desde el último tag.
            {
                check_licenses_gate()?;
                let last = last_tag()?;
                let diff_status = std::process::Command::new("git")
                    .args(["diff", "--quiet"])
                    .status()?;
                let diff_cached_status = std::process::Command::new("git")
                    .args(["diff", "--cached", "--quiet"])
                    .status()?;
                if !diff_status.success() || !diff_cached_status.success() {
                    anyhow::bail!(
                        "working tree con cambios pendientes: commitea o stashea antes de release (git status debe estar limpio)"
                    );
                }
                let log_out = std::process::Command::new("git")
                    .args(["log", &format!("v{}..HEAD", last), "--oneline"])
                    .output()?;
                if !log_out.status.success() {
                    anyhow::bail!("no se pudo verificar el rango v{}..HEAD", last);
                }
                let log_text = String::from_utf8(log_out.stdout)?;
                if log_text.trim().is_empty() {
                    anyhow::bail!(
                        "no se encontraron commits desde el tag v{} — el rango está vacío; commitea cambios antes de release",
                        last
                    );
                }
            }
            bump_version(version)?;
            promote_changelog(version)?;

            // Post-comprobaciones: las mismas puertas que CI ejecuta sobre el tag,
            // corridas antes de crearlo. Los archivos ya están mutados (sin rollback).
            let post_check = |e: anyhow::Error| {
                anyhow!(
                    "los archivos del release ya fueron modificados, pero la verificación \
                     falló: corrige a mano o revierte con `git checkout .` — {}",
                    e
                )
            };
            check_source_offer(version).map_err(post_check)?;
            let changelog_text = std::fs::read_to_string("CHANGELOG.md")?;
            validate_changelog_text(&changelog_text, version).map_err(post_check)?;

            println!("Release {} preparado y verificado:", version);
            println!("  - src/main.rs (VERSION)");
            println!("  - Cargo.toml (package.version)");
            println!("  - Cargo.lock (ai-voice-interconnector)");
            println!("  - tests/golden/cli_version.json");
            println!("  - SOURCE-OFFER.md (oferta GPLv3 §6 versionada)");
            println!("  - CHANGELOG.md (sección promovida desde [No publicado] + ToC + enlace)");
            println!("Comprobaciones: licencias, SOURCE-OFFER.md y CHANGELOG.md en sincronía");
            println!("Revisa el diff, commitea con conventional-commits y crea el tag v{}", version);
        }
        Commands::Changelog { check } => {
            if check {
                check_changelog()?;
                println!("CHANGELOG.md en sincronía con la versión actual");
            } else {
                println!("Usa --check para verificar la sección del CHANGELOG");
            }
        }
        Commands::BuildEngine {
            self_test,
            simd,
            jobs,
        } => {
            build_engine(self_test, simd, jobs)?;
        }
    }
    Ok(())
}

/// Directorio del motor TTS vendorizado.
fn engine_dir() -> PathBuf {
    Path::new("vendor").join("qwen3-tts")
}

/// Nombre del binario del motor según plataforma (.exe en Windows).
fn engine_bin_name() -> &'static str {
    if cfg!(windows) {
        "qwen_tts.exe"
    } else {
        "qwen_tts"
    }
}

/// Programa `make` según plataforma: mingw32-make (Windows/MinGW) o make (Unix).
fn make_program() -> &'static str {
    if cfg!(windows) {
        "mingw32-make"
    } else {
        "make"
    }
}

/// Augmenta el entorno de un `Command` con el toolchain MSYS2 UCRT64 (Windows):
/// PATH con `<MSYS2>\ucrt64\bin` + `<MSYS2>\usr\bin` (cygpath) y MSYSTEM=UCRT64.
/// La raíz se lee de `MSYS2_ROOT` (default `C:\msys64`). No lanza un login-shell:
/// invoca mingw32-make directo con el env corregido (robusto y testeable).
#[cfg(windows)]
fn augment_windows_env(cmd: &mut std::process::Command) -> Result<()> {
    let root = std::env::var("MSYS2_ROOT").unwrap_or_else(|_| r"C:\msys64".to_string());
    let root = Path::new(&root);
    let ucrt_bin = root.join("ucrt64").join("bin");
    let usr_bin = root.join("usr").join("bin");
    if !ucrt_bin.is_dir() {
        anyhow::bail!(
            "no se encontró el toolchain MSYS2 UCRT64 en {}: instala MSYS2 con el grupo \
             mingw-w64-ucrt-x86_64-toolchain (o define MSYS2_ROOT apuntando a tu raíz de MSYS2)",
            ucrt_bin.display()
        );
    }
    // Prepone ucrt64/bin (gcc/mingw32-make) y usr/bin (cygpath) al PATH heredado.
    let current = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![ucrt_bin, usr_bin];
    paths.extend(std::env::split_paths(&current));
    let new_path = std::env::join_paths(paths)
        .map_err(|e| anyhow!("no se pudo construir el PATH de MSYS2: {}", e))?;
    cmd.env("PATH", new_path);
    cmd.env("MSYSTEM", "UCRT64");
    Ok(())
}

/// Compila el motor TTS (`make blas`) desde vendor/qwen3-tts, ocultando el
/// mecanismo por plataforma: Unix invoca `make` con el entorno heredado; Windows
/// invoca `mingw32-make` con el entorno MSYS2 UCRT64 augmentado. Verifica que el
/// binario se generó y, con --self-test, ejecuta el oráculo de kernels.
fn build_engine(self_test: bool, simd: Option<String>, jobs: Option<usize>) -> Result<()> {
    let dir = engine_dir();
    let simd = simd.unwrap_or_else(|| "auto".to_string());
    let jobs = jobs.unwrap_or_else(|| {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
    });

    let mut cmd = std::process::Command::new(make_program());
    cmd.arg("-C")
        .arg(&dir)
        .arg("blas")
        .arg(format!("SIMD={}", simd))
        .arg(format!("-j{}", jobs));
    #[cfg(windows)]
    augment_windows_env(&mut cmd)?;

    eprintln!(
        "Compilando motor TTS: {} -C {} blas SIMD={} -j{}",
        make_program(),
        dir.display(),
        simd,
        jobs
    );
    let status = cmd.status().map_err(|e| {
        anyhow!(
            "no se pudo invocar {}: {} (¿toolchain de compilación instalado?)",
            make_program(),
            e
        )
    })?;
    if !status.success() {
        anyhow::bail!(
            "la compilación del motor TTS falló (exit {:?})",
            status.code()
        );
    }

    let bin = dir.join(engine_bin_name());
    if !bin.is_file() {
        anyhow::bail!("el binario del motor no se generó: {}", bin.display());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&bin)?.permissions().mode();
        if mode & 0o111 == 0 {
            anyhow::bail!("el binario del motor no es ejecutable: {}", bin.display());
        }
    }

    if self_test {
        let mut test_cmd = std::process::Command::new(&bin);
        test_cmd.arg("--self-test");
        // El .exe es -static, pero augmentar el PATH es barato y evita sorpresas
        // si alguna DLL del sistema MSYS2 fuese necesaria.
        #[cfg(windows)]
        augment_windows_env(&mut test_cmd)?;
        let st = test_cmd
            .status()
            .map_err(|e| anyhow!("no se pudo ejecutar {} --self-test: {}", bin.display(), e))?;
        if !st.success() {
            anyhow::bail!("el --self-test del motor falló (exit {:?})", st.code());
        }
    }

    println!("Motor TTS compilado: {}", bin.display());
    Ok(())
}

fn get_version() -> Result<String> {
    // Rust: Cargo.toml
    let cargo = Path::new("Cargo.toml");
    if cargo.is_file() {
        let text = std::fs::read_to_string(cargo)?;
        if let Some(m) = Regex::new(r#"\[package\][^\[]*?version\s*=\s*"([^"]+)""#)
            .unwrap()
            .captures(&text)
        {
            return Ok(m[1].to_string());
        }
    }
    // Rust: src/main.rs
    let main_rs = Path::new("src/main.rs");
    if main_rs.is_file() {
        let text = std::fs::read_to_string(main_rs)?;
        if let Some(m) = Regex::new(r#"const VERSION:\s*&str\s*=\s*"([^"]+)""#)
            .unwrap()
            .captures(&text)
        {
            return Ok(m[1].to_string());
        }
    }
    Err(anyhow!("No se pudo determinar la versión del proyecto"))
}

fn render_source_offer(version: &str) -> String {
    SOURCE_OFFER_TEMPLATE
        .replace("{version}", version)
        .replace("{repo}", GITHUB_REPO)
}

/// Compara `current` (contenido vigente de SOURCE-OFFER.md) contra `rendered`
/// (lo que `render_source_offer` produciría para la versión activa), normalizando
/// CRLF vs LF (git autocrlf, PowerShell). Pura: sin E/S, así se puede testear sin
/// fixtures de archivo. `Ok(())` si coinciden; si no, un mensaje accionable con
/// diff mínimo.
fn diff_source_offer(current: &str, rendered: &str) -> Result<(), String> {
    let norm_current = current.replace("\r\n", "\n");
    let norm_rendered = rendered.replace("\r\n", "\n");
    if norm_current == norm_rendered {
        return Ok(());
    }
    let mut msg = String::from(
        "SOURCE-OFFER.md desincronizado: regenera con `cargo run -p xtask -- source-offer > SOURCE-OFFER.md`\n",
    );
    for line in diff_lines(&norm_current, &norm_rendered) {
        msg.push_str(&line);
        msg.push('\n');
    }
    Err(msg)
}

/// Gate de `SOURCE-OFFER.md`: verifica que esté en sincronía con el renderizado
/// para `version`. Envoltura de E/S sobre `diff_source_offer` (pura, testeable).
fn check_source_offer(version: &str) -> Result<()> {
    let rendered = render_source_offer(version);
    let dest = Path::new("SOURCE-OFFER.md");
    if !dest.is_file() {
        anyhow::bail!("SOURCE-OFFER.md no existe; esperado:\n{}", rendered);
    }
    let current = std::fs::read_to_string(dest)?;
    diff_source_offer(&current, &rendered).map_err(|msg| anyhow!(msg))
}

/// Reemplaza la versión en un archivo usando el regex con dos grupos de captura.
fn bump_in_file(path: &Path, pattern: &str, version: &str) -> Result<()> {
    let text = std::fs::read_to_string(path)?;
    let re = Regex::new(pattern)?;
    if !re.is_match(&text) {
        anyhow::bail!("no se encontró la versión en {}", path.display());
    }
    let result = re.replace_all(&text, |caps: &regex::Captures| {
        format!("{}{}{}", &caps[1], version, &caps[3])
    });
    std::fs::write(path, result.as_ref())?;
    Ok(())
}

fn bump_version(version: &str) -> Result<()> {
    // src/main.rs: const VERSION: &str = "0.13.0";
    bump_in_file(
        Path::new("src/main.rs"),
        r#"(const VERSION:\s*&str\s*=\s*")([^"]+)(")"#,
        version,
    )?;

    // Cargo.toml: version = "0.13.0" dentro de [package] con name
    bump_in_file(
        Path::new("Cargo.toml"),
        r#"(name\s*=\s*"ai-voice-interconnector"\s*\n[^\[]*?version\s*=\s*")([^"]+)(")"#,
        version,
    )?;

    // Cargo.lock: name = "ai-voice-interconnector"\nversion = "0.13.0"
    bump_in_file(
        Path::new("Cargo.lock"),
        r#"(name\s*=\s*"ai-voice-interconnector"\s*\nversion\s*=\s*")([^"]+)(")"#,
        version,
    )?;

    // tests/golden/cli_version.json: "version": "0.13.0"
    bump_in_file(
        Path::new("tests/golden/cli_version.json"),
        r#"("version":\s*")([^"]+)(")"#,
        version,
    )?;

    // SOURCE-OFFER.md: oferta GPLv3 §6 versionada (antes paso manual separado)
    let offer = render_source_offer(version);
    std::fs::write(Path::new("SOURCE-OFFER.md"), offer)?;

    Ok(())
}

/// Resuelve el último tag anotado en el repositorio.
fn last_tag() -> Result<String> {
    let output = std::process::Command::new("git")
        .args(["describe", "--tags", "--abbrev=0"])
        .output()?;
    if !output.status.success() {
        anyhow::bail!("no se pudo determinar el último tag con git describe");
    }
    let tag = String::from_utf8(output.stdout)?;
    Ok(tag.trim().trim_start_matches('v').to_string())
}

/// Promueve la sección curada `## [No publicado]` del CHANGELOG a `## [version]`.
/// Envoltura de E/S: lee el archivo, resuelve el tag previo y la fecha, delega la
/// transformación en `promote_changelog_text` y reescribe el archivo.
fn promote_changelog(version: &str) -> Result<()> {
    let changelog_path = Path::new("CHANGELOG.md");
    let text = std::fs::read_to_string(changelog_path)?;
    let last = last_tag()?;
    let date = today_iso();
    let promoted = promote_changelog_text(&text, version, &last, &date)?;
    std::fs::write(changelog_path, promoted)?;
    Ok(())
}

/// Transformación pura: renombra `## [No publicado]` → `## [version] — date`,
/// actualiza su entrada de ToC y añade la definición de enlace de comparación.
/// Falla ruidosamente si la sección ya fue promovida, si no existe `[No publicado]`
/// o si su cuerpo conserva marcadores `<!-- TODO: curar -->`.
fn promote_changelog_text(text: &str, version: &str, last: &str, date: &str) -> Result<String> {
    // Invariante: no promover dos veces sobre la misma versión. Anclado a inicio
    // de línea: una mención del literal en la prosa no debe contar como cabecera.
    let version_heading = format!("## [{}]", version);
    if find_heading_offset(text, &version_heading).is_some() {
        anyhow::bail!(
            "la sección ## [{}] ya existe en CHANGELOG.md — ¿la promoción ya se aplicó?",
            version
        );
    }

    // Localizar la sección curada a promover (cabecera real, no menciones en prosa).
    let unreleased_heading = "## [No publicado]";
    let heading_pos = find_heading_offset(text, unreleased_heading).ok_or_else(|| {
        anyhow::anyhow!(
            "no se encontró la sección ## [No publicado] en CHANGELOG.md — cura la sección antes del corte"
        )
    })?;

    // Delimitar el cuerpo de [No publicado] hasta la siguiente sección de versión
    // y exigir que esté curado (sin marcadores TODO).
    let after_heading = &text[heading_pos + unreleased_heading.len()..];
    let body_end = after_heading.find("\n## [").unwrap_or(after_heading.len());
    if after_heading[..body_end].contains("<!-- TODO: curar") {
        anyhow::bail!(
            "la sección ## [No publicado] conserva marcadores `<!-- TODO: curar -->`: cúrala antes de promover"
        );
    }

    // 1) Renombrar la cabecera cortando en el offset exacto de la cabecera real
    // (no `replacen`, que sustituiría una mención del literal en la prosa si ésta
    // precediera a la cabecera).
    let new_heading = format!("## [{}] — {}", version, date);
    let mut result = String::with_capacity(text.len() + new_heading.len());
    result.push_str(&text[..heading_pos]);
    result.push_str(&new_heading);
    result.push_str(&text[heading_pos + unreleased_heading.len()..]);

    // 2) Actualizar la línea del ToC.
    let toc_line = "- [No publicado](#no-publicado)";
    if !result.contains(toc_line) {
        anyhow::bail!(
            "no se encontró la entrada de ToC `- [No publicado](#no-publicado)` en CHANGELOG.md"
        );
    }
    let new_toc_line = format!("- [{} — {}](#{})", version, date, slug(version, date));
    result = result.replacen(toc_line, &new_toc_line, 1);

    // 3) Insertar la definición de enlace tras la última definición existente
    // (las definiciones viven al final del archivo, en orden ascendente).
    let link_def = format!(
        "[{}]: https://github.com/{}/compare/v{}...v{}",
        version, GITHUB_REPO, last, version
    );
    result = result.trim_end().to_string();
    result.push('\n');
    result.push_str(&link_def);
    result.push('\n');

    Ok(result)
}

/// Formatea la fecha actual como YYYY-MM-DD usando el comando `date`.
fn today_iso() -> String {
    let output = std::process::Command::new("date").arg("+%Y-%m-%d").output();
    if let Ok(out) = output {
        if out.status.success() {
            if let Ok(s) = String::from_utf8(out.stdout) {
                return s.trim().to_string();
            }
        }
    }
    // Fallback: obtener de git (último commit)
    if let Ok(out) = std::process::Command::new("git")
        .args(["log", "-1", "--format=%ad", "--date=format:%Y-%m-%d"])
        .output()
    {
        if out.status.success() {
            if let Ok(s) = String::from_utf8(out.stdout) {
                return s.trim().to_string();
            }
        }
    }
    "0000-00-00".to_string()
}

/// Genera el ancla de GitHub para la cabecera `## [version] — date`: el slugger
/// descarta corchetes, puntos y la raya, conserva los guiones de la fecha y
/// convierte cada espacio en `-` (de ahí el doble guion).
fn slug(version: &str, date: &str) -> String {
    let v = version.replace('.', "");
    format!("{}--{}", v, date)
}

/// Byte offset de la cabecera `heading` en la primera línea que, tras `trim_start`,
/// empieza con ella. Ancla la búsqueda a inicio de línea para ignorar menciones
/// del literal en la prosa (p. ej. dentro de backticks), que `str::find`/`contains`
/// confundirían con la cabecera real.
fn find_heading_offset(text: &str, heading: &str) -> Option<usize> {
    let mut offset = 0usize;
    for line in text.split_inclusive('\n') {
        let indent = line.len() - line.trim_start().len();
        if line.trim_start().starts_with(heading) {
            return Some(offset + indent);
        }
        offset += line.len();
    }
    None
}

fn check_changelog() -> Result<()> {
    let version = get_version()?;
    let text = std::fs::read_to_string("CHANGELOG.md")?;
    validate_changelog_text(&text, &version)
}

/// Puerta de promoción completa: valida que la sección `[version]` esté cortada
/// entera. Como es el único punto de control del pipeline tags-only, cubre el
/// fallo silencioso de `publish-release` (que extrae las notas de `[version]`).
/// Cada invariante incumplido emite un `bail!` accionable que nombra qué falta.
fn validate_changelog_text(text: &str, version: &str) -> Result<()> {
    // 1) Cabecera de la sección de versión (anclada a inicio de línea).
    let heading = format!("## [{}]", version);
    let heading_pos = find_heading_offset(text, &heading)
        .ok_or_else(|| anyhow!("no se encontró la sección [{}] en CHANGELOG.md", version))?;

    // 2) Entrada de la tabla de contenidos para la versión.
    let toc_prefix = format!("- [{} —", version);
    if !text
        .lines()
        .any(|l| l.trim_start().starts_with(&toc_prefix))
    {
        anyhow::bail!(
            "falta la entrada de la tabla de contenidos para [{}] en CHANGELOG.md (esperado `- [{} — <fecha>](#…)`)",
            version,
            version
        );
    }

    // 3) Definición de enlace de comparación de la versión.
    let link_prefix = format!("[{}]: ", version);
    if !text.lines().any(|l| l.starts_with(&link_prefix)) {
        anyhow::bail!(
            "falta la definición de enlace de comparación para [{}] en CHANGELOG.md (esperado `[{}]: …/compare/…`)",
            version,
            version
        );
    }

    // 4) La sección de la versión no conserva marcadores TODO.
    let after_heading = &text[heading_pos + heading.len()..];
    let section_end = after_heading.find("\n## [").unwrap_or(after_heading.len());
    if after_heading[..section_end].contains("<!-- TODO: curar") {
        anyhow::bail!(
            "la sección [{}] conserva marcadores `<!-- TODO: curar -->`: cúrala antes del corte",
            version
        );
    }

    // 5) No queda una sección `[No publicado]` sin promover. Anclado a inicio de
    // línea: una mención del literal en la prosa (p. ej. dentro de backticks, como
    // esta misma entrada del CHANGELOG) no es una sección residual.
    if find_heading_offset(text, "## [No publicado]").is_some() {
        anyhow::bail!(
            "queda una sección `## [No publicado]` sin promover en CHANGELOG.md: el corte debe promoverla a [{}]",
            version
        );
    }

    // 6) Las anclas del índice coinciden con las que GitHub asigna a cada cabecera.
    validate_changelog_anchors(text)?;

    Ok(())
}

/// Invariante de anclas: cada cabecera `## [X.Y.Z] — fecha` debe tener en la
/// tabla de contenidos la línea exacta `- [X.Y.Z — fecha](#ancla)` con el ancla
/// que GitHub asigna (`slug`), y cada entrada de ToC debe apuntar al ancla de su
/// propia versión y fecha. Solo compara líneas completas, así que las menciones
/// en prosa no cuentan. Independiente de los demás invariantes para poder
/// aplicarla a un CHANGELOG que aún conserva `## [No publicado]`.
fn validate_changelog_anchors(text: &str) -> Result<()> {
    let heading_re = Regex::new(r"(?m)^## \[(\d+\.\d+\.\d+)\] — (\d{4}-\d{2}-\d{2})\s*$").unwrap();
    let toc_re =
        Regex::new(r"(?m)^- \[(\d+\.\d+\.\d+) — (\d{4}-\d{2}-\d{2})\]\(#([^)]+)\)\s*$").unwrap();

    for caps in heading_re.captures_iter(text) {
        let version = &caps[1];
        let date = &caps[2];
        let expected_anchor = slug(version, date);
        let expected_toc_line = format!("- [{} — {}](#{})", version, date, expected_anchor);
        if !text.lines().any(|l| l.trim_start() == expected_toc_line) {
            anyhow::bail!(
                "la tabla de contenidos de CHANGELOG.md no tiene la entrada `{}` para la sección \
                 [{}] — {} (ancla esperada `#{}`, la que GitHub asigna a esa cabecera)",
                expected_toc_line,
                version,
                date,
                expected_anchor
            );
        }
    }

    for caps in toc_re.captures_iter(text) {
        let version = &caps[1];
        let date = &caps[2];
        let anchor = &caps[3];
        let expected_anchor = slug(version, date);
        if anchor != expected_anchor.as_str() {
            anyhow::bail!(
                "la entrada de la tabla de contenidos para [{}] — {} usa el ancla `#{}`, pero \
                 GitHub asignará `#{}` a la cabecera `## [{}] — {}`: corrige el enlace del índice",
                version,
                date,
                anchor,
                expected_anchor,
                version,
                date
            );
        }
    }

    Ok(())
}

fn parse_macos_sha256(sums_text: &str, version: &str) -> Result<String> {
    let pattern = format!(
        r"^([0-9a-fA-F]{{64}})\s+\S*ai-voice-interconnector-{}-arm64-macos\.tar\.gz\s*$",
        regex::escape(version)
    );
    let re = Regex::new(&pattern).unwrap();
    let mut matches = Vec::new();
    for line in sums_text.lines() {
        if let Some(caps) = re.captures(line) {
            matches.push(caps[1].to_lowercase());
        }
    }
    match matches.len() {
        0 => Err(anyhow!(
            "No se encontró el hash del tar.gz arm64 de macOS de la versión {} en SHA256SUMS.txt",
            version
        )),
        1 => Ok(matches[0].clone()),
        _ => Err(anyhow!(
            "Múltiples líneas coinciden con el tar.gz arm64 de macOS de la versión {} en SHA256SUMS.txt",
            version
        )),
    }
}

fn render_cask(version: &str, sha256: &str) -> String {
    CASK_TEMPLATE
        .replace("{cask_name}", CASK_NAME)
        .replace("{cask_version}", version)
        .replace("{cask_sha256}", sha256)
        .replace("{repo}", GITHUB_REPO)
}

fn render_cask_from_tag(circle_tag: &str, sums_text: &str) -> Result<String> {
    let version = circle_tag.trim_start_matches('v');
    let sha256 = parse_macos_sha256(sums_text, version)?;
    Ok(render_cask(version, &sha256))
}

fn diff_lines(a: &str, b: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in diff::lines(a, b) {
        match line {
            diff::Result::Left(l) => out.push(format!("-{}", l)),
            diff::Result::Right(l) => out.push(format!("+{}", l)),
            diff::Result::Both => {}
        }
    }
    out
}

// Minimal diff helper inline to avoid extra dep
mod diff {
    pub enum Result<'a> {
        Left(&'a str),
        Right(&'a str),
        Both,
    }
    pub fn lines<'a>(a: &'a str, b: &'a str) -> Vec<Result<'a>> {
        let a_lines: Vec<&str> = a.lines().collect();
        let b_lines: Vec<&str> = b.lines().collect();
        let mut res = Vec::new();
        let mut i = 0;
        let mut j = 0;
        while i < a_lines.len() && j < b_lines.len() {
            if a_lines[i] == b_lines[j] {
                res.push(Result::Both);
                i += 1;
                j += 1;
            } else {
                // Simple: treat as left then right
                res.push(Result::Left(a_lines[i]));
                res.push(Result::Right(b_lines[j]));
                i += 1;
                j += 1;
            }
        }
        while i < a_lines.len() {
            res.push(Result::Left(a_lines[i]));
            i += 1;
        }
        while j < b_lines.len() {
            res.push(Result::Right(b_lines[j]));
            j += 1;
        }
        res
    }
}

fn normalize(name: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for c in name.chars() {
        if c == '-' || c == '_' || c == '.' {
            if !last_dash {
                out.push('-');
                last_dash = true;
            }
        } else {
            out.push(c.to_ascii_lowercase());
            last_dash = false;
        }
    }
    out
}

fn cargo_lock_packages(text: &str) -> std::collections::HashSet<String> {
    let re = Regex::new(r#"\[\[package\]\]\s+name\s*=\s*"([^"]+)""#).unwrap();
    re.captures_iter(text).map(|c| normalize(&c[1])).collect()
}

fn licenses_doc_packages(text: &str) -> std::collections::HashSet<String> {
    let header = "| Paquete | Versión |";
    let lines = text.lines();
    let mut start = None;
    for (idx, line) in lines.clone().enumerate() {
        if line.starts_with(header) {
            start = Some(idx);
            break;
        }
    }
    let start = start.expect("No se encontró la tabla de inventario");
    let mut set = std::collections::HashSet::new();
    let re = Regex::new(r"^\|\s*`([^`]+)`\s*\|").unwrap();
    for line in text.lines().skip(start + 2) {
        if let Some(caps) = re.captures(line) {
            set.insert(normalize(&caps[1]));
        } else {
            break;
        }
    }
    set
}

fn check_licenses() -> Result<(Vec<String>, Vec<String>)> {
    let lock_text = std::fs::read_to_string("Cargo.lock")?;
    let doc_text = std::fs::read_to_string("THIRD-PARTY-LICENSES.md")?;
    let lock = cargo_lock_packages(&lock_text);
    let doc = licenses_doc_packages(&doc_text);
    let mut missing: Vec<String> = lock.difference(&doc).cloned().collect();
    let mut extra: Vec<String> = doc.difference(&lock).cloned().collect();
    missing.sort();
    extra.sort();
    Ok((missing, extra))
}

/// Mensaje accionable del gate de licencias a partir de las listas de
/// faltantes/sobrantes ya calculadas por `check_licenses`. Pura: sin E/S, así
/// se puede testear sin fixtures de archivo. `None` si está en sincronía.
fn format_licenses_gate_message(missing: &[String], extra: &[String]) -> Option<String> {
    if missing.is_empty() && extra.is_empty() {
        return None;
    }
    let mut msg = String::new();
    if !missing.is_empty() {
        msg.push_str(
            "Crates del lock SIN fila en THIRD-PARTY-LICENSES.md (atribución faltante):\n",
        );
        for n in missing {
            msg.push_str(&format!("  + {}\n", n));
        }
    }
    if !extra.is_empty() {
        msg.push_str("Filas de THIRD-PARTY-LICENSES.md sin crate en el lock (obsoletas):\n");
        for n in extra {
            msg.push_str(&format!("  - {}\n", n));
        }
    }
    msg.push_str("\nRegenera el inventario (cargo metadata).");
    Some(msg)
}

/// Gate de licencias: falla con un mensaje accionable si `THIRD-PARTY-LICENSES.md`
/// está desincronizado de `Cargo.lock` (crates faltantes o filas obsoletas).
fn check_licenses_gate() -> Result<()> {
    let (missing, extra) = check_licenses()?;
    match format_licenses_gate_message(&missing, &extra) {
        None => Ok(()),
        Some(msg) => anyhow::bail!(msg),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_sums() -> String {
        let macos = "a".repeat(64);
        format!(
            "{}  ai-voice-interconnector-1.2.3-x86_64-windows.zip\n{}  ai-voice-interconnector-1.2.3-x86_64-linux.tar.gz\n{}  ai-voice-interconnector-1.2.3-arm64-linux.tar.gz\n{}  ai-voice-interconnector-1.2.3-arm64-macos.tar.gz\n",
            "b".repeat(64),
            "c".repeat(64),
            "d".repeat(64),
            macos
        )
    }

    #[test]
    fn test_parse_macos_sha256_extracts() {
        let sums = sample_sums();
        let h = parse_macos_sha256(&sums, "1.2.3").unwrap();
        assert_eq!(h, "a".repeat(64));
    }

    #[test]
    fn test_parse_missing_raises() {
        let sums = sample_sums();
        assert!(parse_macos_sha256(&sums, "9.9.9").is_err());
    }

    #[test]
    fn test_render_cask_stanzas() {
        let c = render_cask("1.2.3", &"a".repeat(64));
        assert!(c.contains(r#"version "1.2.3""#));
        assert!(c.contains(&format!(r#"sha256 "{}""#, "a".repeat(64))));
        assert!(c.contains("ai-voice-interconnector-#{version}-arm64-macos.tar.gz"));
        assert!(c.contains(r#"cask "ai-voice-interconnector" do"#));
        assert!(c.contains(r#"binary "ai-voice-interconnector""#));
        assert!(!c.contains("\n  app "));
        assert!(c.contains("releases/download/v#{version}/"));
        assert!(c.contains("zap trash:"));
        assert!(c.contains("models--Qwen--"));
        assert!(c.contains("models--istupakov--"));
        assert!(c.contains("models--Helsinki-NLP--"));
        assert!(!c.contains("Chatterbox"));
        assert!(!c.contains("ResembleAI"));
        assert!(c.contains("GPL-3.0-or-later"));
        assert!(c.contains(r#"depends_on macos: ">= :ventura""#));
        assert!(c.contains("síntesis"));
    }

    #[test]
    fn test_cask_zap_no_chatterbox_sino_qwen_parakeet_opusmt() {
        let c = render_cask("9.9.9", &"b".repeat(64));
        assert!(c.contains("models--Qwen--Qwen3-TTS-12Hz-0.6B-CustomVoice"));
        assert!(c.contains("models--Qwen--Qwen3-TTS-12Hz-0.6B-Base"));
        assert!(c.contains("models--istupakov--parakeet-tdt-0.6b-v3-onnx"));
        assert!(c.contains("models--Helsinki-NLP--opus-mt-es-en"));
        assert!(c.contains("models--Helsinki-NLP--opus-mt-en-es"));
        assert!(c.contains("~/.cache/huggingface/xet"));
        assert!(!c.contains("ResembleAI"));
        assert!(!c.contains("chatterbox"));
        assert!(!c.contains("Chatterbox"));
    }

    /// Texto de `.circleci/config.yml`, localizado desde la raíz o desde el crate.
    fn leer_config_ci() -> String {
        let candidates = [
            ".circleci/config.yml",
            "../../.circleci/config.yml",
            "C:/Users/Cristian/Desktop/Proyectos/Voices/AI-Voice-InterConnector/.circleci/config.yml",
        ];
        for p in candidates {
            if let Ok(t) = std::fs::read_to_string(p) {
                return t;
            }
        }
        // Fallback via CARGO_MANIFEST_DIR
        let m = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.circleci/config.yml");
        std::fs::read_to_string(&m).expect("no se pudo leer .circleci/config.yml")
    }

    #[test]
    fn test_pipeline_heterogeneo_y_sccache_incondicional() {
        let cfg = leer_config_ci();
        // Modelo vigente (post remediación de caché): test-linux, test-windows, test-macos,
        // coverage y build-* usan cargo_restore_caches (registry + target-v3) y sccache
        // autoconsistente por variante (cada job pesado restaura y guarda su propio blob).
        // Los jobs pequeños (validate-licenses/validate-changelog/publish-metadata) usan
        // cargo_restore_registry (solo registry + sccache restore-only), por lo que ambos
        // comandos coexisten.
        assert!(
            cfg.contains("cargo_restore_caches") && cfg.contains("cargo_restore_registry"),
            "debe existir ambos comandos cargo_restore_caches y cargo_restore_registry para modelo heterogéneo"
        );
        // sccache se guarda de forma INCONDICIONAL: no existe comando condicional por hit-rate.
        assert!(
            !cfg.contains("sccache_save_cache_conditional"),
            "no debe existir sccache_save_cache_conditional (guardado condicional por hit-rate eliminado)"
        );
        // El guardado vigente es sccache_save_cache con clave rolling por {{ epoch }} y when: always.
        assert!(
            cfg.contains("sccache_save_cache:"),
            "debe existir el comando sccache_save_cache (guardado incondicional)"
        );
        assert!(
            cfg.contains("sccache-v1-{{ arch }}-<< parameters.os >>-<< pipeline.parameters.rust_version >>-<< parameters.variant >>-{{ epoch }}"),
            "sccache_save_cache debe usar clave rolling por epoch segmentada por variante"
        );
        assert!(
            cfg.contains("sccache-v1-{{ arch }}-<< parameters.os >>-<< pipeline.parameters.rust_version >>-<< parameters.variant >>-"),
            "sccache_restore_cache debe usar clave por prefijo segmentada por variante"
        );
        // Secciones scoping por job (delimitadas por siguiente job header para evitar falso-positivo cross-job)
        let linux_section = cfg
            .split("  test-linux:")
            .nth(1)
            .unwrap_or("")
            .split("  test-windows:")
            .next()
            .unwrap_or("");
        assert!(
            linux_section.contains("cargo_restore_caches"),
            "test-linux debe usar cargo_restore_caches (registry + target-v3)"
        );
        assert!(
            linux_section.contains("variant: test"),
            "test-linux debe usar variant: test"
        );
        assert!(
            linux_section.contains("cargo_save_target"),
            "test-linux debe guardar target-v3 (cargo_save_target)"
        );
        assert!(
            linux_section.contains("sccache_restore_cache"),
            "test-linux debe restaurar sccache autoconsistente (variant: test)"
        );
        assert!(
            linux_section.contains("sccache_save_cache"),
            "test-linux debe guardar sccache (sccache_save_cache)"
        );
        let windows_section = cfg
            .split("  test-windows:")
            .nth(1)
            .unwrap_or("")
            .split("  test-macos:")
            .next()
            .unwrap_or("");
        assert!(
            windows_section.contains("cargo_restore_caches"),
            "test-windows debe usar cargo_restore_caches"
        );
        assert!(
            windows_section.contains("os: windows"),
            "test-windows debe usar os: windows"
        );
        assert!(
            windows_section.contains("variant: test"),
            "test-windows debe usar variant: test"
        );
        assert!(
            windows_section.contains("cargo_save_target"),
            "test-windows debe guardar target-v3 (cargo_save_target)"
        );
        assert!(
            windows_section.contains("sccache_restore_cache"),
            "test-windows debe usar sccache"
        );
        assert!(
            windows_section.contains("sccache_save_cache"),
            "test-windows debe guardar sccache (sccache_save_cache)"
        );
        // coverage debe usar cargo_restore_caches con os: linux y variant: cov y guardar target-v3
        let coverage_section = cfg
            .split("  coverage:")
            .nth(1)
            .unwrap_or("")
            .split("  validate-licenses")
            .next()
            .unwrap_or("");
        assert!(
            coverage_section.contains("cargo_restore_caches"),
            "coverage debe usar cargo_restore_caches"
        );
        assert!(
            coverage_section.contains("os: linux"),
            "coverage debe usar os: linux"
        );
        assert!(
            coverage_section.contains("variant: cov"),
            "coverage debe usar variant: cov"
        );
        assert!(
            coverage_section.contains("cargo_save_target"),
            "coverage debe guardar target-v3 (cargo_save_target)"
        );
        assert!(
            coverage_section.contains("sccache_save_cache"),
            "coverage debe guardar sccache (sccache_save_cache)"
        );
        // test-macos usa cargo_restore_caches (registry + target-v3) y sccache autoconsistente (variant: test), igual que test-linux
        let macos_section = cfg
            .split("  test-macos:")
            .nth(1)
            .unwrap_or("")
            .split("  coverage:")
            .next()
            .unwrap_or("");
        assert!(
            macos_section.contains("cargo_restore_caches"),
            "test-macos debe usar cargo_restore_caches (registry + target-v3)"
        );
        assert!(
            macos_section.contains("variant: test"),
            "test-macos debe usar variant: test"
        );
        assert!(
            macos_section.contains("sccache_restore_cache"),
            "test-macos debe restaurar sccache autoconsistente (variant: test)"
        );
        assert!(
            macos_section.contains("sccache_save_cache"),
            "test-macos debe guardar sccache (sccache_save_cache)"
        );
        assert!(
            macos_section.contains("cargo_save_target"),
            "test-macos debe guardar target-v3 (cargo_save_target)"
        );
        // build-* deben usar cargo_restore_caches con target-v3 full (heterogéneo con target en build-*).
        // NO deben ejecutar `cargo clean -p ai-voice-interconnector`: sin --release/--profile
        // es un no-op sobre el perfil release (limpia solo target/debug), y aunque no lo fuera
        // el bump de VERSION ya invalida el fingerprint de cargo por sí solo (mtime + -C
        // metadata) y sccache nunca cachea crates --crate-type bin. Ver docs/BUILD.md §4.
        for job in [
            "build-windows-x64",
            "build-linux-x64",
            "build-linux-arm64",
            "build-darwin-arm64",
        ] {
            let header = format!("  {}:", job);
            let section = cfg
                .split(&header)
                .nth(1)
                .unwrap_or("")
                .split("\n  build-")
                .next()
                .unwrap_or("")
                .split("\n  publish-")
                .next()
                .unwrap_or("");
            assert!(
                section.contains("cargo_restore_caches"),
                "{job} debe usar cargo_restore_caches (con target-v3)"
            );
            assert!(
                section.contains("variant: full"),
                "{job} debe usar variant: full"
            );
            assert!(
                section.contains("cargo_save_target"),
                "{job} debe guardar target-v3 (cargo_save_target)"
            );
            assert!(
                section.contains("sccache_save_cache"),
                "{job} debe guardar sccache (sccache_save_cache)"
            );
            assert!(
                !section.contains("cargo clean -p"),
                "{job} no debe ejecutar cargo clean -p: es un no-op sobre target/release sin --release"
            );
        }
    }

    #[test]
    fn test_clave_target_v3_con_identidad_de_vendor_cmake() {
        let cfg = leer_config_ci();
        // El parche local de cmake se fingerprintea por mtime: el checkout lo marca
        // Dirty y arrastra la cadena nativa. La clave exacta de target-v3 lleva el
        // tree hash git del parche y NO tiene fallback, de modo que un acierto
        // garantiza que target/ corresponde al contenido actual y fijar el mtime es
        // seguro. Reintroducir un fallback fijaría mtimes sobre snapshots ajenos.
        let restore_section = cfg
            .split("  cargo_restore_caches:")
            .nth(1)
            .unwrap_or("")
            .split("\n  cargo_save_registry:")
            .next()
            .unwrap_or("");
        let save_section = cfg
            .split("  cargo_save_target:")
            .nth(1)
            .unwrap_or("")
            .split("\n  cargo_restore_registry:")
            .next()
            .unwrap_or("");

        let hash_cmd = "git rev-parse HEAD:vendor/cmake-0.1.58";
        assert!(
            restore_section.contains(hash_cmd),
            "cargo_restore_caches debe calcular el tree hash git de vendor/cmake-0.1.58"
        );
        assert!(
            restore_section.contains("set -euo pipefail"),
            "el cálculo del tree hash debe fallar ruidosamente (set -euo pipefail)"
        );
        assert!(
            restore_section.contains("^[0-9a-f]{40}$"),
            "cargo_restore_caches debe validar que el tree hash tiene 40 caracteres hexadecimales"
        );

        let restore_keys: Vec<&str> = restore_section
            .lines()
            .map(str::trim)
            .filter_map(|l| l.strip_prefix("- target-v3-"))
            .collect();
        assert_eq!(
            restore_keys.len(),
            1,
            "target-v3 debe restaurarse con una única clave exacta, sin fallback por prefijo"
        );
        assert!(
            restore_keys[0].contains(r#"checksum ".vendor-cmake.tree""#),
            "la clave de target-v3 debe incluir el tree hash de vendor/cmake-0.1.58"
        );
        let save_key = save_section
            .lines()
            .map(str::trim)
            .find_map(|l| l.strip_prefix("key: target-v3-"))
            .unwrap_or("");
        assert_eq!(
            restore_keys[0], save_key,
            "las claves de restauración y guardado de target-v3 deben ser idénticas"
        );

        let pos_hash = restore_section.find(hash_cmd).unwrap();
        let pos_restore = restore_section
            .find("- target-v3-")
            .expect("cargo_restore_caches debe restaurar target-v3");
        let pos_touch = restore_section
            .find("touch -t 200001010000")
            .expect("cargo_restore_caches debe fijar el mtime del parche");
        assert!(
            pos_hash < pos_restore,
            "el tree hash debe calcularse antes de restaurar target-v3"
        );
        assert!(
            pos_restore < pos_touch,
            "el mtime del parche debe fijarse después de restaurar target-v3"
        );

        for residuo in ["sort -z", "sha256_hex", "target/.vendor-cmake.sha256"] {
            assert!(
                !cfg.contains(residuo),
                "la config no debe contener `{residuo}` (mecanismo de sello retirado)"
            );
        }
    }

    /// Regresión EPIPE (v0.20.9, build-linux-x64): un productor Rust seguido
    /// de un consumidor que sale antes de leer toda la entrada (`grep -q`,
    /// `head`) bajo `pipefail` es una condición de carrera — si el binario aún
    /// tiene líneas por escribir cuando el consumidor cierra la tubería, la
    /// escritura devuelve EPIPE y `println!` hace panic (Rust ignora SIGPIPE
    /// por defecto). El patrón correcto es capturar la salida completa primero
    /// (variable o `$(...)`) y solo entonces aplicar `grep -q`/`head` sobre esa
    /// captura ya materializada.
    #[test]
    fn test_sin_tuberia_racy_grep_q_o_head_bajo_pipefail() {
        let cfg = leer_config_ci();
        // El patrón prohibido es "productor que aún puede estar escribiendo |
        // consumidor que sale antes de leer todo". `printf '%s\n' "$var" |
        // grep -q` SÍ es seguro (captura ya materializada, write() atómico) y
        // es justo el patrón de reemplazo; se prohíbe solo el productor
        // original: el binario en ejecución piped directo a `grep -q`.
        assert!(
            !cfg.contains("ai-voice-interconnector voice list | grep -q"),
            "no debe existir `ai-voice-interconnector voice list | grep -q` directo: capturar la salida en variable antes de filtrar (regresión EPIPE v0.20.9)"
        );
        // `find ... | head -nN` tiene el mismo riesgo si `find` tiene más de una
        // coincidencia: `head` cierra la tubería tras la primera línea y `find`
        // puede recibir EPIPE mid-escritura. Nota: `printf '%s\n' "$var" | head`
        // SÍ es seguro (la captura ya está materializada en `$var`, `printf`
        // hace un único write() atómico) y es precisamente el patrón que
        // reemplazó a `find | head`; por eso el test apunta al productor
        // original (`find`), no a `| head` en general.
        assert!(
            !cfg.contains("-type f | head"),
            "no debe existir `find ... | head` directo: materializar la salida (`ort_matches=\"$(find ...)\"`) antes de aplicar head (regresión EPIPE v0.20.9)"
        );
    }

    /// `target-v3` usa clave inmutable (el primer `save_cache` gana): en
    /// build-* (variant: full), guardar con `when: always` persistiría para
    /// siempre un `target/` incompleto si `cargo build --release` falla a
    /// medias (ya ocurrió con linux-x64 en v0.20.9). test-*/coverage sí
    /// conservan `when: always` porque ahí un fallo del job suele ser un test
    /// que falló, no una compilación incompleta.
    #[test]
    fn test_cargo_save_target_on_success_en_build_variant_full() {
        let cfg = leer_config_ci();
        // Cada bloque `cargo_save_target:` con `variant: full` debe traer
        // `when: on_success` en las mismas 3 líneas siguientes.
        let mut vistos_full = 0;
        let lines: Vec<&str> = cfg.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            if line.trim() != "- cargo_save_target:" {
                continue;
            }
            let ventana = lines[i..(i + 5).min(lines.len())].join("\n");
            if ventana.contains("variant: full") {
                vistos_full += 1;
                assert!(
                    ventana.contains("when: on_success"),
                    "cargo_save_target con variant: full debe pasar when: on_success (evita persistir target/ incompleto bajo clave inmutable); bloque:\n{ventana}"
                );
            }
        }
        // Los 4 build-* (windows-x64, linux-x64, linux-arm64, darwin-arm64).
        assert_eq!(
            vistos_full, 4,
            "se esperaban 4 invocaciones de cargo_save_target con variant: full (una por build-*)"
        );
    }

    const BUILD_JOBS: [&str; 4] = [
        "build-windows-x64",
        "build-linux-x64",
        "build-linux-arm64",
        "build-darwin-arm64",
    ];

    /// Sección de la definición de un job build-* (hasta el siguiente job).
    fn seccion_build<'a>(cfg: &'a str, job: &str) -> &'a str {
        cfg.split(&format!("\n  {}:\n", job))
            .nth(1)
            .unwrap_or("")
            .split("\n  build-")
            .next()
            .unwrap_or("")
            .split("\n  # ──")
            .next()
            .unwrap_or("")
    }

    fn sangria(l: &str) -> usize {
        l.len() - l.trim_start().len()
    }

    fn es_estructural(l: &str) -> bool {
        !l.trim().is_empty() && !l.trim_start().starts_with('#')
    }

    /// Guarda condicional (`when`/`unless`, condición) bajo la que cae el paso
    /// de la línea `idx`: sube al `steps:` que lo contiene y de ahí al `- when:`
    /// o `- unless:` que lo abre. `None` si el paso no está anidado.
    fn guarda_de(lines: &[&str], idx: usize) -> Option<(String, String)> {
        let ind = sangria(lines[idx]);
        let j = (0..idx)
            .rev()
            .find(|&j| es_estructural(lines[j]) && sangria(lines[j]) < ind)?;
        if lines[j].trim() != "steps:" {
            return None;
        }
        let k = (0..j)
            .rev()
            .find(|&k| es_estructural(lines[k]) && sangria(lines[k]) < sangria(lines[j]))?;
        let tipo = lines[k].trim().strip_prefix("- ")?.strip_suffix(':')?.to_string();
        let cond = lines[k + 1..j]
            .iter()
            .find_map(|l| l.trim().strip_prefix("condition: "))?
            .to_string();
        Some((tipo, cond))
    }

    /// Workflow de sonda: nunca publica, todos sus jobs van en modo `probe` y
    /// es mutuamente excluyente con `build-all` (el release, que sí publica).
    /// Un `publish-*` o un `context` aquí publicaría una release desde una rama.
    #[test]
    fn test_workflow_sonda_nunca_publica() {
        let cfg = leer_config_ci();
        let lines: Vec<&str> = cfg.lines().collect();
        let ini = lines
            .iter()
            .position(|l| *l == "  native-cache-probe:")
            .expect("debe existir el workflow native-cache-probe");
        let fin = (ini + 1..lines.len())
            .find(|&i| es_estructural(lines[i]) && sangria(lines[i]) <= 2)
            .unwrap_or(lines.len());
        let wf = lines[ini..fin].join("\n");
        for prohibido in ["publish-release", "publish-metadata", "context:", "requires:", "filters:"] {
            assert!(
                !wf.contains(prohibido),
                "el workflow native-cache-probe no debe contener `{prohibido}`:\n{wf}"
            );
        }
        assert!(
            wf.contains("when: << pipeline.parameters.native_cache_probe >>"),
            "native-cache-probe debe activarse solo con native_cache_probe"
        );
        let jobs: Vec<&str> = lines[ini..fin]
            .iter()
            .filter_map(|l| l.trim().strip_prefix("- "))
            .collect();
        assert_eq!(
            jobs,
            BUILD_JOBS.iter().map(|j| format!("{j}:")).collect::<Vec<_>>(),
            "native-cache-probe debe contener exactamente los 4 build-*"
        );
        assert_eq!(
            wf.matches("probe: true").count(),
            BUILD_JOBS.len(),
            "cada job de native-cache-probe debe llevar probe: true"
        );
        let build_all = cfg
            .split("\n  build-all:\n")
            .nth(1)
            .expect("debe existir el workflow build-all")
            .split("\n    jobs:")
            .next()
            .unwrap_or("");
        assert!(
            build_all.contains("when:\n      not: << pipeline.parameters.native_cache_probe >>"),
            "build-all debe excluirse cuando native_cache_probe es verdadero"
        );
        assert!(
            cfg.contains("  native_cache_probe:\n    type: boolean\n    default: false"),
            "native_cache_probe debe ser booleano con default false"
        );
    }

    /// Modo sonda de los build-*: no restaura ni guarda la clave inmutable de
    /// target-v3 (fijaría un target/ ajeno bajo una clave de producción) ni
    /// empaqueta (el staging exige CIRCLE_TAG == const VERSION).
    #[test]
    fn test_modo_sonda_no_toca_target_v2() {
        let cfg = leer_config_ci();
        let sonda = ("when".to_string(), "<< parameters.probe >>".to_string());
        let no_sonda = ("unless".to_string(), "<< parameters.probe >>".to_string());

        // El comando solo restaura target-v3 (y fija mtime) con target: true.
        let cmd = cfg
            .split("\n  cargo_restore_caches:\n")
            .nth(1)
            .unwrap_or("")
            .split("\n  cargo_save_registry:")
            .next()
            .unwrap_or("");
        let cmd_lines: Vec<&str> = cmd.lines().collect();
        let param_target = ("when".to_string(), "<< parameters.target >>".to_string());
        for marca in ["- target-v3-", "name: Fijar mtime de vendor/cmake-0.1.58"] {
            let i = cmd_lines
                .iter()
                .position(|l| l.trim().starts_with(marca))
                .unwrap_or_else(|| panic!("cargo_restore_caches debe contener `{marca}`"));
            // El paso es el ítem de lista (`- restore_cache:`/`- run:`) que lo contiene.
            let item = (0..=i).rev().find(|&k| cmd_lines[k].trim().starts_with("- ")).unwrap();
            let item = if cmd_lines[item].trim().starts_with("- target-v3-") { item - 2 } else { item };
            assert_eq!(
                guarda_de(&cmd_lines, item),
                Some(param_target.clone()),
                "`{marca}` debe ir bajo when: << parameters.target >> en cargo_restore_caches"
            );
        }

        for job in BUILD_JOBS {
            let section = seccion_build(&cfg, job);
            let lines: Vec<&str> = section.lines().collect();
            assert!(
                section.contains("    parameters:\n      probe:\n        type: boolean\n        default: false"),
                "{job} debe declarar el parámetro probe (boolean, default false)"
            );

            let guardados: Vec<usize> = (0..lines.len())
                .filter(|&i| lines[i].trim() == "- cargo_save_target:")
                .collect();
            assert_eq!(guardados.len(), 1, "{job} debe invocar cargo_save_target una vez");
            assert_eq!(
                guarda_de(&lines, guardados[0]),
                Some(no_sonda.clone()),
                "{job}: cargo_save_target debe ir bajo unless: << parameters.probe >>"
            );

            let restauraciones: Vec<usize> = (0..lines.len())
                .filter(|&i| lines[i].trim() == "- cargo_restore_caches:")
                .collect();
            assert_eq!(
                restauraciones.len(),
                2,
                "{job} debe invocar cargo_restore_caches dos veces (excluyentes por probe)"
            );
            for &i in &restauraciones {
                let args = lines[i + 1..(i + 4).min(lines.len())].join("\n");
                match guarda_de(&lines, i) {
                    Some(g) if g == sonda => assert!(
                        args.contains("target: false"),
                        "{job}: en modo sonda cargo_restore_caches debe llevar target: false"
                    ),
                    Some(g) if g == no_sonda => assert!(
                        args.contains("target: true"),
                        "{job}: fuera de sonda cargo_restore_caches debe llevar target: true"
                    ),
                    otra => panic!("{job}: cargo_restore_caches con guarda inesperada: {otra:?}"),
                }
            }

            // Empaquetado: persist_to_workspace, staging y SHA-256 solo fuera de sonda.
            let persist = lines
                .iter()
                .position(|l| l.trim() == "- persist_to_workspace:")
                .unwrap_or_else(|| panic!("{job} debe persistir su artefacto"));
            assert_eq!(
                guarda_de(&lines, persist),
                Some(no_sonda.clone()),
                "{job}: persist_to_workspace debe ir bajo unless: << parameters.probe >>"
            );
            for nombre in [
                "name: Preparar artefacto versionado (staging)",
                "name: Emitir SHA-256 del artefacto",
            ] {
                let n = lines
                    .iter()
                    .position(|l| l.trim() == nombre)
                    .unwrap_or_else(|| panic!("{job} debe contener `{nombre}`"));
                let item = (0..n).rev().find(|&k| lines[k].trim() == "- run:").unwrap();
                assert_eq!(
                    guarda_de(&lines, item),
                    Some(no_sonda.clone()),
                    "{job}: `{nombre}` debe ir bajo unless: << parameters.probe >>"
                );
            }

            // Diagnóstico de CMake solo en modo sonda.
            let diag = lines
                .iter()
                .position(|l| l.trim() == "- cmake_probe_diagnostics")
                .unwrap_or_else(|| panic!("{job} debe invocar cmake_probe_diagnostics"));
            assert_eq!(
                guarda_de(&lines, diag),
                Some(sonda.clone()),
                "{job}: cmake_probe_diagnostics debe ir bajo when: << parameters.probe >>"
            );

            // --timings y su artefacto, sin condición.
            assert!(
                section.contains("cargo build --release --features full --verbose --timings"),
                "{job} debe compilar con --timings"
            );
            let timings = lines
                .iter()
                .position(|l| l.trim() == "path: target/cargo-timings")
                .unwrap_or_else(|| panic!("{job} debe guardar target/cargo-timings como artefacto"));
            assert_eq!(
                guarda_de(&lines, timings - 1),
                None,
                "{job}: el artefacto cargo-timings debe publicarse sin condición"
            );
        }
    }

    /// Launcher sccache para los proyectos CMake de los build-*, incondicional:
    /// en Unix vía CMAKE_{C,CXX}_COMPILER_LAUNCHER exportados a $BASH_ENV; en
    /// Windows además con el generador Ninja (el de Visual Studio ignora los
    /// launchers), el entorno vcvars64 y CC/CXX con la ruta absoluta de cl.exe.
    #[test]
    fn test_launcher_cmake_en_builds() {
        let cfg = leer_config_ci();
        assert!(
            !cfg.contains("native_sccache:") && !cfg.contains("pipeline.parameters.native_sccache "),
            "el launcher es incondicional: no debe existir el parámetro native_sccache"
        );
        let unix = cfg
            .split("\n  native_sccache_setup_unix:\n")
            .nth(1)
            .expect("debe existir el comando native_sccache_setup_unix")
            .split("\n  native_sccache_setup_windows:")
            .next()
            .unwrap_or("");
        for var in ["CMAKE_C_COMPILER_LAUNCHER=sccache", "CMAKE_CXX_COMPILER_LAUNCHER=sccache"] {
            assert!(
                unix.contains(&format!("echo 'export {var}' >> \"$BASH_ENV\"")),
                "native_sccache_setup_unix debe exportar {var} a $BASH_ENV"
            );
        }
        for job in ["build-linux-x64", "build-linux-arm64", "build-darwin-arm64"] {
            let section = seccion_build(&cfg, job);
            let setup = section
                .find("      - sccache_setup_unix\n      - native_sccache_setup_unix\n")
                .unwrap_or_else(|| panic!("{job} debe invocar native_sccache_setup_unix tras sccache_setup_unix"));
            let build = section
                .find("cargo build --release --features")
                .unwrap_or_else(|| panic!("{job} debe compilar el binario release"));
            assert!(setup < build, "{job}: el launcher debe configurarse antes de compilar");
        }
        let win = seccion_build(&cfg, "build-windows-x64");
        assert!(
            win.contains("      - sccache_setup_windows\n      - native_sccache_setup_windows\n"),
            "build-windows-x64 debe instalar Ninja (native_sccache_setup_windows) sin condición"
        );
        let compilar = win
            .split("name: Compilar binario release (cargo build --release)")
            .nth(1)
            .expect("build-windows-x64 debe tener el paso de compilación")
            .split("\n      - ")
            .next()
            .unwrap_or("");
        for asignacion in [
            r#"$env:CMAKE_GENERATOR = "Ninja""#,
            r#"$env:CMAKE_C_COMPILER_LAUNCHER = "sccache""#,
            r#"$env:CMAKE_CXX_COMPILER_LAUNCHER = "sccache""#,
            r#"$env:PATH = "$env:TEMP\ninja;$env:USERPROFILE\.cargo\bin;$env:PATH""#,
            r#"$env:CC = $Cl"#,
            r#"$env:CXX = $Cl"#,
            r#"\VC\Auxiliary\Build\vcvars64.bat""#,
        ] {
            assert!(
                compilar.contains(asignacion),
                "el paso de compilación de Windows debe fijar `{asignacion}`"
            );
        }
        assert!(
            !compilar.contains("<< pipeline.parameters."),
            "el entorno de compilación de Windows no debe depender de parámetros de pipeline"
        );
        let pos_vcvars = compilar.find("vcvars64.bat").unwrap();
        let pos_path = compilar.find(r#"$env:PATH = "$env:TEMP\ninja"#).unwrap();
        let pos_launcher = compilar.find("$env:CMAKE_GENERATOR").unwrap();
        let pos_build = compilar.find("cargo build --release --features").unwrap();
        assert!(
            pos_vcvars < pos_path,
            "vcvars64 debe importarse antes de anteponer Ninja y .cargo\\bin al PATH"
        );
        assert!(
            pos_launcher < pos_build,
            "CMAKE_GENERATOR debe fijarse antes de cargo build"
        );
    }

    #[test]
    fn test_render_cask_from_tag_strips_v() {
        let sums = sample_sums();
        let c = render_cask_from_tag("v1.2.3", &sums).unwrap();
        assert!(c.contains(r#"version "1.2.3""#));
    }

    #[test]
    fn test_get_version_from_cargo() {
        let v = get_version().unwrap();
        assert!(!v.is_empty());
    }

    #[test]
    fn test_normalize() {
        assert_eq!(normalize("My_Package.Name"), "my-package-name");
        assert_eq!(normalize("a--b__c..d"), "a-b-c-d");
    }

    #[test]
    fn test_engine_bin_name_por_plataforma() {
        if cfg!(windows) {
            assert_eq!(engine_bin_name(), "qwen_tts.exe");
        } else {
            assert_eq!(engine_bin_name(), "qwen_tts");
        }
    }

    #[test]
    fn test_make_program_por_plataforma() {
        if cfg!(windows) {
            assert_eq!(make_program(), "mingw32-make");
        } else {
            assert_eq!(make_program(), "make");
        }
    }

    #[test]
    fn test_engine_dir() {
        let dir = engine_dir();
        assert!(dir.ends_with("qwen3-tts"));
        assert!(dir.starts_with("vendor"));
    }

    /// CHANGELOG mínimo con ToC, sección curada `## [No publicado]`, una sección
    /// de versión previa y el bloque de definiciones de enlace al final.
    fn sample_changelog() -> String {
        "\
# Changelog

## Tabla de contenidos

- [No publicado](#no-publicado)
- [0.20.3 — 2026-09-22](#0203--2026-09-22)

## [No publicado]

### Cambiado

- algo curado a mano durante el desarrollo.

## [0.20.3] — 2026-09-22

### Cambiado

- release previa ya publicada.

[0.20.3]: https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/compare/v0.20.2...v0.20.3
"
        .to_string()
    }

    #[test]
    fn test_slug_coincide_con_ancla_de_github() {
        // GitHub ancla `## [0.20.12] — 2026-09-23` como `#02012--2026-09-23`.
        assert_eq!(slug("0.20.12", "2026-09-23"), "02012--2026-09-23");
        assert_eq!(slug("0.20.3", "2026-09-22"), "0203--2026-09-22");
    }

    #[test]
    fn test_promote_changelog_camino_feliz() {
        let out =
            promote_changelog_text(&sample_changelog(), "0.20.4", "0.20.3", "2026-09-24").unwrap();
        // Cabecera renombrada; la sección [No publicado] desaparece.
        assert!(out.contains("## [0.20.4] — 2026-09-24"));
        assert!(!out.contains("## [No publicado]"));
        // Entrada de ToC transformada, con el ancla que GitHub genera para la cabecera.
        assert!(out.contains("- [0.20.4 — 2026-09-24](#0204--2026-09-24)"));
        assert!(!out.contains("- [No publicado](#no-publicado)"));
        // Definición de enlace de comparación añadida al final.
        assert!(out.contains(
            "[0.20.4]: https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/compare/v0.20.3...v0.20.4"
        ));
        // El cuerpo curado se conserva bajo la nueva cabecera.
        assert!(out.contains("- algo curado a mano durante el desarrollo."));
    }

    #[test]
    fn test_promote_changelog_falla_sin_no_publicado() {
        // Un CHANGELOG ya promovido no tiene sección [No publicado] que promover.
        let promovido =
            promote_changelog_text(&sample_changelog(), "0.20.4", "0.20.3", "2026-09-24").unwrap();
        let err = promote_changelog_text(&promovido, "0.20.5", "0.20.4", "2026-09-25").unwrap_err();
        assert!(err.to_string().contains("No publicado"));
    }

    #[test]
    fn test_promote_changelog_falla_con_todo_residual() {
        let con_todo = sample_changelog().replace(
            "- algo curado a mano durante el desarrollo.",
            "- algo a medias.  <!-- TODO: curar -->",
        );
        let err = promote_changelog_text(&con_todo, "0.20.4", "0.20.3", "2026-09-24").unwrap_err();
        assert!(err.to_string().contains("TODO: curar"));
    }

    #[test]
    fn test_promote_changelog_falla_si_version_ya_existe() {
        // La versión objetivo coincide con una sección de versión ya presente.
        let err = promote_changelog_text(&sample_changelog(), "0.20.3", "0.20.2", "2026-09-24")
            .unwrap_err();
        assert!(err.to_string().contains("ya existe"));
    }

    #[test]
    fn test_validate_changelog_promovido_pasa() {
        let promovido =
            promote_changelog_text(&sample_changelog(), "0.20.4", "0.20.3", "2026-09-24").unwrap();
        assert!(validate_changelog_text(&promovido, "0.20.4").is_ok());
    }

    #[test]
    fn test_validate_changelog_falla_sin_toc() {
        let promovido =
            promote_changelog_text(&sample_changelog(), "0.20.4", "0.20.3", "2026-09-24").unwrap();
        let sin_toc = promovido.replace("- [0.20.4 — 2026-09-24](#0204--2026-09-24)\n", "");
        let err = validate_changelog_text(&sin_toc, "0.20.4").unwrap_err();
        assert!(err.to_string().contains("tabla de contenidos"));
    }

    #[test]
    fn test_validate_changelog_falla_sin_enlace() {
        let promovido =
            promote_changelog_text(&sample_changelog(), "0.20.4", "0.20.3", "2026-09-24").unwrap();
        let sin_enlace = promovido.replace(
            "[0.20.4]: https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/compare/v0.20.3...v0.20.4\n",
            "",
        );
        let err = validate_changelog_text(&sin_enlace, "0.20.4").unwrap_err();
        assert!(err.to_string().contains("enlace de comparación"));
    }

    #[test]
    fn test_validate_changelog_falla_con_todo_residual() {
        let promovido =
            promote_changelog_text(&sample_changelog(), "0.20.4", "0.20.3", "2026-09-24").unwrap();
        let con_todo = promovido.replace(
            "- algo curado a mano durante el desarrollo.",
            "- algo a medias.  <!-- TODO: curar -->",
        );
        let err = validate_changelog_text(&con_todo, "0.20.4").unwrap_err();
        assert!(err.to_string().contains("TODO: curar"));
    }

    #[test]
    fn test_validate_changelog_falla_con_no_publicado_residual() {
        let promovido =
            promote_changelog_text(&sample_changelog(), "0.20.4", "0.20.3", "2026-09-24").unwrap();
        // Reintroducir una sección [No publicado] sin promover.
        let con_residuo = promovido.replace(
            "## [0.20.4] — 2026-09-24",
            "## [No publicado]\n\n### Cambiado\n\n- nuevo trabajo.\n\n## [0.20.4] — 2026-09-24",
        );
        let err = validate_changelog_text(&con_residuo, "0.20.4").unwrap_err();
        assert!(err.to_string().contains("No publicado"));
    }

    /// Regresión: una entrada de `[No publicado]` que menciona el literal
    /// `## [No publicado]` en su prosa (dentro de backticks) no debe producir un
    /// falso positivo. La promoción renombra solo la cabecera real y la validación
    /// posterior pasa. Reproduce el defecto expuesto por el corte de v0.20.4.
    #[test]
    fn test_promocion_y_validacion_ignoran_menciones_en_prosa() {
        let con_prosa = sample_changelog().replace(
            "- algo curado a mano durante el desarrollo.",
            "- redefine `release`: renombra la cabecera `## [No publicado]` a `## [X.Y.Z]`.",
        );
        let promovido =
            promote_changelog_text(&con_prosa, "0.20.4", "0.20.3", "2026-09-24").unwrap();
        // La cabecera real se promovió; la mención en prosa se conserva intacta.
        assert!(promovido.contains("## [0.20.4] — 2026-09-24"));
        assert!(promovido.contains("renombra la cabecera `## [No publicado]`"));
        // Y la validación NO da falso positivo por esa mención en backticks.
        assert!(validate_changelog_text(&promovido, "0.20.4").is_ok());
    }

    #[test]
    fn test_validate_changelog_anchors_pasa_sobre_promovido() {
        let promovido =
            promote_changelog_text(&sample_changelog(), "0.20.4", "0.20.3", "2026-09-24").unwrap();
        assert!(validate_changelog_anchors(&promovido).is_ok());
    }

    #[test]
    fn test_validate_changelog_anchors_falla_con_ancla_vieja_sin_doble_guion() {
        let promovido =
            promote_changelog_text(&sample_changelog(), "0.20.4", "0.20.3", "2026-09-24").unwrap();
        // Ancla que no coincide con la que GitHub asigna a la cabecera.
        let ancla_rota = promovido.replace(
            "- [0.20.4 — 2026-09-24](#0204--2026-09-24)",
            "- [0.20.4 — 2026-09-24](#0204-20260924)",
        );
        let err = validate_changelog_anchors(&ancla_rota).unwrap_err();
        assert!(err.to_string().contains("0204--2026-09-24"));
        // El invariante de anclas también se ejerce desde validate_changelog_text.
        let err = validate_changelog_text(&ancla_rota, "0.20.4").unwrap_err();
        assert!(err.to_string().contains("0204--2026-09-24"));
    }

    #[test]
    fn test_validate_changelog_anchors_falla_si_falta_la_entrada_de_toc() {
        // La cabecera de [0.20.3] existe pero su entrada de ToC no está (caso
        // distinto de "ancla rota": aquí no hay ninguna línea para esa versión).
        let sin_entrada =
            sample_changelog().replace("- [0.20.3 — 2026-09-22](#0203--2026-09-22)\n", "");
        let err = validate_changelog_anchors(&sin_entrada).unwrap_err();
        assert!(err.to_string().contains("0203--2026-09-22"));
    }

    #[test]
    fn test_diff_source_offer_coincide() {
        let rendered = render_source_offer("1.2.3");
        assert!(diff_source_offer(&rendered, &rendered).is_ok());
    }

    #[test]
    fn test_diff_source_offer_normaliza_crlf() {
        let rendered = render_source_offer("1.2.3");
        let con_crlf = rendered.replace('\n', "\r\n");
        assert!(diff_source_offer(&con_crlf, &rendered).is_ok());
    }

    #[test]
    fn test_diff_source_offer_desincronizado() {
        let rendered = render_source_offer("1.2.3");
        let viejo = render_source_offer("1.2.2");
        let err = diff_source_offer(&viejo, &rendered).unwrap_err();
        assert!(err.contains("SOURCE-OFFER.md desincronizado"));
    }

    #[test]
    fn test_format_licenses_gate_message_sincronizado() {
        assert!(format_licenses_gate_message(&[], &[]).is_none());
    }

    #[test]
    fn test_format_licenses_gate_message_con_faltantes_y_sobrantes() {
        let missing = vec!["crate-nuevo".to_string()];
        let extra = vec!["crate-viejo".to_string()];
        let msg = format_licenses_gate_message(&missing, &extra).unwrap();
        assert!(msg.contains("crate-nuevo"));
        assert!(msg.contains("atribución faltante"));
        assert!(msg.contains("crate-viejo"));
        assert!(msg.contains("obsoletas"));
    }
}
