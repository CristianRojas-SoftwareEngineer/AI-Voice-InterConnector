// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

#![cfg_attr(not(test), deny(clippy::let_underscore_must_use))]

use anyhow::{anyhow, Result};
use avi_shared::pins::Pins;
use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use regex::Regex;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

mod alsa_symbols;
mod bootstrap;
mod clean;
mod comments;
mod deps;
mod doctor;
mod features;
mod first_party;
mod headers;
mod install;
mod language;
mod notices;
mod package;
mod pins;

const GITHUB_REPO: &str = "CristianRojas-SoftwareEngineer/AI-Voice-InterConnector";
const CASK_NAME: &str = "ai-voice-interconnector";
// Manifiesto raíz embebido: el directorio actual de los tests es el de xtask,
// cuyo Cargo.toml declara otra descripción, y cargo recompila xtask si cambia.
const ROOT_MANIFEST: &str = include_str!("../../../Cargo.toml");

const CASK_TEMPLATE: &str = r#"cask "{cask_name}" do
  version "{cask_version}"
  sha256 "{cask_sha256}"

  url "https://github.com/{repo}/releases/download/v#{version}/ai-voice-interconnector-#{version}-arm64-macos.tar.gz"
  name "AI Voice-InterConnector"
  desc "{cask_desc}"
  homepage "https://github.com/{repo}"

  livecheck do
    url :url
    strategy :github_latest
  end

  depends_on macos: ">= :tahoe"

  binary "ai-voice-interconnector"

  zap trash: [
{zap_entries}
  ]

  caveats <<~EOS
    Los modelos de voz (traduccion, sintesis, clonado de voz y transcripcion; unos 5,85 GB)
    no vienen incluidos:
    descargalos una sola vez con:
      ai-voice-interconnector setup

    Licencia: MPL-2.0. El aviso de disponibilidad del codigo fuente (MPL-2.0
    seccion 3.2) y las atribuciones de terceros viajan dentro del archivo
    instalado:
      #{staged_path}/SOURCE-OFFER.md
      #{staged_path}/THIRD-PARTY-LICENSES.md
  EOS
end
"#;

const SOURCE_OFFER_TEMPLATE: &str = r#"# Disponibilidad del código fuente (MPL-2.0 §3.2)

**AI Voice-InterConnector {version}** se distribuye bajo la licencia
**MPL-2.0** (ver `LICENSE`). Conforme a la sección 3.2 de la MPL-2.0,
el código fuente correspondiente a esta versión exacta está disponible en:

- **Código fuente (tarball del tag):**
  <https://github.com/{repo}/archive/refs/tags/v{version}.tar.gz>
- **Release v{version} (artefactos y notas):**
  <https://github.com/{repo}/releases/tag/v{version}>
- **Repositorio:** <https://github.com/{repo}>

Las atribuciones de las dependencias redistribuidas están en
`THIRD-PARTY-LICENSES.md`, junto a este archivo. Los pesos de los modelos no se
empaquetan en el binario: se descargan con `setup` y conservan sus licencias:
Qwen3-TTS (CustomVoice y Base) y DPDFNet, Apache-2.0; Parakeet TDT 0.6B v3
(ONNX) y los derivados int8 de opus-mt, CC-BY-4.0.
{mpl_crates}
## FFmpeg {ffmpeg_version}

El paquete incluye **FFmpeg {ffmpeg_version}** como ejecutable independiente: la
aplicación lo invoca como proceso y no lo enlaza con el binario principal. Los
builds distribuidos son GPL-3.0-or-later. Su código fuente correspondiente está
disponible en estos assets del release v{version}, verificados contra los
hashes fijados en `packaging/pins.json`:

{ffmpeg_sources}

Los binarios proceden de estos builds (Linux: build `{ffmpeg_btbn_build}` del
release `{ffmpeg_btbn_tag}` de BtbN):

{ffmpeg_binaries}

La configuración con la que se compiló cada binario figura en estos assets del
mismo release:

{ffmpeg_buildconfs}

## Solicitud del código fuente

Si necesitas el árbol de fuentes exacto de un build, incluidas las bibliotecas
que enlaza, o recibiste este binario sin acceso a las URLs anteriores, solicítalo
abriendo un issue en <https://github.com/{repo}/issues>. La oferta se mantiene
durante al menos tres años desde la entrega del binario.
"#;

#[derive(Parser)]
#[command(name = "xtask", about = "Tareas de desarrollo")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

/// Localiza el árbol de comandos de clap al español (réplica de la localización
/// del binario del producto, sin acoplamiento entre crates): plantilla con `Uso:`,
/// encabezados `Opciones`/`Argumentos`/`Comandos`, flag propio `-h/--help`
/// en español, subcomando `help` deshabilitado y anotaciones
/// `[por defecto: …]`/`[valores posibles: …]` derivadas de los valores
/// reales. Se aplica recursivamente a cada subcomando. `xtask` no tiene
/// flag de versión, así que no hay nada que deshabilitar ni replicar ahí.
fn localize_command(cmd: clap::Command) -> clap::Command {
    cmd.help_template("{before-help}{about-with-newline}\nUso: {usage}\n\n{all-args}{after-help}")
        .subcommand_help_heading("Comandos")
        .subcommand_value_name("COMANDO")
        .disable_help_subcommand(true)
        .disable_help_flag(true)
        .arg(
            clap::Arg::new("help")
                .short('h')
                .long("help")
                .action(clap::ArgAction::Help)
                .help("Muestra la ayuda"),
        )
        .mut_args(localize_arg)
        .mut_subcommands(localize_command)
}

/// Localiza un argumento de clap: encabezado `Argumentos`/`Opciones` salvo
/// encabezado propio, y anotaciones en español derivadas de los valores
/// reales (solo si recibe valor y no las ocultaba ya).
fn localize_arg(arg: clap::Arg) -> clap::Arg {
    let mut arg = arg;
    if arg.get_help_heading().is_none() {
        if arg.is_positional() {
            arg = arg.help_heading("Argumentos");
        } else {
            arg = arg.help_heading("Opciones");
        }
    }
    if arg.get_action().takes_values()
        && !arg.is_hide_default_value_set()
        && !arg.is_hide_possible_values_set()
    {
        let defaults: Vec<String> = arg
            .get_default_values()
            .iter()
            .map(|v| v.to_string_lossy().into_owned())
            .collect();
        let possible: Vec<String> = arg
            .get_possible_values()
            .iter()
            .filter(|pv| !pv.is_hide_set())
            .map(|pv| pv.get_name().to_string())
            .collect();
        if !defaults.is_empty() || !possible.is_empty() {
            let mut help = arg.get_help().map(|h| h.to_string()).unwrap_or_default();
            if !defaults.is_empty() {
                if !help.is_empty() {
                    help.push(' ');
                }
                help.push_str(&format!("[por defecto: {}]", defaults.join(", ")));
            }
            if !possible.is_empty() {
                if !help.is_empty() {
                    help.push(' ');
                }
                help.push_str(&format!("[valores posibles: {}]", possible.join(", ")));
            }
            arg = arg
                .hide_default_value(true)
                .hide_possible_values(true)
                .help(help);
        }
    }
    arg
}

/// Traduce los errores comunes de parseo de clap al español conservando el
/// exit code. Ayuda y los tipos sin mapear conservan el render original.
fn render_clap_error(err: clap::Error) -> ! {
    use clap::error::{ContextKind, ErrorKind};
    let kind = err.kind();
    let mapped = matches!(
        kind,
        ErrorKind::UnknownArgument
            | ErrorKind::InvalidValue
            | ErrorKind::ValueValidation
            | ErrorKind::MissingRequiredArgument
            | ErrorKind::InvalidSubcommand
            | ErrorKind::MissingSubcommand
            | ErrorKind::ArgumentConflict
            | ErrorKind::TooManyValues
            | ErrorKind::WrongNumberOfValues
            | ErrorKind::NoEquals
    );
    if !mapped {
        err.exit();
    }
    let param = |k: ContextKind| err.get(k).map(|v| v.to_string()).unwrap_or_default();
    let message = match kind {
        ErrorKind::UnknownArgument => {
            format!("argumento inesperado '{}'", param(ContextKind::InvalidArg))
        }
        ErrorKind::InvalidValue => {
            let base = format!(
                "valor inválido '{}' para '{}'",
                param(ContextKind::InvalidValue),
                param(ContextKind::InvalidArg)
            );
            let valid = param(ContextKind::ValidValue);
            if valid.is_empty() {
                base
            } else {
                format!("{base}, valores posibles: {valid}")
            }
        }
        ErrorKind::ValueValidation => {
            let cause = std::error::Error::source(&err)
                .map(|s| s.to_string())
                .unwrap_or_default();
            if cause.is_empty() {
                format!(
                    "valor inválido '{}' para '{}'",
                    param(ContextKind::InvalidValue),
                    param(ContextKind::InvalidArg)
                )
            } else {
                format!(
                    "valor inválido '{}' para '{}': {cause}",
                    param(ContextKind::InvalidValue),
                    param(ContextKind::InvalidArg)
                )
            }
        }
        ErrorKind::MissingRequiredArgument => {
            format!(
                "faltan argumentos obligatorios: {}",
                param(ContextKind::InvalidArg)
            )
        }
        ErrorKind::InvalidSubcommand => {
            format!(
                "subcomando no reconocido '{}'",
                param(ContextKind::InvalidSubcommand)
            )
        }
        ErrorKind::MissingSubcommand => "falta un subcomando".to_string(),
        ErrorKind::ArgumentConflict => {
            format!(
                "el argumento '{}' no se puede usar junto con '{}'",
                param(ContextKind::InvalidArg),
                param(ContextKind::PriorArg)
            )
        }
        ErrorKind::TooManyValues | ErrorKind::WrongNumberOfValues => {
            format!(
                "cantidad de valores incorrecta para '{}'",
                param(ContextKind::InvalidArg)
            )
        }
        ErrorKind::NoEquals => {
            format!(
                "'{}' requiere '=' para asignar su valor",
                param(ContextKind::InvalidArg)
            )
        }
        _ => unreachable!("tipos sin mapear delegan en err.exit()"),
    };
    let mut lines = vec![format!("Error: {message}")];
    for k in [
        ContextKind::SuggestedArg,
        ContextKind::SuggestedSubcommand,
        ContextKind::SuggestedValue,
    ] {
        if let Some(s) = err.get(k) {
            lines.push(format!("sugerencia: {s}"));
        }
    }
    if let Some(usage) = err.get(ContextKind::Usage) {
        let text = usage.to_string();
        let text = text.strip_prefix("Usage: ").unwrap_or(&text);
        lines.push(format!("Uso: {text}"));
    } else {
        // Sin uso del subcomando que falló: uso de la raíz como orientación
        // mínima, para que todo error en español incluya su línea `Uso:`.
        let mut root = localize_command(Cli::command());
        let text = root.render_usage().to_string();
        let text = text.strip_prefix("Usage: ").unwrap_or(&text);
        lines.push(format!("Uso: {text}"));
    }
    lines.push("Para más información, ejecuta '--help'.".to_string());
    eprintln!("{}", lines.join("\n"));
    // Herramienta de desarrollo sin scratch: no hay nada que liberar antes de salir.
    #[allow(clippy::disallowed_methods)]
    std::process::exit(err.exit_code());
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
    /// Genera o verifica SOURCE-OFFER.md
    SourceOffer {
        #[arg(long)]
        check: bool,
        /// Imprime el pie de fuentes de las notas del release TAG (p. ej. v0.39.0) en lugar de la oferta
        #[arg(long, value_name = "TAG", conflicts_with = "check")]
        notes: Option<String>,
    },
    /// Genera o verifica el inventario y los avisos de THIRD-PARTY-LICENSES.md
    Licenses {
        #[arg(long)]
        check: bool,
    },
    /// Genera o verifica la lista de funciones de ALSA que alcanza un binario Linux enlazado
    AlsaSymbols {
        #[arg(long)]
        check: bool,
        /// Binario Linux enlazado cuya tabla de símbolos define la lista
        #[arg(long, value_name = "FILE")]
        binary: PathBuf,
    },
    /// Corta una release: valida (licencias, fmt, clippy, árbol limpio, commits nuevos y pruebas con recursos) y luego hace el bump de versión + promoción de la sección [No publicado] del CHANGELOG
    Release {
        #[arg(value_name = "X.Y.Z")]
        version: String,
    },
    /// Verifica la sección del CHANGELOG para la versión actual
    Changelog {
        #[arg(long)]
        check: bool,
    },
    /// Verifica que el código de primera parte no tenga identificadores en español
    Language {
        #[arg(long)]
        check: bool,
        /// Sobrescribe la raíz a escanear (por defecto, el directorio actual)
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
    },
    /// Verifica que el código de primera parte no cite secciones ni localice líneas
    Comments {
        #[arg(long)]
        check: bool,
        /// Sobrescribe la raíz a escanear (por defecto, el directorio actual)
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
    },
    /// Verifica que todo archivo propio lleve la cabecera de licencia MPL-2.0
    Headers {
        #[arg(long)]
        check: bool,
        /// Sobrescribe la raíz a escanear (por defecto, el directorio actual)
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
    },
    /// Verifica que los pines versionados contra los parámetros de la CI y el canal de Rust
    Pins {
        #[arg(long)]
        check: bool,
        /// Regenera el canal de Rust desde packaging/pins.json
        #[arg(long)]
        sync: bool,
        /// Sobrescribe la raíz a escanear (por defecto, el directorio actual)
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
    },
    /// Verifica que todo feature que el código usa sea alcanzable desde el paquete raíz
    Features {
        #[arg(long)]
        check: bool,
        /// Sobrescribe la raíz a escanear (por defecto, el directorio actual)
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
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
    /// Limpia el entorno de desarrollo por capas (--repo por defecto)
    Clean {
        /// Solo la capa del repositorio: target/, ort-bundle/, motor y cobertura
        #[arg(long)]
        repo: bool,
        /// Solo la capa de la aplicación: delega en el binario del repositorio
        #[arg(long)]
        app: bool,
        /// Ambas capas: aplicación y después repositorio
        #[arg(long)]
        all: bool,
        /// Poda fina de la capa repo: solo cachés, cruces ajenos, PDBs viejos
        /// y sandboxes de prueba huérfanos, sin tocar la compilación vigente
        /// (el defecto no cambia)
        #[arg(long)]
        prune: bool,
        /// Capa global, nunca incluida en --all: cachés regenerables de Cargo
        /// (registry y git bajo CARGO_HOME) y, en Windows, la caché de pacman
        /// de MSYS2. Sola, no limpia el repositorio
        #[arg(long)]
        global: bool,
        /// Lista rutas y tamaños sin borrar nada
        #[arg(long)]
        dry_run: bool,
        /// Omite la confirmación interactiva
        #[arg(long)]
        yes: bool,
    },
    /// Verifica los requisitos del host y la deriva del entorno, sin modificar nada
    Doctor {
        /// Salida en JSON
        #[arg(long)]
        json: bool,
    },
    /// Verifica recursos completos del bootstrap, sin validar instalación de usuario
    VerifyResources {
        /// Salida en JSON
        #[arg(long)]
        json: bool,
    },
    /// Prepara o pone al día el entorno de desarrollo (convergente)
    Bootstrap {
        /// Ejecuta los gestores del sistema (única vía con sudo/UAC)
        #[arg(long)]
        system: bool,
        /// Provisiona los modelos con el build local
        #[arg(long)]
        models: bool,
        /// Omite la confirmación interactiva
        #[arg(long)]
        yes: bool,
    },
    /// Instala el build local por el camino del release (instalación script)
    Install {
        /// No provisionar modelos
        #[arg(long)]
        no_setup: bool,
    },
    /// Audita las dependencias de un bundle ya montado o extraído contra el contrato del manifiesto
    Deps {
        #[arg(long)]
        check: bool,
        /// Raíz del bundle a auditar
        #[arg(long, value_name = "DIR")]
        dir: PathBuf,
    },
    /// Empaqueta el bundle del release para el target del host desde el manifiesto
    Package {
        /// Directorio de salida del artefacto (por defecto, `artifacts`)
        #[arg(long, value_name = "DIR")]
        out: Option<PathBuf>,
        /// No comprimir: deja el árbol del bundle sin empaquetar
        #[arg(long)]
        no_compress: bool,
        /// Con `--no-compress`, escribe el árbol plano en `--out` en vez de
        /// dentro de un subdirectorio con el nombre del artefacto. Es el layout
        /// que espera un staging: el ejecutable cuelga directamente de la
        /// raíz, como lo dejan el bootstrap y `self update`.
        #[arg(long, requires = "no_compress")]
        flat: bool,
        /// Falla si la versión del CLI no coincide (puerta tag-versión de CI)
        #[arg(long, value_name = "X.Y.Z")]
        expect_version: Option<String>,
    },
    /// Aprovisiona el FFmpeg fijado en packaging/pins.json para el target del host
    Ffmpeg {
        /// Directorio del bundle (por defecto, `ffmpeg-bundle` en la raíz del repositorio)
        #[arg(long, value_name = "DIR")]
        dest: Option<PathBuf>,
        /// En lugar del binario, descarga a DIR las fuentes de FFmpeg que se publican con el release, verificadas contra sus hashes fijados
        #[arg(long, value_name = "DIR", conflicts_with = "dest")]
        sources: Option<PathBuf>,
    },
    /// Ejecuta la puerta de pruebas con recursos del release (ONNX Runtime y FFmpeg fijados, modelos aprovisionados) sin cortar versión
    ResourceGate,
}

fn main() -> Result<()> {
    // Bootstrap: consola en UTF-8 antes de cualquier salida.
    avi_shared::force_utf8_console();
    let cmd = localize_command(Cli::command());
    let matches = cmd
        .try_get_matches()
        .unwrap_or_else(|e| render_clap_error(e));
    let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|e| render_clap_error(e));
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
        Commands::SourceOffer { check, notes } => {
            if let Some(tag) = notes {
                let pins = doctor::load_pins(Path::new("."))?;
                print!("{}", render_release_notes_footer(&tag, &pins));
                return Ok(());
            }
            let version = get_version()?;
            if check {
                check_source_offer(&version)?;
                println!("SOURCE-OFFER.md en sincronía");
            } else {
                let (pins, mpl_crates) = source_offer_inputs()?;
                print!("{}", render_source_offer(&version, &pins, &mpl_crates)?);
            }
        }
        Commands::Licenses { check } => {
            if check {
                check_licenses_gate()?;
                println!(
                    "THIRD-PARTY-LICENSES.md está en sincronía con las dependencias distribuidas"
                );
            } else {
                write_licenses_regions()?;
                println!(
                    "Inventario y avisos de THIRD-PARTY-LICENSES.md regenerados: revisa el diff"
                );
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
            // Pre-validación atómica: abortar antes de mutar si las licencias están
            // desincronizadas, si el código no pasa `cargo fmt` o clippy, si el árbol
            // está sucio, si no hay commits nuevos desde el último tag o si fallan las
            // pruebas con recursos.
            {
                check_licenses_gate()?;
                check_code_quality_gate()?;
                let last = last_tag()?;
                let diff_status = doctor::probe_command("git")
                    .args(["diff", "--quiet"])
                    .status()?;
                let diff_cached_status = doctor::probe_command("git")
                    .args(["diff", "--cached", "--quiet"])
                    .status()?;
                if !diff_status.success() || !diff_cached_status.success() {
                    anyhow::bail!(
                        "working tree con cambios pendientes: commitea o stashea antes de release (git status debe estar limpio)"
                    );
                }
                let log_out = doctor::probe_command("git")
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
                // Última puerta de solo lectura (la más lenta): pruebas con recursos.
                check_resource_tests_gate()?;
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
            check_licenses_gate().map_err(post_check)?;
            check_source_offer(version).map_err(post_check)?;
            let changelog_text = std::fs::read_to_string("CHANGELOG.md")?;
            validate_changelog_text(&changelog_text, version).map_err(post_check)?;

            println!("Release {} preparado y verificado:", version);
            println!("  - src/main.rs (VERSION)");
            println!("  - Cargo.toml (package.version)");
            println!("  - Cargo.lock (ai-voice-interconnector)");
            println!("  - tests/golden/cli_version.json");
            println!("  - SOURCE-OFFER.md (aviso MPL-2.0 §3.2 versionado)");
            println!("  - CHANGELOG.md (sección promovida desde [No publicado] + ToC + enlace)");
            println!(
                "Comprobaciones: formato (cargo fmt), clippy sin avisos, pruebas con recursos (--include-ignored); licencias, SOURCE-OFFER.md y CHANGELOG.md en sincronía"
            );
            println!(
                "Revisa el diff, commitea con conventional-commits y crea el tag v{}",
                version
            );
        }
        Commands::Changelog { check } => {
            if check {
                check_changelog()?;
                println!("CHANGELOG.md en sincronía con la versión actual");
            } else {
                println!("Usa --check para verificar la sección del CHANGELOG");
            }
        }
        Commands::AlsaSymbols { check, binary } => alsa_symbols::run(check, &binary)?,
        Commands::Language { check, root } => language::run(check, root.as_deref())?,
        Commands::Comments { check, root } => comments::run(check, root.as_deref())?,
        Commands::Headers { check, root } => headers::run(check, root.as_deref())?,
        Commands::Pins { check, sync, root } => pins::run(check, sync, root.as_deref())?,
        Commands::Features { check, root } => features::run(check, root.as_deref())?,
        Commands::Deps { check, dir } => deps::run(check, &dir)?,
        Commands::BuildEngine {
            self_test,
            simd,
            jobs,
        } => {
            build_engine(self_test, simd, jobs)?;
        }
        Commands::Clean {
            repo,
            app,
            all,
            prune,
            global,
            dry_run,
            yes,
        } => {
            let layer = clean::Layer::select(repo, app, all, global);
            clean::run(layer, global, dry_run, yes, prune)?
        }
        Commands::Doctor { json } => doctor::run(json)?,
        Commands::VerifyResources { json } => doctor::verify_resources(json)?,
        Commands::Bootstrap {
            system,
            models,
            yes,
        } => bootstrap::run(system, models, yes)?,
        Commands::Install { no_setup } => install::run(no_setup)?,
        Commands::Package {
            out,
            no_compress,
            flat,
            expect_version,
        } => package::run(out, no_compress, flat, expect_version)?,
        Commands::Ffmpeg { dest, sources } => {
            package::run_ffmpeg(dest.as_deref(), sources.as_deref())?
        }
        Commands::ResourceGate => check_resource_tests_gate()?,
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
    // En Windows el motor solo se compila con los paquetes MSYS2 fijados: la misma regla
    // que `doctor`, aplicada aquí para que local y CI no puedan enlazar otra OpenBLAS.
    #[cfg(windows)]
    {
        let pins = doctor::load_pins(Path::new("."))?;
        let row = doctor::check_engine_toolchain(&pins);
        if row.status != doctor::Status::Ok {
            anyhow::bail!(
                "la toolchain del motor TTS no coincide con packaging/pins.json: {} (esperado: {}){}",
                row.found.as_deref().unwrap_or("no encontrada"),
                row.want.as_deref().unwrap_or("los pines"),
                row.hint
                    .as_deref()
                    .map(|hint| format!(". {hint}"))
                    .unwrap_or_default()
            );
        }
    }
    let dir = engine_dir();
    let simd = simd.unwrap_or_else(|| "auto".to_string());
    let jobs = jobs.unwrap_or_else(|| match std::thread::available_parallelism() {
        Ok(n) => n.get(),
        Err(e) => {
            eprintln!(
                "aviso: no se pudo determinar el paralelismo disponible ({e}); se compila con -j1"
            );
            1
        }
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

pub(crate) fn get_version() -> Result<String> {
    // La versión vive en el `[package]` de Cargo.toml.
    let text = std::fs::read_to_string("Cargo.toml")
        .map_err(|e| anyhow!("no se pudo leer Cargo.toml: {}", e))?;
    Regex::new(r#"\[package\][^\[]*?version\s*=\s*"([^"]+)""#)
        .unwrap()
        .captures(&text)
        .map(|m| m[1].to_string())
        .ok_or_else(|| anyhow!("Cargo.toml no declara la versión en [package]"))
}

/// Descripción corta del producto, declarada en el `[package]` del manifiesto.
fn package_description(manifest: &str) -> Result<String> {
    Regex::new(r#"\[package\][^\[]*?description\s*=\s*"([^"]+)""#)
        .unwrap()
        .captures(manifest)
        .map(|m| m[1].to_string())
        .ok_or_else(|| anyhow!("Cargo.toml no declara la descripción en [package]"))
}

/// URL de un asset publicado en el release de `version`.
fn release_asset_url(version: &str, asset: &str) -> String {
    format!("https://github.com/{GITHUB_REPO}/releases/download/v{version}/{asset}")
}

/// Nombres de los assets de fuentes y de configuración de build de FFmpeg que acompañan
/// al release: las fuentes de `package::ffmpeg_sources` y un archivo de configuración por
/// cada triple con binario fijado.
fn ffmpeg_release_assets(pins: &Pins) -> (Vec<String>, Vec<String>) {
    let sources = package::ffmpeg_sources(&pins.ffmpeg)
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    let buildconfs = pins
        .ffmpeg_sha256
        .keys()
        .map(|triple| package::ffmpeg_buildconf_name(&pins.ffmpeg, triple))
        .collect();
    (sources, buildconfs)
}

/// Lista Markdown de enlaces a los assets del release de `version`.
fn asset_links(version: &str, assets: &[String]) -> String {
    assets
        .iter()
        .map(|asset| format!("- `{asset}`: <{}>", release_asset_url(version, asset)))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Crates de terceros que se distribuyen bajo MPL-2.0 como `(nombre, versión)`: su código
/// fuente lo sirve crates.io y la oferta lo nombra.
fn mpl_third_party_crates(packages: &[DistPackage]) -> Result<Vec<(String, String)>> {
    let mut crates = Vec::new();
    for package in packages {
        let Some(expression) = package.license.as_deref() else {
            continue;
        };
        let required = notices::required_licenses(expression)
            .map_err(|e| anyhow!("{} {}: {}", package.name, package.version, e))?;
        if required.iter().any(|license| license.id == "MPL-2.0") {
            crates.push((package.name.clone(), package.version.clone()));
        }
    }
    Ok(crates)
}

/// Oferta de código fuente de `version`. Todo lo variable sale de su fuente única: la
/// versión y los hashes de FFmpeg de `pins.json`, el build y las URLs de `package.rs` y
/// los crates MPL-2.0 de terceros del grafo distribuido.
fn render_source_offer(
    version: &str,
    pins: &Pins,
    mpl_crates: &[(String, String)],
) -> Result<String> {
    let (sources, buildconfs) = ffmpeg_release_assets(pins);
    let mut binaries = Vec::new();
    for triple in pins.ffmpeg_sha256.keys() {
        let (_, url) = package::ffmpeg_download(triple, &pins.ffmpeg)?;
        binaries.push(format!("- {}: <{url}>", notices::platform_name(triple)));
    }
    let mpl_section = if mpl_crates.is_empty() {
        String::new()
    } else {
        let list: Vec<String> = mpl_crates
            .iter()
            .map(|(name, crate_version)| {
                format!(
                    "- `{name}` {crate_version}: <https://crates.io/crates/{name}/{crate_version}>"
                )
            })
            .collect();
        format!(
            "\n## Crates de terceros bajo MPL-2.0\n\n\
             El binario enlaza estos crates de terceros, también bajo MPL-2.0; su código fuente \
             exacto se publica en crates.io:\n\n{}\n",
            list.join("\n")
        )
    };
    Ok(SOURCE_OFFER_TEMPLATE
        .replace("{mpl_crates}", &mpl_section)
        .replace("{ffmpeg_sources}", &asset_links(version, &sources))
        .replace("{ffmpeg_binaries}", &binaries.join("\n"))
        .replace("{ffmpeg_buildconfs}", &asset_links(version, &buildconfs))
        .replace("{ffmpeg_version}", &pins.ffmpeg)
        .replace("{ffmpeg_btbn_build}", package::FFMPEG_BTBN_BUILD)
        .replace("{ffmpeg_btbn_tag}", package::FFMPEG_BTBN_TAG)
        .replace("{version}", version)
        .replace("{repo}", GITHUB_REPO))
}

/// Pie de las notas del release `tag`: el tarball de fuentes del tag y los assets de
/// fuentes de FFmpeg. Lo imprime `source-offer --notes` y lo anexa el job de publicación,
/// de modo que las notas y `SOURCE-OFFER.md` salen de las mismas fuentes.
fn render_release_notes_footer(tag: &str, pins: &Pins) -> String {
    let version = tag.trim_start_matches('v');
    let (sources, buildconfs) = ffmpeg_release_assets(pins);
    format!(
        "\n---\n\n\
         ## Código fuente (MPL-2.0 §3.2)\n\n\
         Este release se distribuye bajo **MPL-2.0**. El código fuente correspondiente a este tag \
         está disponible en:\n\n\
         - Tarball de fuente: https://github.com/{GITHUB_REPO}/archive/refs/tags/{tag}.tar.gz\n\
         - Etiqueta del release: https://github.com/{GITHUB_REPO}/releases/tag/{tag}\n\n\
         Las fuentes de FFmpeg {} (GPL-3.0-or-later) y la configuración de build de cada binario \
         se publican como assets de este release:\n\n{}\n{}\n",
        pins.ffmpeg,
        asset_links(version, &sources),
        asset_links(version, &buildconfs)
    )
}

/// Pines y crates MPL-2.0 de terceros con los que se renderiza la oferta.
fn source_offer_inputs() -> Result<(Pins, Vec<(String, String)>)> {
    let pins = doctor::load_pins(Path::new("."))?;
    let mpl_crates = mpl_third_party_crates(&load_distributed_packages()?)?;
    Ok((pins, mpl_crates))
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
        "SOURCE-OFFER.md desincronizado: regenera con `cargo xtask source-offer > SOURCE-OFFER.md`\n",
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
    let (pins, mpl_crates) = source_offer_inputs()?;
    let rendered = render_source_offer(version, &pins, &mpl_crates)?;
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

    // SOURCE-OFFER.md: aviso MPL-2.0 §3.2 versionado
    let (pins, mpl_crates) = source_offer_inputs()?;
    let offer = render_source_offer(version, &pins, &mpl_crates)?;
    std::fs::write(Path::new("SOURCE-OFFER.md"), offer)?;

    Ok(())
}

/// Resuelve el último tag anotado en el repositorio.
fn last_tag() -> Result<String> {
    let output = doctor::probe_command("git")
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
    let date = today_iso()?;
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

/// Fecha actual como YYYY-MM-DD, obtenida con el comando `date`. Falla si el
/// comando no existe, sale con error o no devuelve una fecha con ese formato.
fn today_iso() -> Result<String> {
    let out = std::process::Command::new("date")
        .arg("+%Y-%m-%d")
        .output()
        .map_err(|e| anyhow!("no se pudo ejecutar `date` para obtener la fecha: {}", e))?;
    if !out.status.success() {
        anyhow::bail!("`date +%Y-%m-%d` falló (exit {:?})", out.status.code());
    }
    parse_iso_date(&String::from_utf8(out.stdout)?)
}

/// Valida que `text` (salida de `date`) sea una fecha YYYY-MM-DD y la devuelve
/// sin espacios sobrantes.
fn parse_iso_date(text: &str) -> Result<String> {
    let date = text.trim();
    if !Regex::new(r"^\d{4}-\d{2}-\d{2}$").unwrap().is_match(date) {
        anyhow::bail!(
            "la fecha obtenida no tiene el formato YYYY-MM-DD: '{}'",
            date
        );
    }
    Ok(date.to_string())
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

/// Entradas `zap trash:` del Cask derivadas de las canónicas de `avi-shared`:
/// la raíz de modelos **exclusiva** de la
/// aplicación, el directorio de soporte de la aplicación, un snapshot por
/// modelo pineado en `MODEL_REVISIONS`. Sin `xet` ni
/// `.locks` globales: en una caché compartida esos directorios no son
/// atribuibles a la aplicación.
fn cask_zap_entries() -> Vec<String> {
    // La raíz de modelos encabeza la lista porque el canal Cask responde
    // `externally_managed` a `self uninstall`: este `zap` es su única vía de
    // borrado, y los modelos viven en la raíz exclusiva de
    // `models_cache_dir()`. Va en forma `~`
    // porque el Cask se genera en la máquina que construye y se ejecuta en la
    // del usuario; los segmentos son los de la columna de modelos de
    // `models_cache_dir()` para macOS (`Library`, `Caches`, `APP_NAME`,
    // `models`).
    let mut entries = vec![
        format!("~/Library/Caches/{}/models", avi_shared::paths::APP_NAME),
        format!(
            "~/Library/Application Support/{}",
            avi_shared::paths::APP_NAME
        ),
    ];
    for pin in avi_shared::paths::MODEL_REVISIONS {
        entries.push(format!(
            "~/.cache/huggingface/hub/models--{}",
            pin.repo.replace('/', "--")
        ));
    }
    entries
}

fn render_cask(version: &str, sha256: &str) -> Result<String> {
    let zap = cask_zap_entries()
        .iter()
        .map(|entry| format!("    \"{entry}\","))
        .collect::<Vec<_>>()
        .join("\n");
    Ok(CASK_TEMPLATE
        .replace("{cask_name}", CASK_NAME)
        .replace("{cask_version}", version)
        .replace("{cask_sha256}", sha256)
        .replace("{cask_desc}", &package_description(ROOT_MANIFEST)?)
        .replace("{repo}", GITHUB_REPO)
        .replace("{zap_entries}", &zap))
}

fn render_cask_from_tag(circle_tag: &str, sums_text: &str) -> Result<String> {
    let version = circle_tag.trim_start_matches('v');
    let sha256 = parse_macos_sha256(sums_text, version)?;
    render_cask(version, &sha256)
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

const LICENSES_DOC: &str = "THIRD-PARTY-LICENSES.md";
const INVENTORY_START: &str = "<!-- inventario:inicio -->";
const INVENTORY_END: &str = "<!-- inventario:fin -->";

/// Paquete que viaja en el binario distribuido: lo alcanza una arista de dependencia
/// normal desde el paquete raíz para alguno de los triples del manifiesto del paquete.
struct DistPackage {
    name: String,
    version: String,
    /// Expresión SPDX del `Cargo.toml`; `None` si el paquete no declara `license`.
    license: Option<String>,
    /// Directorio del paquete, donde viven sus archivos de licencia.
    manifest_dir: PathBuf,
}

/// Licencia del propio proyecto: los paquetes propios que la declaran se cubren con el
/// `LICENSE` de la raíz y no se inventarían como terceros.
const PROJECT_LICENSE: &str = "MPL-2.0";

/// Paquetes distribuidos a partir de la salida de `cargo metadata` de cada triple,
/// ordenados por nombre y versión y unidos por `(nombre, versión)`.
///
/// Se recorre el grafo `resolve` desde el paquete raíz solo por aristas de dependencia
/// normal (los proc-macros lo son): las dependencias de desarrollo y de compilación no
/// forman parte de lo que se distribuye. Los paquetes del espacio de trabajo con la
/// licencia del proyecto quedan fuera; los demás (p. ej. `denoise`, bajo MIT) se
/// inventarían.
fn distributed_packages(metadata: &[serde_json::Value]) -> Result<Vec<DistPackage>> {
    let mut union: BTreeMap<(String, String), DistPackage> = BTreeMap::new();
    for json in metadata {
        let packages = json["packages"]
            .as_array()
            .ok_or_else(|| anyhow!("cargo metadata no devolvió `packages`"))?;
        let nodes = json["resolve"]["nodes"]
            .as_array()
            .ok_or_else(|| anyhow!("cargo metadata no devolvió `resolve.nodes`"))?;
        let root = json["resolve"]["root"]
            .as_str()
            .ok_or_else(|| anyhow!("cargo metadata no devolvió `resolve.root`"))?;
        let workspace: HashSet<&str> = json["workspace_members"]
            .as_array()
            .map(|members| members.iter().filter_map(|m| m.as_str()).collect())
            .unwrap_or_default();
        let by_id: HashMap<&str, &serde_json::Value> = packages
            .iter()
            .filter_map(|p| p["id"].as_str().map(|id| (id, p)))
            .collect();
        let edges: HashMap<&str, Vec<&str>> = nodes
            .iter()
            .filter_map(|node| {
                let id = node["id"].as_str()?;
                let normal = node["deps"]
                    .as_array()?
                    .iter()
                    .filter(|dep| {
                        dep["dep_kinds"]
                            .as_array()
                            .is_some_and(|kinds| kinds.iter().any(|kind| kind["kind"].is_null()))
                    })
                    .filter_map(|dep| dep["pkg"].as_str())
                    .collect();
                Some((id, normal))
            })
            .collect();
        let mut seen: HashSet<&str> = HashSet::new();
        let mut pending = vec![root];
        while let Some(id) = pending.pop() {
            if !seen.insert(id) {
                continue;
            }
            pending.extend(edges.get(id).into_iter().flatten().copied());
        }
        for id in seen {
            let package = by_id
                .get(id)
                .ok_or_else(|| anyhow!("el grafo cita el paquete {id} que `packages` no lista"))?;
            let license = package["license"].as_str().map(str::to_string);
            if workspace.contains(id) && license.as_deref() == Some(PROJECT_LICENSE) {
                continue;
            }
            let (Some(name), Some(version), Some(manifest)) = (
                package["name"].as_str(),
                package["version"].as_str(),
                package["manifest_path"].as_str(),
            ) else {
                anyhow::bail!(
                    "cargo metadata devolvió un paquete sin nombre, versión o manifiesto"
                );
            };
            let manifest_dir = Path::new(manifest)
                .parent()
                .ok_or_else(|| anyhow!("el manifiesto {manifest} no tiene directorio"))?
                .to_path_buf();
            union
                .entry((name.to_string(), version.to_string()))
                .or_insert(DistPackage {
                    name: name.to_string(),
                    version: version.to_string(),
                    license,
                    manifest_dir,
                });
        }
    }
    Ok(union.into_values().collect())
}

/// Familia de una expresión de licencia: `MIT` si MIT (o su variante MIT-0) es
/// alternativa o componente; `Apache-2.0` si no hay MIT pero sí Apache; `BSD`
/// para BSD-2/3-Clause; la propia licencia en los demás casos. Tabla explícita
/// con las expresiones de los paquetes distribuidos: `None` ante una expresión
/// desconocida, que exige decidir su familia a mano.
fn license_family(expr: &str) -> Option<&'static str> {
    let family = match expr {
        "MIT"
        | "MIT OR Apache-2.0"
        | "Apache-2.0 OR MIT"
        | "MIT/Apache-2.0"
        | "Apache-2.0/MIT"
        | "Apache-2.0 / MIT"
        | "Unlicense/MIT"
        | "Unlicense OR MIT"
        | "Unlicense OR MIT OR Apache-2.0 OR CC0-1.0"
        | "MIT AND BSD-3-Clause"
        | "MIT OR Zlib OR Apache-2.0"
        | "MIT OR Apache-2.0 OR Zlib"
        | "Apache-2.0 OR MIT OR Zlib"
        | "Zlib OR Apache-2.0 OR MIT"
        | "Apache-2.0 OR BSL-1.0 OR MIT"
        | "Apache-2.0 OR ISC OR MIT"
        | "Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT"
        | "BSD-2-Clause OR Apache-2.0 OR MIT"
        | "BSD-2-Clause OR MIT OR Apache-2.0"
        | "(Apache-2.0 OR MIT) AND BSD-3-Clause"
        | "(MIT OR Apache-2.0) AND Unicode-3.0"
        | "CC0-1.0 OR MIT-0 OR Apache-2.0"
        | "0BSD OR MIT OR Apache-2.0"
        | "ISC AND (Apache-2.0 OR ISC) AND Apache-2.0 AND MIT AND BSD-3-Clause AND (Apache-2.0 OR ISC OR MIT) AND (Apache-2.0 OR ISC OR MIT-0)" => "MIT",
        "Apache-2.0"
        | "Apache-2.0 AND ISC"
        | "Apache-2.0 OR BSL-1.0"
        | "ISC AND (Apache-2.0 OR ISC)"
        | "CC0-1.0 OR Apache-2.0 OR Apache-2.0 WITH LLVM-exception" => "Apache-2.0",
        "BSD-2-Clause" | "BSD-3-Clause" => "BSD",
        "ISC" => "ISC",
        "Zlib" => "Zlib",
        "MPL-2.0" => "MPL-2.0",
        "Unicode-3.0" => "Unicode-3.0",
        "CDLA-Permissive-2.0" => "CDLA-Permissive-2.0",
        _ => return None,
    };
    Some(family)
}

/// Región del inventario (conteo, resumen por familia y tabla) de los paquetes
/// distribuidos. Pura: falla nombrando el paquete si no declara licencia o si su
/// familia es desconocida.
fn render_licenses_inventory(packages: &[DistPackage]) -> Result<String> {
    let mut rows = String::new();
    let mut families: BTreeMap<&str, usize> = BTreeMap::new();
    let mut names = HashSet::new();
    for package in packages {
        let (name, version) = (&package.name, &package.version);
        let license = package.license.as_deref().ok_or_else(|| {
            anyhow!(
                "{} {} no declara `license` en su Cargo.toml: no hay licencia que registrar",
                name,
                version
            )
        })?;
        let family = license_family(license).ok_or_else(|| {
            anyhow!(
                "{} {}: expresión de licencia sin familia `{}`: añádela a license_family",
                name,
                version,
                license
            )
        })?;
        *families.entry(family).or_default() += 1;
        names.insert(name.as_str());
        rows.push_str(&format!(
            "| `{}` | {} | {} | {} |\n",
            name, version, license, family
        ));
    }
    let summary: Vec<String> = families
        .iter()
        .map(|(family, count)| format!("{} {}", family, count))
        .collect();
    Ok(format!(
        "Generado desde el grafo de dependencias distribuidas ({} paquetes resueltos, {} crates únicos): \
         dependencias normales de `ai-voice-interconnector` con `--features full`, para los triples de \
         `packaging/bundle-manifest.json`, sin dependencias de desarrollo ni de compilación.\n\
         Resumen por familia (paquetes resueltos): {}.\n\
         \n\
         | Paquete | Versión | Licencia (metadato) | Familia |\n\
         |---------|---------|---------------------|--------|\n\
         {}",
        packages.len(),
        names.len(),
        summary.join(", "),
        rows
    ))
}

/// Reemplaza el contenido entre los marcadores `start_marker` y `end_marker` por
/// `region`, conservando el texto exterior. Falla si falta alguno de los marcadores.
fn replace_region(doc: &str, start_marker: &str, end_marker: &str, region: &str) -> Result<String> {
    let start = doc
        .find(start_marker)
        .ok_or_else(|| anyhow!("{} no tiene el marcador `{}`", LICENSES_DOC, start_marker))?
        + start_marker.len();
    let end = doc[start..]
        .find(end_marker)
        .map(|i| start + i)
        .ok_or_else(|| anyhow!("{} no tiene el marcador `{}`", LICENSES_DOC, end_marker))?;
    Ok(format!("{}\n{}{}", &doc[..start], region, &doc[end..]))
}

/// Documento con las dos regiones generadas (inventario y avisos) sustituidas.
fn replace_licenses_regions(doc: &str, inventory: &str, notices: &str) -> Result<String> {
    let with_inventory = replace_region(doc, INVENTORY_START, INVENTORY_END, inventory)?;
    replace_region(
        &with_inventory,
        notices::NOTICES_START,
        notices::NOTICES_END,
        notices,
    )
}

/// Compara `current` (contenido vigente de THIRD-PARTY-LICENSES.md) contra el
/// documento con las regiones `inventory` y `notices` recién generadas,
/// normalizando CRLF vs LF. Pura. `Ok(())` si coinciden; si no, un mensaje
/// accionable con diff.
fn diff_licenses_doc(current: &str, inventory: &str, notices: &str) -> Result<(), String> {
    let norm_current = current.replace("\r\n", "\n");
    let expected =
        replace_licenses_regions(&norm_current, inventory, notices).map_err(|e| e.to_string())?;
    if norm_current == expected {
        return Ok(());
    }
    let mut msg = format!(
        "{} desincronizado: regenera con `cargo xtask licenses` y revisa el diff\n",
        LICENSES_DOC
    );
    for line in diff_lines(&norm_current, &expected) {
        msg.push_str(&line);
        msg.push('\n');
    }
    Err(msg)
}

/// Paquetes distribuidos del repositorio: el grafo de `cargo metadata --features full`
/// de cada triple del manifiesto del paquete, recorrido por [`distributed_packages`].
fn load_distributed_packages() -> Result<Vec<DistPackage>> {
    let manifest_text = std::fs::read_to_string(package::MANIFEST_REL)
        .map_err(|e| anyhow!("no se pudo leer {}: {}", package::MANIFEST_REL, e))?;
    let targets = avi_shared::manifest::parse_manifest_text(&manifest_text)
        .map_err(|e| anyhow!("{}: {}", package::MANIFEST_REL, e))?;
    let mut metadata = Vec::new();
    for triple in targets.keys() {
        let out = std::process::Command::new("cargo")
            .args([
                "metadata",
                "--format-version",
                "1",
                "--locked",
                "--features",
                "full",
                "--filter-platform",
                triple,
            ])
            .output()
            .map_err(|e| anyhow!("no se pudo invocar cargo metadata: {}", e))?;
        if !out.status.success() {
            anyhow::bail!(
                "cargo metadata falló para {}: {}",
                triple,
                String::from_utf8_lossy(&out.stderr)
            );
        }
        metadata.push(serde_json::from_slice(&out.stdout)?);
    }
    distributed_packages(&metadata)
}

/// Carpeta de compilación que informa `cargo metadata` (`target_directory`).
/// Así respeta `CARGO_TARGET_DIR` y `build.target-dir` sin replicar la lógica
/// de resolución de cargo.
fn cargo_target_dir() -> Result<PathBuf> {
    let out = std::process::Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .output()
        .map_err(|e| anyhow!("no se pudo invocar cargo metadata: {}", e))?;
    if !out.status.success() {
        anyhow::bail!(
            "cargo metadata falló: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let json: serde_json::Value = serde_json::from_slice(&out.stdout)?;
    json["target_directory"]
        .as_str()
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("cargo metadata no devolvió `target_directory`"))
}

/// Regiones del inventario y de los avisos, generadas desde el mismo grafo de
/// dependencias distribuidas.
fn render_licenses_regions() -> Result<(String, String)> {
    let packages = load_distributed_packages()?;
    let pins = doctor::load_pins(Path::new("."))?;
    Ok((
        render_licenses_inventory(&packages)?,
        notices::render_notices(Path::new("."), &packages, &pins.ort)?,
    ))
}

/// Reescribe las regiones de inventario y de avisos de THIRD-PARTY-LICENSES.md.
fn write_licenses_regions() -> Result<()> {
    let (inventory, notices) = render_licenses_regions()?;
    let doc = std::fs::read_to_string(LICENSES_DOC)?.replace("\r\n", "\n");
    std::fs::write(
        LICENSES_DOC,
        replace_licenses_regions(&doc, &inventory, &notices)?,
    )?;
    Ok(())
}

/// Gate de licencias: falla con el diff si las regiones de inventario o de avisos de
/// `THIRD-PARTY-LICENSES.md` difieren de las generadas (paquetes, versiones,
/// licencias, familias, totales o textos). Envoltura de E/S sobre `diff_licenses_doc`.
fn check_licenses_gate() -> Result<()> {
    let (inventory, notices) = render_licenses_regions()?;
    let current = std::fs::read_to_string(LICENSES_DOC)?;
    diff_licenses_doc(&current, &inventory, &notices).map_err(|msg| anyhow!(msg))
}

/// Gate de calidad de código: `cargo fmt --all --check` y
/// `cargo clippy --all-targets -- -D warnings`, con la salida heredada para que
/// el diff o los avisos queden visibles.
fn check_code_quality_gate() -> Result<()> {
    let fmt_status = std::process::Command::new("cargo")
        .args(["fmt", "--all", "--check"])
        .status()?;
    if !fmt_status.success() {
        anyhow::bail!(
            "código sin formatear: ejecuta `cargo fmt --all` y commitea el resultado antes de release"
        );
    }
    let clippy_status = std::process::Command::new("cargo")
        .args(["clippy", "--all-targets", "--", "-D", "warnings"])
        .status()?;
    if !clippy_status.success() {
        anyhow::bail!(
            "clippy reporta avisos: corrígelos y commitea antes de release (`cargo clippy --all-targets -- -D warnings`)"
        );
    }
    Ok(())
}

/// Gate de pruebas con recursos: ejecuta toda la suite del workspace, incluidas
/// las pruebas marcadas como `#[ignore]` (modelos, daemon, instalación real),
/// con el entorno y la salida heredados. No tiene modo de omisión.
///
/// La gate asegura los recursos fijados (ONNX Runtime en `ort-bundle/` y FFmpeg
/// en `ffmpeg-bundle/`) con `ensure_pinned_bundles`, la misma función que usa
/// `bootstrap`; por eso, tras un `bootstrap`, ese aseguramiento no descarga
/// nada. Las pruebas cargan ONNX Runtime desde `ORT_DYLIB_PATH`: la gate
/// exporta la ruta del fichero de la librería, que prevalece sobre el valor por
/// defecto de la configuración de cargo, de modo que valida el mismo runtime
/// que se distribuye.
///
/// Además habilita la capa `<cwd>/vendor` (`AVI_ALLOW_CWD_VENDOR=1`): las
/// pruebas se ejecutan desde la raíz del repo y los hijos CLI que lanzan
/// resuelven el motor recién compilado en `vendor/qwen3-tts/`, sin exigir un
/// programa instalado. Sin este opt-in, un entorno limpio falla las 21 pruebas
/// del motor aunque todo esté provisionado.
///
/// La CLI solo busca FFmpeg junto a su propio ejecutable, así que la gate lo copia a la
/// carpeta de binarios de prueba (`<target_directory>/debug`); y como las
/// pruebas del crate `denoise` lo resuelven por `PATH`, antepone la carpeta
/// del bundle al `PATH` del proceso de pruebas, igual que la CI. Ni una ni otra
/// vía depende del FFmpeg del sistema. `xtask release` y `xtask resource-gate`
/// comparten esta función, de modo que el ensayo reproduce la gate exacta.
fn check_resource_tests_gate() -> Result<()> {
    let root = std::env::current_dir()?;
    let pins = doctor::load_pins(&root)?;
    let triple = package::host_triple()?;
    let bundles = package::ensure_pinned_bundles(&root, triple, &pins)?;
    let ort_dylib = package::ort_dylib_path(&bundles.ort, triple)?;
    package::stage_ffmpeg_beside(&bundles.ffmpeg, &cargo_target_dir()?.join("debug"), triple)?;
    let status = std::process::Command::new("cargo")
        .env("ORT_DYLIB_PATH", &ort_dylib)
        .env("AVI_ALLOW_CWD_VENDOR", "1")
        .env(
            "PATH",
            package::path_with_prepended(&bundles.ffmpeg, std::env::var_os("PATH").as_deref())?,
        )
        .args([
            "test",
            "--workspace",
            "--features",
            "full",
            "--",
            "--include-ignored",
        ])
        .status()?;
    if !status.success() {
        anyhow::bail!(
            "las pruebas con recursos fallaron y no se ha modificado nada: corrígelas antes de release (reprodúcelo con `cargo xtask resource-gate`)"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ambos binarios arrancan con el helper compartido de consola y el
    /// producto ya no duplica la función local.
    #[test]
    fn test_console_bootstrap_uses_shared_helper() {
        let xtask_main = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/main.rs");
        let product_main = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../src/main.rs");
        // La aguja se compone por partes para que este mismo test no la
        // contenga literalmente y dispare su propia comprobación.
        let local: String = ["fn force_utf8", "("].concat();
        for main in [&xtask_main, &product_main] {
            let text = std::fs::read_to_string(main).unwrap();
            assert!(
                text.contains("avi_shared::force_utf8_console"),
                "{} debe llamar al helper compartido",
                main.display()
            );
            assert!(
                !text.contains(&local),
                "{} no debe duplicar la función local",
                main.display()
            );
        }
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
        let c = render_cask("1.2.3", &"a".repeat(64)).unwrap();
        assert!(c.contains(r#"version "1.2.3""#));
        assert!(c.contains(&format!(r#"sha256 "{}""#, "a".repeat(64))));
        assert!(c.contains("ai-voice-interconnector-#{version}-arm64-macos.tar.gz"));
        assert!(c.contains(r#"cask "ai-voice-interconnector" do"#));
        assert!(c.contains(r#"binary "ai-voice-interconnector""#));
        assert!(!c.contains("\n  app "));
        assert!(c.contains("releases/download/v#{version}/"));
        assert!(c.contains("zap trash:"));
        // El `zap` deriva de las canónicas: un snapshot por modelo pineado.
        for pin in avi_shared::paths::MODEL_REVISIONS {
            let snapshot = format!("models--{}", pin.repo.replace('/', "--"));
            assert!(
                c.contains(&snapshot),
                "el zap debe derivar el snapshot canónico {snapshot}"
            );
        }
        // Los recursos globales de la caché HF compartida no son de la aplicación:
        // el zap no los nombra.
        assert!(!c.contains("~/.cache/huggingface/xet"));
        assert!(!c.contains("~/.cache/huggingface/hub/.locks"));
        assert!(c.contains("MPL-2.0"));
        // El mínimo del Cask es el que el job de macOS fija con
        // MACOSX_DEPLOYMENT_TARGET (macOS 26, Tahoe): Homebrew no debe
        // instalar el binario donde dyld no lo cargaría.
        assert!(c.contains(r#"depends_on macos: ">= :tahoe""#));
        let desc = package_description(ROOT_MANIFEST).unwrap();
        assert!(c.contains(&format!(r#"desc "{desc}""#)));
    }

    /// `brew audit` rechaza un `desc` de más de 80 caracteres o terminado en
    /// punto, y el Cask lo toma de la descripción del manifiesto raíz.
    #[test]
    fn test_package_description_fits_homebrew() {
        let desc = package_description(ROOT_MANIFEST).unwrap();
        assert!(
            desc.chars().count() <= 80,
            "la descripción supera 80 caracteres: {desc}"
        );
        assert!(
            !desc.ends_with('.'),
            "la descripción termina en punto: {desc}"
        );
    }

    #[test]
    fn test_package_description_requires_field() {
        let manifest = "[package]\nname = \"x\"\nversion = \"1.0.0\"\n\n[workspace]\n";
        assert!(package_description(manifest).is_err());
    }

    /// El mínimo de macOS se fija una sola vez, con MACOSX_DEPLOYMENT_TARGET en
    /// el job `build-darwin-arm64`, y lo repiten el `depends_on` del Cask y el
    /// diagnóstico de `install.sh`: si el pipeline lo sube y alguno de los dos
    /// no, esta prueba falla.
    #[test]
    fn test_macos_minimum_matches_deployment_target() {
        let cfg = include_str!("../../../.circleci/config.yml");
        let target = cfg
            .lines()
            .find_map(|l| l.trim().strip_prefix("MACOSX_DEPLOYMENT_TARGET: \""))
            .and_then(|v| v.strip_suffix('"'))
            .expect("el pipeline fija MACOSX_DEPLOYMENT_TARGET");
        let major = target.split('.').next().unwrap_or_default();
        let symbol = match major {
            "26" => "tahoe",
            other => panic!("falta el símbolo de Homebrew para macOS {other}"),
        };
        let cask = render_cask("9.9.9", &"b".repeat(64)).unwrap();
        assert!(cask.contains(&format!(r#"depends_on macos: ">= :{symbol}""#)));
        let install_sh = include_str!("../../../packaging/bootstrap/install.sh");
        assert!(
            install_sh.contains(&format!("versión anterior a macOS {major} ")),
            "el diagnóstico de install.sh debe nombrar macOS {major}"
        );
        let manifest = include_str!("../../../packaging/bundle-manifest.json");
        let sections = avi_shared::manifest::parse_manifest_text(manifest)
            .expect("el manifiesto de bundle es válido");
        assert_eq!(
            sections["aarch64-apple-darwin"].min_os.as_deref(),
            Some(target),
            "el min_os del manifiesto debe ser MACOSX_DEPLOYMENT_TARGET"
        );
    }

    #[test]
    fn test_cask_zap_lists_current_model_caches() {
        let c = render_cask("9.9.9", &"b".repeat(64)).unwrap();
        // Raíz de modelos exclusiva: el canal Cask no puede usar
        // `self uninstall`, así que este `zap` es su única vía de borrado.
        let models_root = format!("~/Library/Caches/{}/models", avi_shared::paths::APP_NAME);
        assert!(
            c.contains(&models_root),
            "el zap debe incluir la raíz de modelos {models_root}"
        );
        // Soporte de la aplicación desde la canónica.
        assert!(c.contains(&format!(
            "~/Library/Application Support/{}",
            avi_shared::paths::APP_NAME
        )));
        // Un snapshot por modelo pineado, sin réplica literal.
        for pin in avi_shared::paths::MODEL_REVISIONS {
            let snapshot = format!("models--{}", pin.repo.replace('/', "--"));
            assert!(
                c.contains(&snapshot),
                "el zap debe derivar el snapshot canónico {snapshot}"
            );
        }
        // Los recursos globales de la caché HF compartida no son de la aplicación:
        // el zap no los nombra.
        for forbidden in [
            "~/.cache/huggingface/xet",
            "~/.cache/huggingface/hub/.locks",
        ] {
            assert!(
                !c.contains(forbidden),
                "el zap no debe incluir el global {forbidden}"
            );
        }
        // Conteo exacto: modelos + soporte + un snapshot por modelo.
        let zap_block = c
            .split("zap trash: [")
            .nth(1)
            .unwrap_or("")
            .split(']')
            .next()
            .unwrap_or("");
        let trash_lines = zap_block.lines().filter(|l| l.contains('"')).count();
        assert_eq!(
            trash_lines,
            2 + avi_shared::paths::MODEL_REVISIONS.len(),
            "el zap debe listar exactamente modelos + soporte + snapshots"
        );
    }

    /// Texto de `.circleci/config.yml`, localizado desde la raíz o desde el crate.
    fn read_ci_config() -> String {
        let candidates = [".circleci/config.yml", "../../.circleci/config.yml"];
        for p in candidates {
            if let Ok(t) = std::fs::read_to_string(p) {
                return t;
            }
        }
        // Fallback via CARGO_MANIFEST_DIR
        let m = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.circleci/config.yml");
        std::fs::read_to_string(&m).expect("no se pudo leer .circleci/config.yml")
    }

    /// Texto de un job de la pipeline, desde su cabecera hasta la del siguiente.
    fn ci_job_block(cfg: &str, job: &str) -> String {
        let header = format!("\n  {job}:\n");
        let start = cfg
            .find(&header)
            .unwrap_or_else(|| panic!("no hay job {job}"));
        let body = &cfg[start + header.len()..];
        let next = Regex::new(r"\n  [a-z0-9-]+:\n")
            .unwrap()
            .find(body)
            .map_or(body.len(), |m| m.start());
        body[..next].to_string()
    }

    /// El job de publicación usa la misma imagen con Rust que `publish-metadata`, genera el pie
    /// de las notas y las fuentes de FFmpeg con xtask y publica sus assets; cada job de build
    /// persiste la configuración de build de FFmpeg que el release adjunta.
    #[test]
    fn test_publish_release_ships_ffmpeg_sources_and_buildconf() {
        let cfg = read_ci_config();
        let release = ci_job_block(&cfg, "publish-release");
        let metadata = ci_job_block(&cfg, "publish-metadata");
        let image = |block: &str| {
            block
                .lines()
                .find_map(|l| l.trim().strip_prefix("- image: ").map(str::to_string))
                .expect("el job declara su imagen")
        };
        assert!(image(&release).starts_with("cimg/rust:"));
        assert_eq!(image(&release), image(&metadata));
        assert!(release.contains("cargo --quiet xtask ffmpeg --sources artifacts"));
        assert!(release
            .contains("cargo --quiet xtask source-offer --notes \"$CIRCLE_TAG\" >> notes.md"));
        assert!(release.contains("sha256sum ai-voice-interconnector-* ffmpeg-*"));
        assert!(release.contains("artifacts/ffmpeg-*"));
        assert!(!release.contains("echo \"- Tarball de fuente"));
        for job in [
            "build-windows-x64",
            "build-linux-x64",
            "build-linux-arm64",
            "build-darwin-arm64",
        ] {
            assert!(
                ci_job_block(&cfg, job).contains("- ffmpeg-*-buildconf-*.txt"),
                "{job} debe persistir la configuración de build de FFmpeg"
            );
        }
    }

    /// Los builds no esperan a los gates y `publish-release` es la única puerta de
    /// la publicación: exige todo job de prueba y validación del workflow, los
    /// builds y las verificaciones de portabilidad. Un gate nuevo que no entre en
    /// sus `requires:` dejaría publicar sin él.
    #[test]
    fn test_workflow_builds_run_in_parallel_and_gate_publish_release() {
        let cfg = read_ci_config();
        let entries = workflow_entries(&cfg);
        let builds = [
            "build-windows-x64",
            "build-linux-x64",
            "build-linux-arm64",
            "build-darwin-arm64",
        ];
        for build in builds {
            let (_, requires) = entries
                .iter()
                .find(|(name, _)| name == build)
                .unwrap_or_else(|| panic!("build-all debe incluir {build}"));
            assert!(requires.is_empty(), "{build} no debe declarar requires:");
        }
        let (_, publish) = entries
            .iter()
            .find(|(name, _)| name == "publish-release")
            .expect("build-all debe incluir publish-release");
        let expected: Vec<&str> = entries
            .iter()
            .map(|(name, _)| name.as_str())
            .filter(|name| !name.starts_with("publish-"))
            .collect();
        assert!(
            expected.iter().any(|n| n.starts_with("test-"))
                && expected.contains(&"coverage")
                && expected.contains(&"validate-licenses")
                && expected
                    .iter()
                    .any(|n| n.starts_with("verify-portability-")),
            "build-all debe incluir gates, builds y verificaciones: {expected:?}"
        );
        for name in &expected {
            assert!(
                publish.iter().any(|r| r == name),
                "publish-release debe requerir {name}"
            );
        }
        assert_eq!(
            publish.len(),
            expected.len(),
            "publish-release solo requiere gates, builds y verificaciones: {publish:?}"
        );
    }

    /// `ffmpeg-bundle/` se restaura antes de su primer uso en los ocho jobs que
    /// aseguran FFmpeg, y la clave solo depende del triple y de
    /// `packaging/pins.json`: la caché es segura porque `xtask` reutiliza el
    /// bundle solo si su marcador coincide con el SHA-256 fijado, y repetir en la
    /// clave la versión, la URL o el hash de FFmpeg los duplicaría fuera de la
    /// fuente única de pines.
    #[test]
    fn test_ffmpeg_bundle_is_cached_before_first_use() {
        let cfg = read_ci_config();
        let tests = &["- provision_ffmpeg_unix", "- provision_ffmpeg_windows"][..];
        let builds = &["name: Empaquetar bundle con xtask package"][..];
        let jobs = [
            ("test-linux", tests),
            ("coverage", tests),
            ("test-macos", tests),
            ("test-windows", tests),
            ("build-windows-x64", builds),
            ("build-linux-x64", builds),
            ("build-linux-arm64", builds),
            ("build-darwin-arm64", builds),
        ];
        for (job, first_use) in jobs {
            let section = build_section(&cfg, job);
            let restore = section
                .find("- ffmpeg_restore_cache:")
                .unwrap_or_else(|| panic!("{job} debe restaurar la caché de ffmpeg-bundle/"));
            let used = first_use
                .iter()
                .filter_map(|marker| section.find(marker))
                .min()
                .unwrap_or_else(|| panic!("{job} debe asegurar FFmpeg"));
            let save = section
                .find("- ffmpeg_save_cache:")
                .unwrap_or_else(|| panic!("{job} debe guardar la caché de ffmpeg-bundle/"));
            assert!(
                restore < used && used < save,
                "{job}: la caché de ffmpeg-bundle/ debe restaurarse antes de su primer uso y guardarse después"
            );
        }
        let key = "ffmpeg-v1-{{ checksum \".cache-root\" }}-<< parameters.triple >>-{{ checksum \"packaging/pins.json\" }}";
        assert_eq!(
            cfg.matches(key).count(),
            2,
            "la restauración y el guardado deben usar la misma clave por triple, raíz del job y pins.json"
        );
        let commands = cfg
            .split("\n  ffmpeg_restore_cache:")
            .nth(1)
            .and_then(|s| s.split("\n  cargo_fetch:").next())
            .expect("deben existir ffmpeg_restore_cache y ffmpeg_save_cache");
        let pins = crate::doctor::load_pins(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
            .expect("se cargan los pines");
        for (literal, what) in [
            (pins.ffmpeg.as_str(), "la versión"),
            ("https://", "una URL"),
            ("sha256", "un hash"),
        ] {
            assert!(
                !commands.contains(literal),
                "la clave de la caché de FFmpeg no debe repetir {what} de FFmpeg"
            );
        }
        assert!(commands.contains("- ffmpeg-bundle\n") && commands.contains("when: on_success"));
    }

    /// El paso de MSYS2 no compara versiones por su cuenta: la regla estricta vive en
    /// `build-engine`.
    #[test]
    fn test_windows_msys2_step_has_no_drift_logic() {
        let windows = ci_job_block(&read_ci_config(), "build-windows-x64");
        assert!(!windows.contains("$GccPin"));
        assert!(!windows.contains("[WARN] gcc instalado"));
        assert!(windows.contains("pacman -Q mingw-w64-ucrt-x86_64-gcc"));
    }

    /// Directorios `vendor/<dir>` con `Cargo.toml`, ordenados: la regla del CI
    /// para decidir qué crates son parches vendorizados.
    fn vendor_crate_dirs() -> Vec<String> {
        let vendor = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vendor");
        let mut dirs: Vec<String> = std::fs::read_dir(&vendor)
            .expect("se lee vendor/")
            .filter_map(|entry| {
                let entry = entry.ok()?;
                entry
                    .path()
                    .join("Cargo.toml")
                    .is_file()
                    .then(|| format!("vendor/{}", entry.file_name().to_string_lossy()))
            })
            .collect();
        dirs.sort();
        dirs
    }

    /// Los crates de `vendor/*/Cargo.toml` que toma el CI son exactamente los
    /// paquetes de ruta ajenos al workspace que resuelve cargo con todas las
    /// features: detecta parches sin usar, parches fuera de `vendor/` y crates
    /// vendorizados sin `[patch]`.
    #[test]
    fn test_vendored_patches_match_cargo_metadata() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let out = std::process::Command::new("cargo")
            .args([
                "metadata",
                "--format-version",
                "1",
                "--locked",
                "--all-features",
            ])
            .current_dir(&root)
            .output()
            .expect("se ejecuta cargo metadata");
        assert!(
            out.status.success(),
            "cargo metadata falló: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let meta: serde_json::Value =
            serde_json::from_slice(&out.stdout).expect("cargo metadata emite JSON");
        let members: HashSet<&str> = meta["workspace_members"]
            .as_array()
            .expect("workspace_members es una lista")
            .iter()
            .filter_map(|m| m.as_str())
            .collect();
        let root_abs = root.canonicalize().expect("se resuelve la raíz");
        let mut resolved: Vec<String> = Vec::new();
        for pkg in meta["packages"].as_array().expect("packages es una lista") {
            let id = pkg["id"].as_str().unwrap_or_default();
            if !pkg["source"].is_null() || members.contains(id) {
                continue;
            }
            let manifest = PathBuf::from(pkg["manifest_path"].as_str().unwrap_or_default());
            let dir = manifest.parent().expect("el manifiesto tiene directorio");
            let dir = dir
                .canonicalize()
                .unwrap_or_else(|e| panic!("no se pudo resolver {}: {e}", dir.display()));
            let rel = dir
                .strip_prefix(&root_abs)
                .unwrap_or_else(|_| panic!("{} queda fuera del repositorio", dir.display()))
                .to_string_lossy()
                .replace('\\', "/");
            assert!(rel.starts_with("vendor/"), "{rel} debe vivir bajo vendor/");
            resolved.push(rel);
        }
        resolved.sort();
        assert!(
            !resolved.is_empty(),
            "cargo debe resolver parches vendorizados"
        );
        assert_eq!(
            vendor_crate_dirs(),
            resolved,
            "vendor/*/Cargo.toml debe coincidir con los paquetes de ruta que resuelve cargo"
        );
    }

    /// Ninguna regla de ignorado alcanza los ficheros de un parche vendorizado,
    /// que se compila entero desde sus fuentes: un fichero ignorado al copiarlo
    /// no entraría en git y el build fallaría solo en un checkout limpio. Solo
    /// se admite el `Cargo.lock` propio del crate, que cargo no usa en una
    /// dependencia por ruta.
    #[test]
    fn test_vendored_patches_have_no_ignored_files() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        for dir in vendor_crate_dirs() {
            let out = std::process::Command::new("git")
                .args([
                    "ls-files",
                    "--others",
                    "--ignored",
                    "--exclude-standard",
                    "--",
                ])
                .arg(&dir)
                .current_dir(&root)
                .output()
                .expect("se ejecuta git ls-files");
            assert!(out.status.success(), "git ls-files falló en {dir}");
            let allowed = format!("{dir}/Cargo.lock");
            let ignored: Vec<String> = String::from_utf8_lossy(&out.stdout)
                .lines()
                .filter(|l| *l != allowed)
                .map(str::to_string)
                .collect();
            assert!(
                ignored.is_empty(),
                "ficheros ignorados en el parche {dir}: {ignored:?}"
            );
        }
    }

    /// El prefijo con que xtask busca las envolturas de ALSA en la tabla de
    /// símbolos es el mismo que escribe el parche en cada `export_name`.
    #[test]
    fn test_alsa_wrapper_export_name_uses_symbol_prefix() {
        let build =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vendor/alsa-sys-0.4.0/build.rs");
        let text = std::fs::read_to_string(&build).expect("se lee el build.rs de alsa-sys");
        let expected = format!(
            "export_name = \\\"{}{{name}}\\\"",
            alsa_symbols::SYMBOL_PREFIX
        );
        assert!(
            text.contains(&expected),
            "el build.rs de alsa-sys debe nombrar cada estático con `{expected}`"
        );
    }

    /// Jobs de la sección `jobs:` como pares (nombre, texto). Cada sección va
    /// desde su clave a dos espacios de sangría hasta la siguiente clave del
    /// mismo nivel (o hasta `workflows:` para el último).
    fn ci_jobs(cfg: &str) -> Vec<(&str, &str)> {
        let start = cfg
            .find("\njobs:\n")
            .expect("debe existir la sección jobs:")
            + "\njobs:\n".len();
        let end = cfg
            .find("\nworkflows:")
            .expect("debe existir la sección workflows:");
        let body = &cfg[start..end];
        let is_key = |l: &str| {
            l.starts_with("  ")
                && !l.starts_with("   ")
                && !l.trim_start().starts_with('#')
                && l.trim_end().ends_with(':')
        };
        let mut heads: Vec<(usize, &str)> = Vec::new();
        let mut offset = 0;
        for line in body.split_inclusive('\n') {
            let bare = line.trim_end_matches(['\n', '\r']);
            if is_key(bare) {
                heads.push((offset, bare.trim().trim_end_matches(':')));
            }
            offset += line.len();
        }
        heads
            .iter()
            .enumerate()
            .map(|(i, &(from, name))| {
                let to = heads.get(i + 1).map_or(body.len(), |h| h.0);
                (name, &body[from..to])
            })
            .collect()
    }

    /// Entradas del workflow `build-all` como pares (nombre efectivo, `requires:`).
    /// El nombre efectivo es el `name:` de la entrada cuando lo declara (las
    /// instancias de un job parametrizado) y, si no, el del job.
    fn workflow_entries(cfg: &str) -> Vec<(String, Vec<String>)> {
        let body = cfg
            .split("\n  build-all:\n")
            .nth(1)
            .expect("debe existir el workflow build-all")
            .split("\n    jobs:\n")
            .nth(1)
            .expect("build-all debe declarar jobs:");
        let mut entries: Vec<(String, Vec<String>)> = Vec::new();
        let mut in_requires = false;
        for line in body.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            let depth = indent(line);
            if depth < 6 {
                break;
            }
            if depth == 6 {
                let job = trimmed
                    .strip_prefix("- ")
                    .and_then(|s| s.strip_suffix(':'))
                    .unwrap_or_else(|| panic!("entrada de build-all inesperada: `{line}`"));
                entries.push((job.to_string(), Vec::new()));
                in_requires = false;
                continue;
            }
            let entry = entries
                .last_mut()
                .expect("toda línea pertenece a una entrada");
            if depth == 10 {
                in_requires = trimmed == "requires:";
                if let Some(name) = trimmed.strip_prefix("name: ") {
                    entry.0 = name.to_string();
                }
            } else if in_requires {
                if let Some(req) = trimmed.strip_prefix("- ") {
                    entry.1.push(req.to_string());
                }
            }
        }
        entries
    }

    /// Jobs que usan cargo: los que restauran el registry y `target/`.
    fn cargo_jobs(cfg: &str) -> Vec<(&str, &str)> {
        ci_jobs(cfg)
            .into_iter()
            .filter(|(_, s)| s.contains("- cargo_restore_caches"))
            .collect()
    }

    /// Ninguna compilación precede a la restauración de cachés: una restauración
    /// extraída sobre un `target/` ya poblado mezcla el estado de dos ejecuciones,
    /// y una compilación anterior al entorno completo no usa el registry ni el
    /// `sccache` del job. Las puertas `validate_*` son la primera compilación del
    /// workspace, juntas y, en los jobs que restauran `sccache`, tras su
    /// configuración.
    #[test]
    fn test_cargo_jobs_compile_only_after_cache_restore() {
        let cfg = read_ci_config();
        let compiling = [
            "cargo xtask",
            "cargo --quiet xtask",
            "cargo build",
            "cargo test",
            "cargo run",
            "cargo llvm-cov",
            "cargo install",
        ];
        let jobs = cargo_jobs(&cfg);
        assert!(
            jobs.len() >= 11,
            "se esperaban al menos 11 jobs con cargo, hay {}",
            jobs.len()
        );
        for (job, section) in jobs {
            let lines: Vec<&str> = section
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with("name:"))
                .collect();
            let restore = lines
                .iter()
                .position(|l| l.starts_with("- cargo_restore_caches"))
                .unwrap_or_else(|| panic!("{job} debe restaurar cachés de cargo"));
            for l in &lines[..restore] {
                assert!(
                    !l.starts_with("- validate_pins") && !l.starts_with("- validate_features"),
                    "{job}: `{l}` compila antes de restaurar las cachés de cargo"
                );
                assert!(
                    !compiling.iter().any(|c| l.contains(c)),
                    "{job}: `{l}` compila antes de restaurar las cachés de cargo"
                );
            }
            let pins = lines
                .iter()
                .position(|l| *l == "- validate_pins")
                .unwrap_or_else(|| panic!("{job} debe ejecutar validate_pins"));
            assert_eq!(
                lines.get(pins + 1),
                Some(&"- validate_features"),
                "{job}: validate_features debe ir inmediatamente después de validate_pins"
            );
            if !lines
                .iter()
                .any(|l| l.starts_with("- sccache_restore_cache"))
            {
                continue;
            }
            let last_sccache = lines
                .iter()
                .rposition(|l| {
                    l.starts_with("- sccache_setup_") || l.starts_with("- native_sccache_setup_")
                })
                .unwrap_or_else(|| panic!("{job} restaura sccache y debe configurarlo"));
            assert!(
                pins > last_sccache,
                "{job}: las puertas validate_* deben ir después de la configuración de sccache"
            );
        }
    }

    /// `CARGO_INCREMENTAL=0` es propiedad del entorno de cada job que usa cargo:
    /// `sccache` no cachea compilaciones incrementales y el estado incremental no
    /// debe viajar en la caché de `target/`. No se fija en ningún otro punto.
    #[test]
    fn test_cargo_jobs_disable_incremental_at_job_level() {
        let cfg = read_ci_config();
        let jobs = cargo_jobs(&cfg);
        for (job, section) in &jobs {
            let lines: Vec<&str> = section.lines().collect();
            let env = lines
                .iter()
                .position(|l| *l == "    environment:")
                .unwrap_or_else(|| panic!("{job} debe declarar environment:"));
            let block: Vec<&str> = lines[env + 1..]
                .iter()
                .copied()
                .take_while(|l| l.trim().is_empty() || indent(l) > 4)
                .collect();
            assert!(
                block.iter().any(|l| l.trim() == "CARGO_INCREMENTAL: \"0\""),
                "{job} debe fijar CARGO_INCREMENTAL=0 en el environment del job"
            );
        }
        assert_eq!(
            cfg.matches("CARGO_INCREMENTAL").count(),
            jobs.len(),
            "CARGO_INCREMENTAL solo puede aparecer una vez por job, en su environment"
        );
    }

    #[test]
    fn test_pipeline_heterogeneous_and_weekly_sccache() {
        let cfg = read_ci_config();
        // Modelo vigente: test-linux, test-windows, test-macos, coverage y build-* usan
        // cargo_restore_caches (registry + caché de target/) y sccache autoconsistente por
        // variante (cada job pesado restaura y guarda su propio blob). Los jobs pequeños
        // (validate-licenses, publish-release y publish-metadata) usan la variante xtask de la
        // caché de target/, que guarda validate-licenses, y no restauran sccache: `xtask` solo
        // recompila los crates propios.
        for job in ["validate-licenses", "publish-release", "publish-metadata"] {
            let section = build_section(&cfg, job);
            assert!(
                section.contains(
                    "- cargo_restore_caches:\n          os: linux\n          variant: xtask\n"
                ),
                "{job} debe restaurar la variante xtask de la caché de target/"
            );
            assert!(
                !section.contains("sccache_restore_cache")
                    && !section.contains("sccache_setup_unix"),
                "{job} no debe restaurar ni configurar sccache"
            );
        }
        assert!(
            build_section(&cfg, "validate-licenses")
                .contains("- cargo_save_target:\n          os: linux\n          variant: xtask\n"),
            "validate-licenses debe guardar la variante xtask de la caché de target/"
        );
        // El guardado de sccache no depende de la tasa de aciertos: no existe comando condicional.
        assert!(
            !cfg.contains("sccache_save_cache_conditional"),
            "no debe existir sccache_save_cache_conditional (guardado condicional por hit-rate eliminado)"
        );
        // El guardado vigente es sccache_save_cache con clave por semana ISO y when: on_success:
        // la primera corrida de la semana ocupa la clave, y un job fallido no debe ocuparla con
        // lo restaurado sin cambios.
        let save = cfg
            .split("\n  sccache_save_cache:")
            .nth(1)
            .and_then(|s| s.split("\n  sccache_setup_unix:").next())
            .expect("debe existir el comando sccache_save_cache");
        assert!(
            save.contains("key: sccache-v1-{{ checksum \".cache-root\" }}-{{ arch }}-<< parameters.os >>-<< pipeline.parameters.rust_pin >>-<< parameters.variant >>-{{ checksum \".sccache-week\" }}"),
            "sccache_save_cache debe usar clave por semana ISO segmentada por variante"
        );
        assert!(
            save.contains("when: on_success") && !save.contains("when: always"),
            "sccache_save_cache debe guardar solo si el job tiene éxito"
        );
        // La semana se escribe antes de restaurar: la clave se forma en tiempo de config y
        // `.sccache-week` debe existir cuando el guardado calcula su checksum.
        let restore = cfg
            .split("\n  sccache_restore_cache:")
            .nth(1)
            .and_then(|s| s.split("\n  sccache_save_cache:").next())
            .expect("debe existir el comando sccache_restore_cache");
        let week = restore
            .find("date -u +%G-W%V > .sccache-week")
            .expect("sccache_restore_cache debe escribir la semana ISO en .sccache-week");
        assert!(
            restore.contains("grep -Eqx '[0-9]{4}-W[0-9]{2}' .sccache-week"),
            "sccache_restore_cache debe fallar si la semana no tiene la forma AAAA-WNN"
        );
        assert!(
            week < restore
                .find("- restore_cache:")
                .expect("sccache_restore_cache restaura"),
            "la semana debe escribirse antes de restaurar la caché de sccache"
        );
        assert!(
            cfg.contains("sccache-v1-{{ checksum \".cache-root\" }}-{{ arch }}-<< parameters.os >>-<< pipeline.parameters.rust_pin >>-<< parameters.variant >>-"),
            "sccache_restore_cache debe usar clave por prefijo segmentada por variante"
        );
        // Secciones scoping por job (delimitadas por siguiente job header para evitar falso-positivo cross-job)
        let linux_section = build_section(&cfg, "test-linux");
        assert!(
            linux_section.contains("cargo_restore_caches"),
            "test-linux debe usar cargo_restore_caches (registry + caché de target/)"
        );
        assert!(
            linux_section.contains("variant: test"),
            "test-linux debe usar variant: test"
        );
        assert!(
            linux_section.contains("cargo_save_target"),
            "test-linux debe guardar la caché de target/ (cargo_save_target)"
        );
        assert!(
            linux_section.contains("sccache_restore_cache"),
            "test-linux debe restaurar sccache autoconsistente (variant: test)"
        );
        assert!(
            linux_section.contains("sccache_save_cache"),
            "test-linux debe guardar sccache (sccache_save_cache)"
        );
        let windows_section = build_section(&cfg, "test-windows");
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
            "test-windows debe guardar la caché de target/ (cargo_save_target)"
        );
        assert!(
            windows_section.contains("sccache_restore_cache"),
            "test-windows debe usar sccache"
        );
        assert!(
            windows_section.contains("sccache_save_cache"),
            "test-windows debe guardar sccache (sccache_save_cache)"
        );
        // coverage debe usar cargo_restore_caches con os: linux y variant: cov y guardar la caché de target/
        let coverage_section = build_section(&cfg, "coverage");
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
            "coverage debe guardar la caché de target/ (cargo_save_target)"
        );
        assert!(
            coverage_section.contains("sccache_save_cache"),
            "coverage debe guardar sccache (sccache_save_cache)"
        );
        // test-macos usa cargo_restore_caches (registry + caché de target/) y sccache autoconsistente (variant: test), igual que test-linux
        let macos_section = build_section(&cfg, "test-macos");
        assert!(
            macos_section.contains("cargo_restore_caches"),
            "test-macos debe usar cargo_restore_caches (registry + caché de target/)"
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
            "test-macos debe guardar la caché de target/ (cargo_save_target)"
        );
        // build-* deben usar cargo_restore_caches con la caché de target/ full (heterogéneo con target en build-*).
        // NO deben ejecutar `cargo clean -p ai-voice-interconnector`: sin --release/--profile
        // es un no-op sobre el perfil release (limpia solo target/debug), y aunque no lo fuera
        // el bump de VERSION ya invalida el fingerprint de cargo por sí solo (mtime + -C
        // metadata) y sccache nunca cachea crates --crate-type bin. Ver la
        // documentación de compilación.
        for job in [
            "build-windows-x64",
            "build-linux-x64",
            "build-linux-arm64",
            "build-darwin-arm64",
        ] {
            let section = build_section(&cfg, job);
            assert!(
                section.contains("cargo_restore_caches"),
                "{job} debe usar cargo_restore_caches (con la caché de target/)"
            );
            assert!(
                section.contains("variant: full"),
                "{job} debe usar variant: full"
            );
            assert!(
                section.contains("cargo_save_target"),
                "{job} debe guardar la caché de target/ (cargo_save_target)"
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

    /// `Cargo.lock.cachekey` se genera en un único command, que usa la
    /// restauración de cachés de cargo, y que falla si la versión del propio crate no se
    /// sustituye exactamente una vez: un lock con otro formato no puede dejar en
    /// silencio una clave que cambie con cada release.
    #[test]
    fn test_lock_cachekey_is_one_checked_command() {
        let cfg = read_ci_config();
        assert_eq!(
            cfg.matches("Cargo.lock > Cargo.lock.cachekey").count(),
            1,
            "Cargo.lock.cachekey debe generarse en un solo sitio"
        );
        let command = cfg
            .split("\n  cargo_lock_cachekey:")
            .nth(1)
            .and_then(|s| s.split("\n  cargo_restore_caches:").next())
            .expect("el command cargo_lock_cachekey precede a cargo_restore_caches");
        assert!(
            command.contains("Cargo.lock > Cargo.lock.cachekey")
                && command.contains("END { $? = 1 if $n != 1 }")
                && command.contains("set -euo pipefail"),
            "cargo_lock_cachekey debe fallar si la sustitución no ocurre exactamente una vez"
        );
        let steps = cfg
            .split("\n  cargo_restore_caches:")
            .nth(1)
            .and_then(|s| s.split("\n    steps:\n").nth(1))
            .expect("cargo_restore_caches debe tener pasos");
        assert!(
            steps.starts_with("      - cargo_lock_cachekey\n"),
            "cargo_restore_caches debe generar la clave antes de restaurar"
        );
    }

    #[test]
    fn test_target_key_with_vendored_patches_identity() {
        let cfg = read_ci_config();
        // Los parches locales se fingerprintean por mtime: el checkout los marca
        // Dirty y arrastra lo que dependa de ellos. La clave exacta de la caché
        // de target/ lleva el tree hash git de cada uno, con la lista tomada del
        // glob `vendor/*/Cargo.toml`, y NO tiene fallback,
        // de modo que un acierto garantiza que target/ corresponde al contenido
        // actual y fijar el mtime es seguro. Reintroducir un fallback fijaría
        // mtimes sobre snapshots ajenos.
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
            .split("\n  toolchain_restore_cache:")
            .next()
            .unwrap_or("");

        let hash_cmd = r#"git rev-parse "HEAD:$dir""#;
        // Paso que calcula la identidad: desde su nombre hasta la restauración.
        let identity_step = restore_section
            .split("Calcular identidad de contenido de los parches vendorizados")
            .nth(1)
            .unwrap_or("")
            .split("- restore_cache:")
            .next()
            .unwrap_or("");
        assert!(
            identity_step.contains("for manifest in vendor/*/Cargo.toml")
                && identity_step.contains("LC_ALL=C"),
            "cargo_restore_caches debe tomar los parches del glob vendor/*/Cargo.toml en orden determinista"
        );
        assert!(
            !identity_step.contains("perl") && !identity_step.contains(r"[patch\.crates-io]"),
            "la lista de parches no debe analizar TOML"
        );
        assert!(
            restore_section.contains(hash_cmd),
            "cargo_restore_caches debe calcular el tree hash git de cada parche de la lista"
        );
        assert!(
            restore_section.contains("done < .vendor-patches.list")
                && restore_section.contains(r#"/usr/bin/find "$dir" -exec touch -t 200001010000"#),
            "cargo_restore_caches debe fijar el mtime de cada parche de la lista"
        );
        assert!(
            !restore_section.contains("HEAD:vendor/"),
            "cargo_restore_caches no debe enumerar parches a mano"
        );
        assert!(
            restore_section.contains(".vendor-patches.tree"),
            "el tree hash de los parches debe escribirse en .vendor-patches.tree"
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
            .filter_map(|l| l.strip_prefix("- target-v6-"))
            .collect();
        assert_eq!(
            restore_keys.len(),
            1,
            "la caché de target/ debe restaurarse con una única clave exacta, sin fallback por prefijo"
        );
        assert!(
            restore_keys[0].contains(r#"checksum ".vendor-patches.tree""#),
            "la clave de target/ debe incluir el tree hash de los parches vendorizados"
        );
        let save_key = save_section
            .lines()
            .map(str::trim)
            .find_map(|l| l.strip_prefix("key: target-v6-"))
            .unwrap_or("");
        assert_eq!(
            restore_keys[0], save_key,
            "las claves de restauración y guardado de la caché de target/ deben ser idénticas"
        );

        let pos_hash = restore_section.find(hash_cmd).unwrap();
        let pos_restore = restore_section
            .find("- target-v6-")
            .expect("cargo_restore_caches debe restaurar la caché de target/");
        let pos_touch = restore_section
            .find("touch -t 200001010000")
            .expect("cargo_restore_caches debe fijar el mtime de los parches");
        assert!(
            pos_hash < pos_restore,
            "el tree hash debe calcularse antes de restaurar la caché de target/"
        );
        assert!(
            pos_restore < pos_touch,
            "el mtime de los parches debe fijarse después de restaurar la caché de target/"
        );

        for leftover in [
            "sort -z",
            "sha256_hex",
            "target/.vendor-cmake.sha256",
            ".vendor-cmake.tree",
        ] {
            assert!(
                !cfg.contains(leftover),
                "la config no debe contener `{leftover}` (mecanismo de sello retirado)"
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
    fn test_no_racy_pipe_grep_q_or_head_under_pipefail() {
        let cfg = read_ci_config();
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
            "no debe existir `find ... | head` directo: materializar la salida (`matches=\"$(find ...)\"`) antes de aplicar head (regresión EPIPE v0.20.9)"
        );
    }

    /// Las cachés de `target/` y de MSYS2 usan clave inmutable (el primer
    /// `save_cache` gana): guardar tras un fallo persistiría para siempre un
    /// directorio incompleto (ya ocurrió con linux-x64 en v0.20.9). Por eso sus
    /// commands fijan `when: on_success` en cada paso y no lo exponen como
    /// parámetro que un job pueda cambiar.
    #[test]
    fn test_immutable_cache_saves_only_on_success() {
        let cfg = read_ci_config();
        for command in ["cargo_save_target", "msys2_save_cache"] {
            // El command termina en la primera línea en blanco.
            let block = cfg
                .split(&format!("\n  {command}:\n"))
                .nth(1)
                .unwrap_or("")
                .split("\n\n")
                .next()
                .unwrap_or("");
            let whens: Vec<&str> = block
                .lines()
                .map(str::trim)
                .filter(|line| line.starts_with("when:"))
                .collect();
            assert!(
                !whens.is_empty(),
                "{command} debe declarar `when` en sus pasos"
            );
            assert!(
                whens.iter().all(|when| *when == "when: on_success"),
                "{command} debe guardar solo con `when: on_success` (clave inmutable); encontrado: {whens:?}"
            );
            assert!(
                !block.contains("parameters.when"),
                "{command} no debe exponer `when` como parámetro"
            );
        }
    }

    /// Claves de caché del config: cada `key:` de `save_cache` y cada elemento de
    /// la lista `keys:` de `restore_cache`.
    fn cache_keys(cfg: &str) -> Vec<&str> {
        let mut keys = Vec::new();
        let mut list_indent = None;
        for line in cfg.lines() {
            let trimmed = line.trim_start();
            let indent = line.len() - trimmed.len();
            if let Some(list) = list_indent {
                match trimmed.strip_prefix("- ") {
                    Some(key) if indent > list => {
                        keys.push(key);
                        continue;
                    }
                    _ => list_indent = None,
                }
            }
            if trimmed == "keys:" {
                list_indent = Some(indent);
            } else if let Some(key) = trimmed.strip_prefix("key: ") {
                keys.push(key);
            }
        }
        keys
    }

    /// CircleCI restaura cada caché en la ruta absoluta desde la que se guardó, y
    /// los ejecutores no comparten raíz (`cimg/rust` corre como `circleci`,
    /// manylinux como root). La regla es uniforme: toda clave lleva el checksum
    /// de `.cache-root` (HOME y directorio de trabajo) justo tras su prefijo
    /// `familia-vN-`, también los prefijos de restauración, y todo job que toca
    /// una caché, directamente o a través de un command, escribe `.cache-root`
    /// antes de la primera. Así ninguna familia, presente o futura, puede servir
    /// una entrada que se extraiga donde el job no la busca.
    #[test]
    fn test_every_cache_key_is_scoped_to_job_root() {
        let cfg = read_ci_config();
        let root_key = "{{ checksum \".cache-root\" }}";
        let keys = cache_keys(&cfg);
        assert!(
            keys.len() >= 20,
            "se esperaban todas las claves de caché: {keys:?}"
        );
        for key in keys {
            let scoped = key.split_once("-v").is_some_and(|(family, rest)| {
                let digits = rest.chars().take_while(char::is_ascii_digit).count();
                !family.is_empty()
                    && digits > 0
                    && rest[digits..].starts_with(&format!("-{root_key}-"))
            });
            assert!(
                scoped,
                "la clave {key} debe llevar {root_key} justo tras su prefijo familia-vN-"
            );
        }

        // Commands que tocan una caché, directa o indirectamente.
        let commands = cfg
            .split("\ncommands:\n")
            .nth(1)
            .and_then(|s| s.split("\njobs:\n").next())
            .expect("el config declara commands y jobs");
        // Cada command empieza en una línea `  nombre:` con sangría 2.
        let mut blocks: Vec<(&str, String)> = Vec::new();
        for line in commands.lines() {
            let name = line
                .strip_prefix("  ")
                .and_then(|l| l.strip_suffix(':'))
                .filter(|n| n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
            match name {
                Some(name) => blocks.push((name, String::new())),
                None => {
                    if let Some(last) = blocks.last_mut() {
                        last.1.push_str(line);
                        last.1.push('\n');
                    }
                }
            }
        }
        let invokes = |text: &str, name: &str| {
            text.match_indices(&format!("- {name}"))
                .map(|(i, m)| i + m.len())
                .find(|&end| matches!(text[end..].chars().next(), Some(':' | '\n' | '\r')))
                .map(|end| end - name.len() - 2)
        };
        let mut cache_commands: Vec<&str> = Vec::new();
        loop {
            let before = cache_commands.len();
            for (name, body) in &blocks {
                let touches = body.contains("restore_cache:")
                    || body.contains("save_cache:")
                    || cache_commands.iter().any(|c| invokes(body, c).is_some());
                if touches && !cache_commands.contains(name) {
                    cache_commands.push(name);
                }
            }
            if cache_commands.len() == before {
                break;
            }
        }
        assert!(
            !cache_commands.contains(&"cache_root"),
            "cache_root solo escribe .cache-root"
        );

        let mut jobs_with_cache = 0;
        for (job, section) in ci_jobs(&cfg) {
            let first_cache = ["- restore_cache:", "- save_cache:"]
                .iter()
                .filter_map(|marker| section.find(marker))
                .chain(cache_commands.iter().filter_map(|c| invokes(section, c)))
                .min();
            let Some(first_cache) = first_cache else {
                continue;
            };
            jobs_with_cache += 1;
            let root = invokes(section, "cache_root")
                .unwrap_or_else(|| panic!("{job} usa cachés y debe ejecutar cache_root"));
            assert!(
                root < first_cache,
                "{job} debe ejecutar cache_root antes de su primera caché"
            );
        }
        assert_eq!(jobs_with_cache, 12, "se esperaban los 12 jobs con caché");
        assert!(
            cfg.contains("printf '%s\\n%s\\n' \"$HOME\" \"$(pwd)\" > .cache-root"),
            ".cache-root debe registrar HOME y el directorio de trabajo"
        );
    }

    /// Los informes de `--timings` salen de `target/` en el mismo paso que los
    /// genera: vaciados antes del build (la caché de `target/` puede traer los de
    /// otra corrida) y movidos fuera al terminar. Así la caché no los guarda y la
    /// limpieza previa al guardado no tiene que borrarlos mientras la subida del
    /// artefacto aún los mantiene abiertos, que en Windows hace fallar `rm`.
    #[test]
    fn test_cargo_timings_leave_target_before_upload() {
        let cfg = read_ci_config();
        let command = "cargo build --release --features full --verbose --timings";
        let builds: Vec<&str> = cfg
            .split("- run:")
            .filter(|step| step.contains(command))
            .collect();
        assert_eq!(builds.len(), 4, "se esperaban 4 builds con --timings");
        for step in builds {
            let build = step.find(command).unwrap();
            let clear = step
                .find("rm -rf -- target/cargo-timings")
                .or_else(|| step.find("Remove-Item -Recurse -Force target\\cargo-timings"));
            let moved = step
                .find("mv target/cargo-timings cargo-timings")
                .or_else(|| step.find("Move-Item target\\cargo-timings cargo-timings"));
            assert!(
                clear.is_some_and(|i| i < build) && moved.is_some_and(|i| i > build),
                "el paso con --timings debe vaciar target/cargo-timings antes de compilar y moverlo fuera de target/ después:\n{step}"
            );
        }
        assert!(
            !cfg.contains("path: target/cargo-timings"),
            "los informes de --timings se suben desde fuera de target/"
        );
    }

    /// Sección de la definición de un job (hasta el siguiente job).
    fn build_section<'a>(cfg: &'a str, job: &str) -> &'a str {
        ci_jobs(cfg)
            .into_iter()
            .find(|(name, _)| *name == job)
            .map_or("", |(_, section)| section)
    }

    fn indent(l: &str) -> usize {
        l.len() - l.trim_start().len()
    }

    /// La traza de diagnóstico del arranque es permanente y vive en el job
    /// ordinario `test-windows`: el volcado debe correr SIEMPRE (pase o falle
    /// el caso), porque cuando el defecto no se reproduce la cronología de la
    /// fase lenta es el dato que explica por qué. Y la traza debe caer en el
    /// directorio del job, no en el sandbox de la prueba: el sandbox se borra
    /// al terminar el caso, y con ella la evidencia.
    #[test]
    fn test_daemon_start_trace_is_dumped_always_in_test_windows() {
        let cfg = read_ci_config();
        let windows = build_section(&cfg, "test-windows");
        assert!(
            windows.contains("Volcar la traza de diagnóstico del arranque"),
            "test-windows debe tener el paso que vuelca la traza"
        );
        assert!(
            windows.contains("when: always"),
            "el volcado de la traza debe correr siempre (pase o falle el caso)"
        );
        assert!(
            windows.contains("AVI_DIAG_TRACE_DIR"),
            "la traza debe fijarse en un directorio que sobreviva al sandbox"
        );
        // La traza vive en el directorio de trabajo del job. `CIRCLE_WORKING_DIRECTORY`
        // vale lo que el job declare en `working_directory`, y `test-windows` no lo
        // declara: la variable puede no existir o apuntar a otro sitio, y entonces
        // la traza se escribiría fuera del directorio de trabajo y el volcado
        // final no la recogería. La resolución debe caer al directorio de
        // trabajo actual y comprobar que el directorio existe antes de usarlo.
        assert!(
            !windows.contains("working_directory:"),
            "test-windows no debe declarar working_directory: la resolución de la traza asume que el directorio de trabajo es el actual"
        );
        assert!(
            windows.contains("(Get-Location).Path"),
            "la traza debe resolverse al directorio de trabajo actual como respaldo"
        );
        assert!(
            windows.contains("Test-Path -PathType Container $env:CIRCLE_WORKING_DIRECTORY"),
            "CIRCLE_WORKING_DIRECTORY solo se usa si existe de verdad"
        );
        // La corrida de la suite no puede cambiar: el defecto depende de la
        // condición de la corrida completa.
        assert!(
            windows.contains("cargo nextest run --all\n"),
            "la corrida de la suite debe ser la de producción, sin filtros:\n{windows}"
        );
    }

    /// `test-windows` compila con un único entorno en todos sus pasos: los build
    /// scripts de `cc` (aws-lc-sys, blake3) vigilan CC, CXX y las variables que
    /// fija vcvars64, así que importarlo solo en la suite recompila toda la
    /// cadena de `cc` entre los pasos de `xtask` y la suite.
    #[test]
    fn test_test_windows_compiles_with_one_environment() {
        let cfg = read_ci_config();
        let windows = build_section(&cfg, "test-windows");
        for forbidden in ["vcvars64.bat", "$env:CC", "$env:CXX"] {
            assert!(
                !windows.contains(forbidden),
                "test-windows no debe contener `{forbidden}`: cambiaría el entorno de `cc` entre pasos"
            );
        }
    }

    /// La instrumentación de diagnóstico es permanente y acotada: solo cuatro
    /// casos de `daemon start` la activan, y el resto de la suite no escribe
    /// traza. Un caso de más añade coste a cada corrida de CI.
    #[test]
    fn test_daemon_start_trace_is_enabled_only_in_the_four_focused_cases() {
        let tests = std::fs::read_to_string("../../tests/cli_golden.rs")
            .or_else(|_| {
                std::fs::read_to_string(
                    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("../../tests/cli_golden.rs"),
                )
            })
            .expect("no se pudo leer tests/cli_golden.rs");
        let calls = tests.matches(".enable_diag_trace(").count();
        assert_eq!(
            calls, 4,
            "solo los cuatro casos de `daemon start` afectados activan la traza: \
             se encontraron {calls} llamadas"
        );
        for tag in [
            "start_port_in_use",
            "start_failure_log",
            "start_missing_warm_voice",
            "start_reclaims_unresponsive",
        ] {
            assert!(
                tests.contains(&format!("enable_diag_trace(\"{tag}\")")),
                "el caso `{tag}` debe activar la traza del arranque"
            );
        }
    }

    /// Launcher sccache para los proyectos CMake de los build-*, incondicional:
    /// en Unix vía CMAKE_{C,CXX}_COMPILER_LAUNCHER exportados a $BASH_ENV; en
    /// Windows además con el generador Ninja (el de Visual Studio ignora los
    /// launchers), el entorno vcvars64 y CC/CXX con la ruta absoluta de cl.exe.
    #[test]
    fn test_cmake_launcher_in_builds() {
        let cfg = read_ci_config();
        assert!(
            !cfg.contains("native_sccache:")
                && !cfg.contains("pipeline.parameters.native_sccache "),
            "el launcher es incondicional: no debe existir el parámetro native_sccache"
        );
        let unix = cfg
            .split("\n  native_sccache_setup_unix:\n")
            .nth(1)
            .expect("debe existir el comando native_sccache_setup_unix")
            .split("\n  native_sccache_setup_windows:")
            .next()
            .unwrap_or("");
        for var in [
            "CMAKE_C_COMPILER_LAUNCHER=sccache",
            "CMAKE_CXX_COMPILER_LAUNCHER=sccache",
        ] {
            assert!(
                unix.contains(&format!("echo 'export {var}' >> \"$BASH_ENV\"")),
                "native_sccache_setup_unix debe exportar {var} a $BASH_ENV"
            );
        }
        for job in ["build-linux-x64", "build-linux-arm64", "build-darwin-arm64"] {
            let section = build_section(&cfg, job);
            let setup = section
                .find("      - sccache_setup_unix\n      - native_sccache_setup_unix\n")
                .unwrap_or_else(|| {
                    panic!("{job} debe invocar native_sccache_setup_unix tras sccache_setup_unix")
                });
            let build = section
                .find("cargo build --release --features")
                .unwrap_or_else(|| panic!("{job} debe compilar el binario release"));
            assert!(
                setup < build,
                "{job}: el launcher debe configurarse antes de compilar"
            );
        }
        let win = build_section(&cfg, "build-windows-x64");
        assert!(
            win.contains("      - sccache_setup_windows\n      - native_sccache_setup_windows\n"),
            "build-windows-x64 debe instalar Ninja (native_sccache_setup_windows) sin condición"
        );
        let compile_step = win
            .split("name: Compilar binario release (--features full)")
            .nth(1)
            .expect("build-windows-x64 debe tener el paso de compilación")
            .split("\n      - ")
            .next()
            .unwrap_or("");
        for assignment in [
            r#"$env:CMAKE_GENERATOR = "Ninja""#,
            r#"$env:CMAKE_C_COMPILER_LAUNCHER = "sccache""#,
            r#"$env:CMAKE_CXX_COMPILER_LAUNCHER = "sccache""#,
            r#"$env:PATH = "$env:TEMP\ninja;$env:USERPROFILE\.cargo\bin;$env:PATH""#,
            r#"$env:CC = $Cl"#,
            r#"$env:CXX = $Cl"#,
            r#"\VC\Auxiliary\Build\vcvars64.bat""#,
        ] {
            assert!(
                compile_step.contains(assignment),
                "el paso de compilación de Windows debe fijar `{assignment}`"
            );
        }
        assert!(
            !compile_step.contains("<< pipeline.parameters."),
            "el entorno de compilación de Windows no debe depender de parámetros de pipeline"
        );
        let pos_vcvars = compile_step.find("vcvars64.bat").unwrap();
        let pos_path = compile_step
            .find(r#"$env:PATH = "$env:TEMP\ninja"#)
            .unwrap();
        let pos_launcher = compile_step.find("$env:CMAKE_GENERATOR").unwrap();
        let pos_build = compile_step
            .find("cargo build --release --features")
            .unwrap();
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
    fn test_parse_iso_date_accepts_only_iso_dates() {
        assert_eq!(parse_iso_date("2026-10-09\n").unwrap(), "2026-10-09");
        assert!(parse_iso_date("").is_err());
        assert!(parse_iso_date("0000-00-0").is_err());
        assert!(parse_iso_date("09/10/2026").is_err());
    }

    #[test]
    fn test_engine_bin_name_by_platform() {
        if cfg!(windows) {
            assert_eq!(engine_bin_name(), "qwen_tts.exe");
        } else {
            assert_eq!(engine_bin_name(), "qwen_tts");
        }
    }

    #[test]
    fn test_make_program_by_platform() {
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
    fn test_slug_matches_github_anchor() {
        // GitHub ancla `## [0.20.12] — 2026-09-23` como `#02012--2026-09-23`.
        assert_eq!(slug("0.20.12", "2026-09-23"), "02012--2026-09-23");
        assert_eq!(slug("0.20.3", "2026-09-22"), "0203--2026-09-22");
    }

    #[test]
    fn test_promote_changelog_happy_path() {
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
    fn test_promote_changelog_fails_without_unreleased() {
        // Un CHANGELOG ya promovido no tiene sección [No publicado] que promover.
        let promoted =
            promote_changelog_text(&sample_changelog(), "0.20.4", "0.20.3", "2026-09-24").unwrap();
        let err = promote_changelog_text(&promoted, "0.20.5", "0.20.4", "2026-09-25").unwrap_err();
        assert!(err.to_string().contains("No publicado"));
    }

    #[test]
    fn test_promote_changelog_fails_with_leftover_todo() {
        let with_todo = sample_changelog().replace(
            "- algo curado a mano durante el desarrollo.",
            "- algo a medias.  <!-- TODO: curar -->",
        );
        let err = promote_changelog_text(&with_todo, "0.20.4", "0.20.3", "2026-09-24").unwrap_err();
        assert!(err.to_string().contains("TODO: curar"));
    }

    #[test]
    fn test_promote_changelog_fails_if_version_already_exists() {
        // La versión objetivo coincide con una sección de versión ya presente.
        let err = promote_changelog_text(&sample_changelog(), "0.20.3", "0.20.2", "2026-09-24")
            .unwrap_err();
        assert!(err.to_string().contains("ya existe"));
    }

    #[test]
    fn test_validate_changelog_promoted_passes() {
        let promoted =
            promote_changelog_text(&sample_changelog(), "0.20.4", "0.20.3", "2026-09-24").unwrap();
        assert!(validate_changelog_text(&promoted, "0.20.4").is_ok());
    }

    #[test]
    fn test_validate_changelog_fails_without_toc() {
        let promoted =
            promote_changelog_text(&sample_changelog(), "0.20.4", "0.20.3", "2026-09-24").unwrap();
        let without_toc = promoted.replace("- [0.20.4 — 2026-09-24](#0204--2026-09-24)\n", "");
        let err = validate_changelog_text(&without_toc, "0.20.4").unwrap_err();
        assert!(err.to_string().contains("tabla de contenidos"));
    }

    #[test]
    fn test_validate_changelog_fails_without_link() {
        let promoted =
            promote_changelog_text(&sample_changelog(), "0.20.4", "0.20.3", "2026-09-24").unwrap();
        let without_link = promoted.replace(
            "[0.20.4]: https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/compare/v0.20.3...v0.20.4\n",
            "",
        );
        let err = validate_changelog_text(&without_link, "0.20.4").unwrap_err();
        assert!(err.to_string().contains("enlace de comparación"));
    }

    #[test]
    fn test_validate_changelog_fails_with_leftover_todo() {
        let promoted =
            promote_changelog_text(&sample_changelog(), "0.20.4", "0.20.3", "2026-09-24").unwrap();
        let with_todo = promoted.replace(
            "- algo curado a mano durante el desarrollo.",
            "- algo a medias.  <!-- TODO: curar -->",
        );
        let err = validate_changelog_text(&with_todo, "0.20.4").unwrap_err();
        assert!(err.to_string().contains("TODO: curar"));
    }

    #[test]
    fn test_validate_changelog_fails_with_leftover_unreleased() {
        let promoted =
            promote_changelog_text(&sample_changelog(), "0.20.4", "0.20.3", "2026-09-24").unwrap();
        // Reintroducir una sección [No publicado] sin promover.
        let with_leftover = promoted.replace(
            "## [0.20.4] — 2026-09-24",
            "## [No publicado]\n\n### Cambiado\n\n- nuevo trabajo.\n\n## [0.20.4] — 2026-09-24",
        );
        let err = validate_changelog_text(&with_leftover, "0.20.4").unwrap_err();
        assert!(err.to_string().contains("No publicado"));
    }

    /// Regresión: una entrada de `[No publicado]` que menciona el literal
    /// `## [No publicado]` en su prosa (dentro de backticks) no debe producir un
    /// falso positivo. La promoción renombra solo la cabecera real y la validación
    /// posterior pasa. Reproduce el defecto expuesto por el corte de v0.20.4.
    #[test]
    fn test_promotion_and_validation_ignore_prose_mentions() {
        let with_prose = sample_changelog().replace(
            "- algo curado a mano durante el desarrollo.",
            "- redefine `release`: renombra la cabecera `## [No publicado]` a `## [X.Y.Z]`.",
        );
        let promoted =
            promote_changelog_text(&with_prose, "0.20.4", "0.20.3", "2026-09-24").unwrap();
        // La cabecera real se promovió; la mención en prosa se conserva intacta.
        assert!(promoted.contains("## [0.20.4] — 2026-09-24"));
        assert!(promoted.contains("renombra la cabecera `## [No publicado]`"));
        // Y la validación NO da falso positivo por esa mención en backticks.
        assert!(validate_changelog_text(&promoted, "0.20.4").is_ok());
    }

    #[test]
    fn test_validate_changelog_anchors_passes_on_promoted() {
        let promoted =
            promote_changelog_text(&sample_changelog(), "0.20.4", "0.20.3", "2026-09-24").unwrap();
        assert!(validate_changelog_anchors(&promoted).is_ok());
    }

    #[test]
    fn test_validate_changelog_anchors_fails_with_old_anchor_without_double_dash() {
        let promoted =
            promote_changelog_text(&sample_changelog(), "0.20.4", "0.20.3", "2026-09-24").unwrap();
        // Ancla que no coincide con la que GitHub asigna a la cabecera.
        let broken_anchor = promoted.replace(
            "- [0.20.4 — 2026-09-24](#0204--2026-09-24)",
            "- [0.20.4 — 2026-09-24](#0204-20260924)",
        );
        let err = validate_changelog_anchors(&broken_anchor).unwrap_err();
        assert!(err.to_string().contains("0204--2026-09-24"));
        // El invariante de anclas también se ejerce desde validate_changelog_text.
        let err = validate_changelog_text(&broken_anchor, "0.20.4").unwrap_err();
        assert!(err.to_string().contains("0204--2026-09-24"));
    }

    #[test]
    fn test_validate_changelog_anchors_fails_if_toc_entry_missing() {
        // La cabecera de [0.20.3] existe pero su entrada de ToC no está (caso
        // distinto de "ancla rota": aquí no hay ninguna línea para esa versión).
        let without_entry =
            sample_changelog().replace("- [0.20.3 — 2026-09-22](#0203--2026-09-22)\n", "");
        let err = validate_changelog_anchors(&without_entry).unwrap_err();
        assert!(err.to_string().contains("0203--2026-09-22"));
    }

    /// Pines reales del repositorio: la oferta se prueba contra la misma fuente que usa.
    fn repo_pins() -> Pins {
        doctor::load_pins(&Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")).unwrap()
    }

    fn sample_mpl_crates() -> Vec<(String, String)> {
        vec![("ejemplo-mpl".to_string(), "1.2.0".to_string())]
    }

    fn render_offer(version: &str) -> String {
        render_source_offer(version, &repo_pins(), &sample_mpl_crates()).unwrap()
    }

    #[test]
    fn test_diff_source_offer_matches() {
        let rendered = render_offer("1.2.3");
        assert!(diff_source_offer(&rendered, &rendered).is_ok());
    }

    #[test]
    fn test_diff_source_offer_normalizes_crlf() {
        let rendered = render_offer("1.2.3");
        let with_crlf = rendered.replace('\n', "\r\n");
        assert!(diff_source_offer(&with_crlf, &rendered).is_ok());
    }

    #[test]
    fn test_diff_source_offer_out_of_sync() {
        let rendered = render_offer("1.2.3");
        let old = render_offer("1.2.2");
        let err = diff_source_offer(&old, &rendered).unwrap_err();
        assert!(err.contains("SOURCE-OFFER.md desincronizado"));
    }

    /// La oferta cita la versión y el build de FFmpeg fijados, los assets de fuentes y de
    /// configuración, los crates MPL-2.0 de terceros y solo Issues como canal.
    #[test]
    fn test_source_offer_is_derived_from_the_pins() {
        let pins = repo_pins();
        let offer = render_offer("1.2.3");
        assert!(offer.contains(&format!("FFmpeg {}", pins.ffmpeg)));
        assert!(offer.contains(package::FFMPEG_BTBN_BUILD));
        assert!(offer.contains(package::FFMPEG_BTBN_TAG));
        let (sources, buildconfs) = ffmpeg_release_assets(&pins);
        for asset in sources.iter().chain(&buildconfs) {
            assert!(
                offer.contains(&release_asset_url("1.2.3", asset)),
                "falta el asset {asset}"
            );
        }
        assert!(offer.contains("https://crates.io/crates/ejemplo-mpl/1.2.0"));
        assert!(offer.contains(&format!("https://github.com/{GITHUB_REPO}/issues")));
        assert!(!offer.contains("contactando al mantenedor"));
        assert!(!offer.contains('{'), "marcador sin resolver en la oferta");
    }

    #[test]
    fn test_source_offer_follows_a_pin_change() {
        let mut pins = repo_pins();
        pins.ffmpeg = "10.1".to_string();
        let offer = render_source_offer("1.2.3", &pins, &[]).unwrap();
        assert!(offer.contains("ffmpeg-10.1.tar.xz"));
        assert!(!offer.contains("ffmpeg-9.0.2.tar.xz"));
        assert!(!offer.contains("Crates de terceros bajo MPL-2.0"));
    }

    #[test]
    fn test_release_notes_footer_names_the_tag_tarball_and_the_source_assets() {
        let pins = repo_pins();
        let footer = render_release_notes_footer("v1.2.3", &pins);
        assert!(footer.contains(&format!(
            "https://github.com/{GITHUB_REPO}/archive/refs/tags/v1.2.3.tar.gz"
        )));
        let (sources, buildconfs) = ffmpeg_release_assets(&pins);
        for asset in sources.iter().chain(&buildconfs) {
            assert!(footer.contains(&release_asset_url("1.2.3", asset)));
        }
    }

    #[test]
    fn test_mpl_third_party_crates_are_selected_by_license() {
        let package = |name: &str, license: &str| DistPackage {
            name: name.to_string(),
            version: "1.0.0".to_string(),
            license: Some(license.to_string()),
            manifest_dir: PathBuf::new(),
        };
        let crates = mpl_third_party_crates(&[
            package("a", "MIT"),
            package("b", "MPL-2.0"),
            package("c", "MIT OR Apache-2.0"),
        ])
        .unwrap();
        assert_eq!(crates, [("b".to_string(), "1.0.0".to_string())]);
    }

    /// Paquete del fixture de `cargo metadata`: `(nombre, versión, licencia, miembro del
    /// espacio de trabajo)`.
    type FixturePackage = (&'static str, &'static str, Option<&'static str>, bool);

    /// Salida mínima de `cargo metadata` con un paquete raíz propio, dos versiones de
    /// `cfg-if` como dependencias normales, un proc-macro, un paquete propio bajo otra
    /// licencia y un propio bajo la del proyecto, más una dependencia de desarrollo, una
    /// de compilación y una transitiva de esta última, que no se distribuyen.
    fn fixture_metadata(
        packages: &[FixturePackage],
        edges: &[(&str, &str, Option<&str>)],
    ) -> serde_json::Value {
        let id = |name: &str, version: &str| format!("{name} {version}");
        let package_list: Vec<_> = packages
            .iter()
            .map(|(name, version, license, _)| {
                serde_json::json!({
                    "id": id(name, version),
                    "name": name,
                    "version": version,
                    "license": license,
                    "manifest_path": format!("/registro/{name}-{version}/Cargo.toml"),
                })
            })
            .collect();
        let nodes: Vec<_> = packages
            .iter()
            .map(|(name, version, _, _)| {
                let deps: Vec<_> = edges
                    .iter()
                    .filter(|(from, _, _)| from.starts_with(&format!("{name} ")))
                    .map(|(_, to, kind)| {
                        serde_json::json!({
                            "pkg": to,
                            "dep_kinds": [{"kind": kind, "target": null}],
                        })
                    })
                    .collect();
                serde_json::json!({"id": id(name, version), "deps": deps})
            })
            .collect();
        let members: Vec<_> = packages
            .iter()
            .filter(|(_, _, _, member)| *member)
            .map(|(name, version, _, _)| id(name, version))
            .collect();
        serde_json::json!({
            "packages": package_list,
            "workspace_members": members,
            "resolve": {"root": id(packages[0].0, packages[0].1), "nodes": nodes},
        })
    }

    fn sample_metadata() -> serde_json::Value {
        fixture_metadata(
            &[
                ("ai-voice-interconnector", "1.0.0", Some("MPL-2.0"), true),
                ("cfg-if", "0.1.10", Some("MIT/Apache-2.0"), false),
                ("cfg-if", "1.0.4", Some("MIT OR Apache-2.0"), false),
                ("macros", "0.2.0", Some("MIT"), false),
                ("denoise", "1.0.0", Some("MIT"), true),
                ("avi-core", "0.1.0", Some("MPL-2.0"), true),
                ("dev-only", "1.0.0", Some("MIT"), false),
                ("build-only", "1.0.0", Some("MIT"), false),
                ("build-helper", "1.0.0", Some("Zlib"), false),
            ],
            &[
                ("ai-voice-interconnector 1.0.0", "cfg-if 1.0.4", None),
                ("ai-voice-interconnector 1.0.0", "macros 0.2.0", None),
                ("ai-voice-interconnector 1.0.0", "denoise 1.0.0", None),
                ("ai-voice-interconnector 1.0.0", "avi-core 0.1.0", None),
                (
                    "ai-voice-interconnector 1.0.0",
                    "dev-only 1.0.0",
                    Some("dev"),
                ),
                (
                    "ai-voice-interconnector 1.0.0",
                    "build-only 1.0.0",
                    Some("build"),
                ),
                ("macros 0.2.0", "cfg-if 0.1.10", None),
                ("build-only 1.0.0", "build-helper 1.0.0", None),
            ],
        )
    }

    fn sample_packages() -> Vec<DistPackage> {
        distributed_packages(&[sample_metadata()]).unwrap()
    }

    fn sample_region() -> String {
        render_licenses_inventory(&sample_packages()).unwrap()
    }

    const SAMPLE_NOTICES: &str = "### Texto 1: MIT\n\nTexto de prueba.\n";

    fn sample_licenses_doc() -> String {
        format!(
            "# Licencias\n\nProsa a mano.\n\n{}\n{}{}\n\n## Avisos\n\n{}\n{}{}\n\n## Regeneración\n",
            INVENTORY_START,
            sample_region(),
            INVENTORY_END,
            notices::NOTICES_START,
            SAMPLE_NOTICES,
            notices::NOTICES_END
        )
    }

    #[test]
    fn test_distributed_packages_only_follow_normal_edges() {
        let names: Vec<String> = sample_packages()
            .iter()
            .map(|p| format!("{} {}", p.name, p.version))
            .collect();
        // Entran las dependencias normales (también las de un proc-macro) y el crate
        // propio bajo otra licencia; quedan fuera la raíz, el crate propio bajo la
        // licencia del proyecto, la dependencia de desarrollo y la de compilación con
        // su transitiva.
        assert_eq!(
            names,
            [
                "cfg-if 0.1.10",
                "cfg-if 1.0.4",
                "denoise 1.0.0",
                "macros 0.2.0"
            ]
        );
    }

    #[test]
    fn test_distributed_packages_union_across_triples() {
        let other = fixture_metadata(
            &[
                ("ai-voice-interconnector", "1.0.0", Some("MPL-2.0"), true),
                ("windows-sys", "0.52.0", Some("MIT OR Apache-2.0"), false),
                ("cfg-if", "1.0.4", Some("MIT OR Apache-2.0"), false),
            ],
            &[
                ("ai-voice-interconnector 1.0.0", "windows-sys 0.52.0", None),
                ("ai-voice-interconnector 1.0.0", "cfg-if 1.0.4", None),
            ],
        );
        let packages = distributed_packages(&[sample_metadata(), other]).unwrap();
        let names: Vec<String> = packages.iter().map(|p| p.name.clone()).collect();
        assert_eq!(names.iter().filter(|n| *n == "cfg-if").count(), 2);
        assert!(names.contains(&"windows-sys".to_string()));
        assert_eq!(packages.len(), 5);
    }

    #[test]
    fn test_license_family() {
        assert_eq!(license_family("MIT OR Apache-2.0"), Some("MIT"));
        assert_eq!(license_family("Apache-2.0 / MIT"), Some("MIT"));
        assert_eq!(license_family("Apache-2.0 AND ISC"), Some("Apache-2.0"));
        assert_eq!(license_family("BSD-3-Clause"), Some("BSD"));
        assert_eq!(license_family("Zlib"), Some("Zlib"));
        assert_eq!(license_family("GPL-3.0-or-later"), None);
        assert_eq!(license_family("WTFPL"), None);
    }

    #[test]
    fn test_render_inventory_two_versions() {
        let region = sample_region();
        assert!(region.contains("| `cfg-if` | 0.1.10 | MIT/Apache-2.0 | MIT |\n"));
        assert!(region.contains("| `cfg-if` | 1.0.4 | MIT OR Apache-2.0 | MIT |\n"));
        assert!(region.contains("| `denoise` | 1.0.0 | MIT | MIT |\n"));
        assert!(!region.contains("dev-only"));
        assert!(!region.contains("build-helper"));
    }

    #[test]
    fn test_render_inventory_count_and_summary() {
        let region = sample_region();
        assert!(region.starts_with(
            "Generado desde el grafo de dependencias distribuidas (4 paquetes resueltos, 3 crates únicos)"
        ));
        assert!(region.contains("Resumen por familia (paquetes resueltos): MIT 4.\n"));
    }

    #[test]
    fn test_render_inventory_fails_with_unknown_expression() {
        let mut packages = sample_packages();
        packages[0].license = Some("WTFPL".to_string());
        let err = render_licenses_inventory(&packages).unwrap_err();
        assert!(err.to_string().contains("cfg-if"));
        assert!(err.to_string().contains("WTFPL"));
    }

    #[test]
    fn test_render_inventory_fails_without_license() {
        let mut packages = sample_packages();
        packages[0].license = None;
        let err = render_licenses_inventory(&packages).unwrap_err();
        assert!(err.to_string().contains("cfg-if 0.1.10"));
    }

    #[test]
    fn test_replace_region_preserves_outer_text() {
        let doc = format!(
            "antes\n{}\nviejo\n{}\ndespués\n",
            INVENTORY_START, INVENTORY_END
        );
        let out = replace_region(&doc, INVENTORY_START, INVENTORY_END, "nuevo\n").unwrap();
        assert_eq!(
            out,
            format!(
                "antes\n{}\nnuevo\n{}\ndespués\n",
                INVENTORY_START, INVENTORY_END
            )
        );
    }

    #[test]
    fn test_replace_region_fails_without_markers() {
        assert!(replace_region("sin marcadores\n", INVENTORY_START, INVENTORY_END, "x\n").is_err());
        let only_start = format!("{}\nx\n", INVENTORY_START);
        let err = replace_region(&only_start, INVENTORY_START, INVENTORY_END, "x\n").unwrap_err();
        assert!(err.to_string().contains(INVENTORY_END));
    }

    #[test]
    fn test_replace_licenses_regions_needs_both_regions() {
        let only_inventory = format!("{}\nx\n{}\n", INVENTORY_START, INVENTORY_END);
        let err = replace_licenses_regions(&only_inventory, "a\n", "b\n").unwrap_err();
        assert!(err.to_string().contains(notices::NOTICES_START));
    }

    #[test]
    fn test_diff_licenses_doc_synced_and_crlf() {
        let doc = sample_licenses_doc();
        assert!(diff_licenses_doc(&doc, &sample_region(), SAMPLE_NOTICES).is_ok());
        let with_crlf = doc.replace('\n', "\r\n");
        assert!(diff_licenses_doc(&with_crlf, &sample_region(), SAMPLE_NOTICES).is_ok());
    }

    #[test]
    fn test_diff_licenses_doc_detects_version_license_and_notice() {
        let doc = sample_licenses_doc();
        let old_version = doc.replace("| `denoise` | 1.0.0 |", "| `denoise` | 0.9.0 |");
        let err = diff_licenses_doc(&old_version, &sample_region(), SAMPLE_NOTICES).unwrap_err();
        assert!(err.contains("desincronizado"));
        assert!(err.contains("+| `denoise` | 1.0.0 | MIT | MIT |"));
        let bad_license = doc.replace(
            "| `denoise` | 1.0.0 | MIT | MIT |",
            "| `denoise` | 1.0.0 | Zlib | Zlib |",
        );
        let err = diff_licenses_doc(&bad_license, &sample_region(), SAMPLE_NOTICES).unwrap_err();
        assert!(err.contains("-| `denoise` | 1.0.0 | Zlib | Zlib |"));
        let old_notice = doc.replace("Texto de prueba.", "Texto viejo.");
        let err = diff_licenses_doc(&old_notice, &sample_region(), SAMPLE_NOTICES).unwrap_err();
        assert!(err.contains("+Texto de prueba."));
    }

    /// Renderiza el `--help` de un nodo vía `try_get_matches_from`,
    /// capturando el `DisplayHelp` como texto (misma ruta que el binario).
    fn help_text(node: &[&str]) -> String {
        let mut args = node.to_vec();
        args.push("--help");
        let err = localize_command(Cli::command())
            .try_get_matches_from(&args)
            .expect_err("--help debe salir como DisplayHelp");
        err.render().to_string()
    }

    /// Ayuda de la raíz y de cada subcomando en español, sin textos de clap.
    #[test]
    fn help_output_is_spanish() {
        for node in [
            vec!["xtask"],
            vec!["xtask", "cask"],
            vec!["xtask", "source-offer"],
            vec!["xtask", "licenses"],
            vec!["xtask", "release"],
            vec!["xtask", "changelog"],
            vec!["xtask", "language"],
            vec!["xtask", "comments"],
            vec!["xtask", "build-engine"],
            vec!["xtask", "package"],
            vec!["xtask", "clean"],
            vec!["xtask", "doctor"],
            vec!["xtask", "bootstrap"],
            vec!["xtask", "install"],
        ] {
            let help = help_text(&node);
            assert!(help.contains("Uso:"), "{:?} debe contener 'Uso:'", node);
            assert!(
                help.contains("Opciones:") || help.contains("Argumentos:"),
                "{:?} debe contener encabezado en español",
                node
            );
            for forbidden in [
                "Usage:",
                "Options:",
                "Arguments:",
                "Commands:",
                "Print help",
                "Print version",
                "[default:",
                "[possible values:",
            ] {
                assert!(
                    !help.contains(forbidden),
                    "{:?} no debe contener '{}'",
                    node,
                    forbidden
                );
            }
        }
    }

    /// El subcomando automático `help` está deshabilitado.
    #[test]
    fn help_subcommand_is_disabled() {
        let err = localize_command(Cli::command())
            .try_get_matches_from(["xtask", "help"])
            .expect_err("help debe fallar");
        assert_eq!(err.kind(), clap::error::ErrorKind::InvalidSubcommand);
    }
}
