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
            let rendered = render_source_offer(&version);
            if check {
                let dest = Path::new("SOURCE-OFFER.md");
                if !dest.is_file() {
                    eprintln!("SOURCE-OFFER.md no existe; esperado:\n{}", rendered);
                    std::process::exit(1);
                }
                let current = std::fs::read_to_string(dest)?;
                // Normalizar CRLF vs LF para comparar (git autocrlf, PowerShell)
                let norm_current = current.replace("\r\n", "\n");
                let norm_rendered = rendered.replace("\r\n", "\n");
                if norm_current != norm_rendered {
                    eprintln!("SOURCE-OFFER.md desincronizado: regenera con `cargo run -p xtask -- source-offer > SOURCE-OFFER.md`");
                    // Diff mínimo
                    for line in diff_lines(&norm_current, &norm_rendered) {
                        eprintln!("{}", line);
                    }
                    std::process::exit(1);
                }
                println!("SOURCE-OFFER.md en sincronía");
            } else {
                print!("{}", rendered);
            }
        }
        Commands::Licenses { check: _ } => {
            let (missing, extra) = check_licenses()?;
            if missing.is_empty() && extra.is_empty() {
                println!("THIRD-PARTY-LICENSES.md está en sincronía con Cargo.lock");
            } else {
                if !missing.is_empty() {
                    println!("Crates del lock SIN fila en THIRD-PARTY-LICENSES.md (atribución faltante):");
                    for n in &missing {
                        println!("  + {}", n);
                    }
                }
                if !extra.is_empty() {
                    println!("Filas de THIRD-PARTY-LICENSES.md sin crate en el lock (obsoletas):");
                    for n in &extra {
                        println!("  - {}", n);
                    }
                }
                println!("\nRegenera el inventario (cargo metadata).");
                std::process::exit(1);
            }
        }
        Commands::Release { version } => {
            let version = version.trim();
            if !Regex::new(r"^\d+\.\d+\.\d+$").unwrap().is_match(version) {
                anyhow::bail!(
                    "versión inválida '{}': debe ser X.Y.Z (ej. 0.14.0)",
                    version
                );
            }
            // Pre-validación atómica: abortar antes de mutar si el árbol está sucio
            // o no hay commits nuevos desde el último tag.
            {
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
            println!("Release {} preparado:", version);
            println!("  - src/main.rs (VERSION)");
            println!("  - Cargo.toml (package.version)");
            println!("  - Cargo.lock (ai-voice-interconnector)");
            println!("  - tests/golden/cli_version.json");
            println!("  - SOURCE-OFFER.md (oferta GPLv3 §6 versionada)");
            println!("  - CHANGELOG.md (sección promovida desde [No publicado] + ToC + enlace)");
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

/// Genera el slug de anclaje para el TOC (lowercase, sin puntos/special).
fn slug(version: &str, date: &str) -> String {
    let v = version.replace('.', "");
    let d = date.replace('-', "");
    format!("{}-{}", v, d)
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

    #[test]
    fn test_pipeline_heterogeneo_y_sccache_incondicional() {
        let candidates = [
            ".circleci/config.yml",
            "../../.circleci/config.yml",
            "C:/Users/Cristian/Desktop/Proyectos/Voices/AI-Voice-InterConnector/.circleci/config.yml",
        ];
        let mut cfg_opt = None;
        for p in candidates {
            if let Ok(t) = std::fs::read_to_string(p) {
                cfg_opt = Some(t);
                break;
            }
        }
        // Fallback via CARGO_MANIFEST_DIR
        let cfg = cfg_opt.unwrap_or_else(|| {
            let m =
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.circleci/config.yml");
            std::fs::read_to_string(&m).expect("no se pudo leer .circleci/config.yml")
        });
        // Modelo vigente (post remediación de caché): test-linux, test-windows, test-macos,
        // coverage y build-* usan cargo_restore_caches (registry + target-v2) y sccache
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
            "test-linux debe usar cargo_restore_caches (registry + target-v2)"
        );
        assert!(
            linux_section.contains("variant: test"),
            "test-linux debe usar variant: test"
        );
        assert!(
            linux_section.contains("cargo_save_target"),
            "test-linux debe guardar target-v2 (cargo_save_target)"
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
            "test-windows debe guardar target-v2 (cargo_save_target)"
        );
        assert!(
            windows_section.contains("sccache_restore_cache"),
            "test-windows debe usar sccache"
        );
        assert!(
            windows_section.contains("sccache_save_cache"),
            "test-windows debe guardar sccache (sccache_save_cache)"
        );
        // coverage debe usar cargo_restore_caches con os: linux y variant: cov y guardar target-v2
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
            "coverage debe guardar target-v2 (cargo_save_target)"
        );
        assert!(
            coverage_section.contains("sccache_save_cache"),
            "coverage debe guardar sccache (sccache_save_cache)"
        );
        // test-macos usa cargo_restore_caches (registry + target-v2) y sccache autoconsistente (variant: test), igual que test-linux
        let macos_section = cfg
            .split("  test-macos:")
            .nth(1)
            .unwrap_or("")
            .split("  coverage:")
            .next()
            .unwrap_or("");
        assert!(
            macos_section.contains("cargo_restore_caches"),
            "test-macos debe usar cargo_restore_caches (registry + target-v2)"
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
            "test-macos debe guardar target-v2 (cargo_save_target)"
        );
        // build-* deben usar cargo_restore_caches con target-v2 full + cargo clean -p (heterogéneo con target en build-*)
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
                "{job} debe usar cargo_restore_caches (con target-v2)"
            );
            assert!(
                section.contains("variant: full"),
                "{job} debe usar variant: full"
            );
            assert!(
                section.contains("cargo_save_target"),
                "{job} debe guardar target-v2 (cargo_save_target)"
            );
            assert!(
                section.contains("sccache_save_cache"),
                "{job} debe guardar sccache (sccache_save_cache)"
            );
            assert!(
                section.contains("cargo clean -p ai-voice-interconnector"),
                "{job} debe ejecutar cargo clean -p ai-voice-interconnector para determinismo"
            );
        }
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
- [0.20.3 — 2026-09-22](#0203-20260922)

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
    fn test_promote_changelog_camino_feliz() {
        let out =
            promote_changelog_text(&sample_changelog(), "0.20.4", "0.20.3", "2026-09-24").unwrap();
        // Cabecera renombrada; la sección [No publicado] desaparece.
        assert!(out.contains("## [0.20.4] — 2026-09-24"));
        assert!(!out.contains("## [No publicado]"));
        // Entrada de ToC transformada (con slug sin puntos ni guiones).
        assert!(out.contains("- [0.20.4 — 2026-09-24](#0204-20260924)"));
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
        let sin_toc = promovido.replace("- [0.20.4 — 2026-09-24](#0204-20260924)\n", "");
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
}
