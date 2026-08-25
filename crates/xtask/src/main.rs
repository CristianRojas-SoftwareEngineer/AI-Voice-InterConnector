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
            diff::Result::Both(_, _) => {}
        }
    }
    out
}

// Minimal diff helper inline to avoid extra dep
mod diff {
    pub enum Result<'a> {
        Left(&'a str),
        Right(&'a str),
        Both(&'a str, &'a str),
    }
    pub fn lines<'a>(a: &'a str, b: &'a str) -> Vec<Result<'a>> {
        let a_lines: Vec<&str> = a.lines().collect();
        let b_lines: Vec<&str> = b.lines().collect();
        let mut res = Vec::new();
        let mut i = 0;
        let mut j = 0;
        while i < a_lines.len() && j < b_lines.len() {
            if a_lines[i] == b_lines[j] {
                res.push(Result::Both(a_lines[i], b_lines[j]));
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
    let mut lines = text.lines();
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

    fn sha(label: &str) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        // Use actual sha256 for test stability: use a simple deterministic hex (not real sha256 but for test we need 64 hex)
        // Instead compute real sha256 via a small helper
        let mut hasher = DefaultHasher::new();
        label.hash(&mut hasher);
        format!("{:064x}", hasher.finish())
    }

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
