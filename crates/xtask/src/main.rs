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
  desc "Motor de sintesis de voz (TTS) offline con clonacion de voz en espanol latinoamericano"
  homepage "https://github.com/{repo}"

  livecheck do
    url :url
    strategy :github_latest
  end

  depends_on macos: ">= :big_sur"

  binary "ai-voice-interconnector"

  zap trash: [
    "~/Library/Application Support/ai-voice-interconnector",
    "~/.cache/huggingface/hub/models--ResembleAI--Chatterbox-Multilingual-es-mx-latam",
    "~/.cache/huggingface/hub/models--ResembleAI--chatterbox",
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
    /// Genera el borrador de release (bump de versión + CHANGELOG)
    Release {
        #[arg(value_name = "X.Y.Z")]
        version: String,
    },
    /// Verifica la sección del CHANGELOG para la versión actual
    Changelog {
        #[arg(long)]
        check: bool,
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
                anyhow::bail!("versión inválida '{}': debe ser X.Y.Z (ej. 0.14.0)", version);
            }
            bump_version(version)?;
            scaffold_changelog(version)?;
            println!("Borrador de release {} generado:", version);
            println!("  - src/main.rs (VERSION)");
            println!("  - Cargo.toml (package.version)");
            println!("  - Cargo.lock (ai-voice-interconnector)");
            println!("  - tests/golden/cli_version.json");
            println!("  - SOURCE-OFFER.md (oferta GPLv3 §6 versionada)");
            println!("  - CHANGELOG.md (sección + TOC + definición de enlace)");
            println!("Cierra los TODO: curar del CHANGELOG, commitea con conventional-commits y crea el tag v{}", version);
        }
        Commands::Changelog { check } => {
            if check {
                check_changelog()?;
                println!("CHANGELOG.md en sincronía con la versión actual");
            } else {
                println!("Usa --check para verificar la sección del CHANGELOG");
            }
        }
    }
    Ok(())
}

fn get_version() -> Result<String> {
    // Intento legado Python (por compatibilidad, aunque ya no existe)
    let legacy = Path::new("src/ai_voice_interconnector/__init__.py");
    if legacy.is_file() {
        let text = std::fs::read_to_string(legacy)?;
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with("__version__") {
                if let Some((_, v)) = line.split_once('=') {
                    return Ok(v.trim().trim_matches(|c| c == '"' || c == '\'').to_string());
                }
            }
        }
    }
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

/// Genera la sección del CHANGELOG pre-rellenada para `version`.
fn scaffold_changelog(version: &str) -> Result<()> {
    let last = last_tag()?;
    let date = today_iso();

    // Recopilar commits desde el último tag
    let output = std::process::Command::new("git")
        .args([
            "log",
            &format!("v{}..HEAD", last),
            "--pretty=format:%H%x00%s%x00%B",
        ])
        .output()?;

    if !output.status.success() {
        anyhow::bail!("no se pudo obtener el historial de commits");
    }

    let raw = String::from_utf8(output.stdout)?;
    let records: Vec<&str> = raw.split("\u{00}").filter(|s| !s.is_empty()).collect();

    // Cada commit: hash, subject, body (separados por \x00)
    // records = [hash, subject, body, hash, subject, body, ...]
    let mut added: Vec<String> = Vec::new();
    let mut fixed: Vec<String> = Vec::new();
    let mut changed: Vec<String> = Vec::new();
    let mut breaking: Vec<String> = Vec::new();

    let mut i = 0;
    while i + 2 < records.len() {
        let _hash = records[i];
        let subject = records[i + 1].trim();
        let body = records[i + 2].trim();
        i += 3;

        // Extraer el Resumen de cambios (primera línea del bloque si existe)
        let summary = extract_resumen_cambios(body);

        // Parsear tipo y breaking del subject
        let (tipo, is_breaking) = parse_commit_header(subject);

        let bullet = if let Some(resumen) = &summary {
            format!("- {} — {}  <!-- TODO: curar -->", subject, resumen)
        } else {
            format!("- {}  <!-- TODO: curar -->", subject)
        };

        match (tipo.as_str(), is_breaking) {
            ("feat", false) => added.push(bullet),
            ("fix", false) => fixed.push(bullet),
            (_, true) => breaking.push(bullet),
            ("refactor", false) | ("perf", false) | ("build", false) => changed.push(bullet),
            _ => changed.push(bullet),
        }
    }

    if added.is_empty() && fixed.is_empty() && changed.is_empty() && breaking.is_empty() {
        anyhow::bail!(
            "no se encontraron commits desde el tag v{} — el rango está vacío",
            last
        );
    }

    // Construir la sección del CHANGELOG
    let mut section = String::new();
    section.push_str(&format!("## [{}] — {}\n\n", version, date));

    // Párrafo introductorio: placeholder del humano
    section.push_str("<!-- TODO: curar — escribe aquí el párrafo introductorio que\n");
    section.push_str("   sintetice la necesidad observada y la propuesta de la release. -->\n\n");

    if !changed.is_empty() {
        section.push_str("### Cambiado\n\n");
        section.push_str(&changed.join("\n"));
        section.push_str("\n\n");
    }
    if !added.is_empty() {
        section.push_str("### Añadido\n\n");
        section.push_str(&added.join("\n"));
        section.push_str("\n\n");
    }
    if !fixed.is_empty() {
        section.push_str("### Corregido\n\n");
        section.push_str(&fixed.join("\n"));
        section.push_str("\n\n");
    }
    if !breaking.is_empty() {
        section.push_str("### Notas de release\n\n");
        section.push_str(&breaking.join("\n"));
        section.push_str("\n\n");
    }

    // Insertar la sección en el CHANGELOG (después del TOC, antes de la primera sección)
    let changelog_path = Path::new("CHANGELOG.md");
    let text = std::fs::read_to_string(changelog_path)?;

    // Insertar después de la tabla de contenidos (línea "## Tabla de contenidos")
    let toc_end = text
        .find("## Tabla de contenidos")
        .ok_or_else(|| anyhow::anyhow!("no se encontró la tabla de contenidos del CHANGELOG"))?;

    // Buscar el final del TOC: la primera "## [" que sigue
    let after_toc = &text[toc_end..];
    let toc_close = after_toc
        .find("\n## [")
        .ok_or_else(|| anyhow::anyhow!("no se encontró el inicio de la primera sección del CHANGELOG"))?;

    let insert_pos = toc_end + toc_close + 1; // posición del "\n" antes de "## ["
    let mut new_text = text[..insert_pos].to_string();
    new_text.push('\n');
    new_text.push_str(&section);
    new_text.push_str(&text[insert_pos..]);

    // Añadir entrada al TOC (después del encabezado "## Tabla de contenidos")
    let toc_marker = "## Tabla de contenidos\n\n";
    let toc_pos = new_text
        .find(toc_marker)
        .ok_or_else(|| anyhow::anyhow!("no se encontró el marcador del TOC"))?
        + toc_marker.len();
    let toc_entry = format!("- [{} — {}](#{})\n", version, date, slug(version, &date));
    let mut result = new_text[..toc_pos].to_string();
    result.push_str(&toc_entry);
    result.push_str(&new_text[toc_pos..]);

    // Añadir definición de enlace al final del archivo
    let link = format!("[{}]: https://github.com/{}/compare/v{}...v{}\n", version, GITHUB_REPO, last, version);
    if !result.contains(&format!("[{}]: ", version)) {
        result = result.trim_end().to_string();
        result.push('\n');
        result.push_str(&link);
    }

    std::fs::write(changelog_path, result)?;
    Ok(())
}

/// Extrae la primera línea del bloque "Resumen de cambios:" del cuerpo del commit.
fn extract_resumen_cambios(body: &str) -> Option<String> {
    for line in body.lines() {
        if line.trim_start().starts_with("Resumen de cambios") {
            // La primera bullet después del header
            for bullet in body.lines().skip_while(|l| !l.trim_start().starts_with("Resumen de cambios")) {
                let bullet = bullet.trim_start();
                if bullet.starts_with("- ") || bullet.starts_with("* ") {
                    return Some(bullet.trim_start_matches(|c| c == '-' || c == '*' || c == ' ').to_string());
                }
            }
        }
    }
    None
}

/// Parsea el tipo de commit y si es breaking change del subject.
fn parse_commit_header(subject: &str) -> (String, bool) {
    // feat(scope)!: ... o feat!: ...
    let re = Regex::new(r"^(\w+)(\([^)]+\))?!?:.").unwrap();
    if let Some(caps) = re.captures(subject) {
        let tipo = caps[1].to_string();
        let has_bang = subject.contains("!:");
        let has_breaking_footer = subject.contains("BREAKING CHANGE");
        return (tipo, has_bang || has_breaking_footer);
    }
    ("chore".to_string(), false)
}

/// Formatea la fecha actual como YYYY-MM-DD usando el comando `date`.
fn today_iso() -> String {
    let output = std::process::Command::new("date")
        .arg("+%Y-%m-%d")
        .output();
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

fn check_changelog() -> Result<()> {
    let version = get_version()?;
    let text = std::fs::read_to_string("CHANGELOG.md")?;
    let marker = format!("## [{}]", version);
    for line in text.lines() {
        if line.starts_with(&marker) {
            return Ok(());
        }
    }
    anyhow::bail!("no se encontró la sección [{}] en CHANGELOG.md", version)
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
        assert!(c.contains("models--ResembleAI--chatterbox"));
        assert!(c.contains("GPL-3.0-or-later"));
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
}
