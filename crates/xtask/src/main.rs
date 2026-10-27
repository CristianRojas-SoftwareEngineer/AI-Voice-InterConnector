// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

#![cfg_attr(not(test), deny(clippy::let_underscore_must_use))]

use anyhow::{anyhow, Result};
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
mod install;
mod language;
mod package;
mod pins;

const GITHUB_REPO: &str = "CristianRojas-SoftwareEngineer/AI-Voice-InterConnector";
const CASK_NAME: &str = "ai-voice-interconnector";

const CASK_TEMPLATE: &str = r#"cask "{cask_name}" do
  version "{cask_version}"
  sha256 "{cask_sha256}"

  url "https://github.com/{repo}/releases/download/v#{version}/ai-voice-interconnector-#{version}-arm64-macos.tar.gz"
  name "AI Voice-InterConnector"
  desc "Motor de síntesis de voz (TTS) offline con clonación de voz en español latinoamericano"
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
empaquetan en el binario: se descargan con `setup` y conservan sus licencias
(Qwen3-TTS MIT/Apache-2.0, opus-mt CC-BY-4.0).

El paquete incluye **FFmpeg 9.0.2** como ejecutable independiente: la aplicación
lo invoca como proceso y no lo enlaza con el binario principal. Los builds
distribuidos son GPL-3.0-or-later. Su código fuente correspondiente está
disponible en:

- **FFmpeg 9.0.2 (fuente upstream):**
  <https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz>
- **Builds Linux (BtbN) y scripts de compilación:**
  <https://github.com/BtbN/FFmpeg-Builds/releases/tag/autobuild-2026-10-24-13-02>
  (build `n9.0.2-35-g9a9b70bb8c`)
- **Build Windows (Gyan):**
  <https://github.com/GyanD/codexffmpeg/releases/tag/9.0.2>
  (el mismo binario se publica en <https://www.gyan.dev/ffmpeg/builds/>)
- **Build macOS (Martin Riedl):** <https://ffmpeg.martin-riedl.de/>

Si necesitas el árbol de fuentes exacto de un build, incluidas las bibliotecas
que enlaza, solicítalo abriendo un issue en el repositorio o contactando al
mantenedor. La oferta se mantiene durante al menos tres años desde la entrega
del binario.

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
    },
    /// Genera o verifica el inventario de THIRD-PARTY-LICENSES.md
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
    },
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
        Commands::SourceOffer { check } => {
            let version = get_version()?;
            if check {
                check_source_offer(&version)?;
                println!("SOURCE-OFFER.md en sincronía");
            } else {
                print!("{}", render_source_offer(&version));
            }
        }
        Commands::Licenses { check } => {
            if check {
                check_licenses_gate()?;
                println!("THIRD-PARTY-LICENSES.md está en sincronía con Cargo.lock");
            } else {
                write_licenses_inventory()?;
                println!("Inventario de THIRD-PARTY-LICENSES.md regenerado: revisa el diff");
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
            // El inventario incluye la versión del crate raíz: regenerarlo tras el bump.
            write_licenses_inventory()?;
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
            println!("  - THIRD-PARTY-LICENSES.md (inventario regenerado)");
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
        Commands::Ffmpeg { dest } => package::run_ffmpeg(dest.as_deref())?,
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

    // SOURCE-OFFER.md: aviso MPL-2.0 §3.2 versionado
    let offer = render_source_offer(version);
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

fn render_cask(version: &str, sha256: &str) -> String {
    let zap = cask_zap_entries()
        .iter()
        .map(|entry| format!("    \"{entry}\","))
        .collect::<Vec<_>>()
        .join("\n");
    CASK_TEMPLATE
        .replace("{cask_name}", CASK_NAME)
        .replace("{cask_version}", version)
        .replace("{cask_sha256}", sha256)
        .replace("{repo}", GITHUB_REPO)
        .replace("{zap_entries}", &zap)
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

const LICENSES_DOC: &str = "THIRD-PARTY-LICENSES.md";
const INVENTORY_START: &str = "<!-- inventario:inicio -->";
const INVENTORY_END: &str = "<!-- inventario:fin -->";

/// Licencias de paquetes de `Cargo.lock` que `cargo metadata` no reporta (p. ej.,
/// `cxxbridge-cmd`, dependencia de artefacto de `cxx`). Solo se consultan para
/// paquetes ausentes del metadato.
const LICENSE_OVERRIDES: &[(&str, &str)] = &[("cxxbridge-cmd", "MIT OR Apache-2.0")];

/// Paquetes `(nombre, versión)` de `Cargo.lock`, en el orden del lock (una
/// entrada por versión resuelta).
fn cargo_lock_entries(text: &str) -> Vec<(String, String)> {
    let re =
        Regex::new(r#"\[\[package\]\]\s+name\s*=\s*"([^"]+)"\s+version\s*=\s*"([^"]+)""#).unwrap();
    re.captures_iter(text)
        .map(|c| (c[1].to_string(), c[2].to_string()))
        .collect()
}

/// Familia de una expresión de licencia: `MIT` si MIT (o su variante MIT-0) es
/// alternativa o componente; `Apache-2.0` si no hay MIT pero sí Apache; `BSD`
/// para BSD-2/3-Clause; la propia licencia en los demás casos. Tabla explícita:
/// `None` ante una expresión desconocida, que exige decidir su familia a mano.
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
        | "MIT OR Apache-2.0 OR LGPL-2.1-or-later"
        | "MIT OR Zlib OR Apache-2.0"
        | "MIT OR Apache-2.0 OR Zlib"
        | "Apache-2.0 OR MIT OR Zlib"
        | "Zlib OR Apache-2.0 OR MIT"
        | "Apache-2.0 OR BSL-1.0 OR MIT"
        | "Apache-2.0 OR ISC OR MIT"
        | "Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT"
        | "BSD-2-Clause OR Apache-2.0 OR MIT"
        | "BSD-2-Clause OR MIT OR Apache-2.0"
        | "BSD-3-Clause OR MIT OR Apache-2.0"
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
        "GPL-3.0-or-later" => "GPL-3.0-or-later",
        _ => return None,
    };
    Some(family)
}

/// Región del inventario (conteo, resumen por familia y tabla) a partir de los
/// paquetes del lock y de las licencias de `cargo metadata` por `(nombre,
/// versión)`; `None` como licencia = el paquete no declara `license`. Pura: falla
/// nombrando el paquete si no hay licencia o si su familia es desconocida.
fn render_licenses_inventory(
    entries: &[(String, String)],
    licenses: &HashMap<(String, String), Option<String>>,
) -> Result<String> {
    let mut rows = String::new();
    let mut families: BTreeMap<&str, usize> = BTreeMap::new();
    let mut names = HashSet::new();
    for (name, version) in entries {
        let license = match licenses.get(&(name.clone(), version.clone())) {
            Some(Some(license)) => license.as_str(),
            Some(None) => anyhow::bail!(
                "{} {} no declara `license` en su Cargo.toml: no hay licencia que registrar",
                name,
                version
            ),
            None => LICENSE_OVERRIDES
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, l)| *l)
                .ok_or_else(|| {
                    anyhow!(
                        "{} {} está en Cargo.lock pero no en cargo metadata: añade su licencia a LICENSE_OVERRIDES",
                        name,
                        version
                    )
                })?,
        };
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
        "Generado desde `Cargo.lock` ({} paquetes resueltos, {} crates únicos, directos y transitivos).\n\
         Resumen por familia (paquetes resueltos): {}.\n\
         \n\
         | Paquete | Versión | Licencia (metadato) | Familia |\n\
         |---------|---------|---------------------|--------|\n\
         {}",
        entries.len(),
        names.len(),
        summary.join(", "),
        rows
    ))
}

/// Reemplaza el contenido entre `INVENTORY_START` e `INVENTORY_END` por `region`,
/// conservando el texto exterior. Falla si falta alguno de los marcadores.
fn replace_inventory_region(doc: &str, region: &str) -> Result<String> {
    let start = doc.find(INVENTORY_START).ok_or_else(|| {
        anyhow!(
            "{} no tiene el marcador `{}`",
            LICENSES_DOC,
            INVENTORY_START
        )
    })? + INVENTORY_START.len();
    let end = doc[start..]
        .find(INVENTORY_END)
        .map(|i| start + i)
        .ok_or_else(|| anyhow!("{} no tiene el marcador `{}`", LICENSES_DOC, INVENTORY_END))?;
    Ok(format!("{}\n{}{}", &doc[..start], region, &doc[end..]))
}

/// Compara `current` (contenido vigente de THIRD-PARTY-LICENSES.md) contra el
/// documento con la región `region` recién generada, normalizando CRLF vs LF.
/// Pura. `Ok(())` si coinciden; si no, un mensaje accionable con diff.
fn diff_licenses_inventory(current: &str, region: &str) -> Result<(), String> {
    let norm_current = current.replace("\r\n", "\n");
    let expected = replace_inventory_region(&norm_current, region).map_err(|e| e.to_string())?;
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

/// Licencia declarada de cada paquete de `cargo metadata --all-features` (sin
/// `--all-features` se omiten los crates que solo entran por features opcionales).
fn metadata_licenses() -> Result<HashMap<(String, String), Option<String>>> {
    let out = std::process::Command::new("cargo")
        .args([
            "metadata",
            "--format-version",
            "1",
            "--locked",
            "--all-features",
        ])
        .output()
        .map_err(|e| anyhow!("no se pudo invocar cargo metadata: {}", e))?;
    if !out.status.success() {
        anyhow::bail!(
            "cargo metadata falló: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let json: serde_json::Value = serde_json::from_slice(&out.stdout)?;
    let packages = json["packages"]
        .as_array()
        .ok_or_else(|| anyhow!("cargo metadata no devolvió `packages`"))?;
    let mut licenses = HashMap::new();
    for p in packages {
        let (Some(name), Some(version)) = (p["name"].as_str(), p["version"].as_str()) else {
            anyhow::bail!("cargo metadata devolvió un paquete sin nombre o versión");
        };
        licenses.insert(
            (name.to_string(), version.to_string()),
            p["license"].as_str().map(str::to_string),
        );
    }
    Ok(licenses)
}

/// Región del inventario generada desde `Cargo.lock` y `cargo metadata`.
fn render_licenses_region() -> Result<String> {
    let lock_text = std::fs::read_to_string("Cargo.lock")?;
    render_licenses_inventory(&cargo_lock_entries(&lock_text), &metadata_licenses()?)
}

/// Reescribe la región del inventario de THIRD-PARTY-LICENSES.md.
fn write_licenses_inventory() -> Result<()> {
    let region = render_licenses_region()?;
    let doc = std::fs::read_to_string(LICENSES_DOC)?.replace("\r\n", "\n");
    std::fs::write(LICENSES_DOC, replace_inventory_region(&doc, &region)?)?;
    Ok(())
}

/// Gate de licencias: falla con el diff si la región del inventario de
/// `THIRD-PARTY-LICENSES.md` difiere de la generada (nombre, versión, licencia,
/// familia o totales). Envoltura de E/S sobre `diff_licenses_inventory`.
fn check_licenses_gate() -> Result<()> {
    let region = render_licenses_region()?;
    let current = std::fs::read_to_string(LICENSES_DOC)?;
    diff_licenses_inventory(&current, &region).map_err(|msg| anyhow!(msg))
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
/// Las pruebas cargan ONNX Runtime desde `ORT_DYLIB_PATH`; la gate asegura
/// `ort-bundle/` en la versión fijada y exporta la ruta del fichero de la
/// librería, que prevalece sobre el valor por defecto de la configuración de
/// cargo, de modo que valida el mismo runtime que se distribuye.
///
/// Además habilita la capa `<cwd>/vendor` (`AVI_ALLOW_CWD_VENDOR=1`): las
/// pruebas se ejecutan desde la raíz del repo y los hijos CLI que lanzan
/// resuelven el motor recién compilado en `vendor/qwen3-tts/`, sin exigir un
/// programa instalado. Sin este opt-in, un entorno limpio falla las 21 pruebas
/// del motor aunque todo esté provisionado.
fn check_resource_tests_gate() -> Result<()> {
    let root = std::env::current_dir()?;
    let pins = doctor::load_pins(&root)?;
    let triple = package::host_triple()?;
    let bundle = package::ensure_ort_bundle(&root, triple, &pins)?;
    let ort_dylib = package::ort_dylib_path(&bundle, triple)?;
    let status = std::process::Command::new("cargo")
        .env("ORT_DYLIB_PATH", &ort_dylib)
        .env("AVI_ALLOW_CWD_VENDOR", "1")
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
            "las pruebas con recursos fallaron y no se ha modificado nada: corrígelas antes de release (reprodúcelo con `AVI_ALLOW_CWD_VENDOR=1 ORT_DYLIB_PATH={} cargo test --workspace --features full -- --include-ignored`)",
            ort_dylib.display()
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
        let c = render_cask("1.2.3", &"a".repeat(64));
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
        assert!(c.contains("síntesis"));
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
        let cask = render_cask("9.9.9", &"b".repeat(64));
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
        let c = render_cask("9.9.9", &"b".repeat(64));
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

    /// Jobs que usan cargo: los que restauran `target/` o el registry.
    fn cargo_jobs(cfg: &str) -> Vec<(&str, &str)> {
        ci_jobs(cfg)
            .into_iter()
            .filter(|(_, s)| {
                s.contains("- cargo_restore_caches") || s.contains("- cargo_restore_registry")
            })
            .collect()
    }

    /// Ninguna compilación precede a la restauración de cachés: una restauración
    /// extraída sobre un `target/` ya poblado mezcla el estado de dos ejecuciones,
    /// y una compilación anterior al entorno completo no usa el registry ni el
    /// `sccache` del job. Las puertas `validate_*` son la primera compilación del
    /// workspace, juntas y tras la configuración de `sccache`.
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
                .position(|l| {
                    l.starts_with("- cargo_restore_caches")
                        || l.starts_with("- cargo_restore_registry")
                })
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
            let last_sccache = lines
                .iter()
                .rposition(|l| {
                    l.starts_with("- sccache_setup_") || l.starts_with("- native_sccache_setup_")
                })
                .unwrap_or_else(|| panic!("{job} debe configurar sccache"));
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
    fn test_pipeline_heterogeneous_and_unconditional_sccache() {
        let cfg = read_ci_config();
        // Modelo vigente (post remediación de caché): test-linux, test-windows, test-macos,
        // coverage y build-* usan cargo_restore_caches (registry + caché de target/) y sccache
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
            cfg.contains("sccache-v1-{{ arch }}-<< parameters.os >>-<< pipeline.parameters.rust_pin >>-<< parameters.variant >>-{{ epoch }}"),
            "sccache_save_cache debe usar clave rolling por epoch segmentada por variante"
        );
        assert!(
            cfg.contains("sccache-v1-{{ arch }}-<< parameters.os >>-<< pipeline.parameters.rust_pin >>-<< parameters.variant >>-"),
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

    /// `Cargo.lock.cachekey` se genera en un único command, que usan las dos
    /// restauraciones, y que falla si la versión del propio crate no se
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
        for restore in ["cargo_restore_caches", "cargo_restore_registry"] {
            let steps = cfg
                .split(&format!("\n  {restore}:"))
                .nth(1)
                .and_then(|s| s.split("\n    steps:\n").nth(1))
                .unwrap_or_else(|| panic!("{restore} debe tener pasos"));
            assert!(
                steps.starts_with("      - cargo_lock_cachekey\n"),
                "{restore} debe generar la clave antes de restaurar"
            );
        }
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
            .split("\n  cargo_restore_registry:")
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
            "no debe existir `find ... | head` directo: materializar la salida (`ort_matches=\"$(find ...)\"`) antes de aplicar head (regresión EPIPE v0.20.9)"
        );
    }

    /// La caché de `target/` usa clave inmutable (el primer `save_cache` gana): en
    /// build-* (variant: full), guardar con `when: always` persistiría para
    /// siempre un `target/` incompleto si `cargo build --release` falla a
    /// medias (ya ocurrió con linux-x64 en v0.20.9). test-*/coverage sí
    /// conservan `when: always` porque ahí un fallo del job suele ser un test
    /// que falló, no una compilación incompleta.
    #[test]
    fn test_cargo_save_target_on_success_in_build_variant_full() {
        let cfg = read_ci_config();
        // Cada bloque `cargo_save_target:` con `variant: full` debe traer
        // `when: on_success` en las mismas 3 líneas siguientes.
        let mut seen_full = 0;
        let lines: Vec<&str> = cfg.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            if line.trim() != "- cargo_save_target:" {
                continue;
            }
            let window = lines[i..(i + 5).min(lines.len())].join("\n");
            if window.contains("variant: full") {
                seen_full += 1;
                assert!(
                    window.contains("when: on_success"),
                    "cargo_save_target con variant: full debe pasar when: on_success (evita persistir target/ incompleto bajo clave inmutable); bloque:\n{window}"
                );
            }
        }
        // Los 4 build-* (windows-x64, linux-x64, linux-arm64, darwin-arm64).
        assert_eq!(
            seen_full, 4,
            "se esperaban 4 invocaciones de cargo_save_target con variant: full (una por build-*)"
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

    #[test]
    fn test_diff_source_offer_matches() {
        let rendered = render_source_offer("1.2.3");
        assert!(diff_source_offer(&rendered, &rendered).is_ok());
    }

    #[test]
    fn test_diff_source_offer_normalizes_crlf() {
        let rendered = render_source_offer("1.2.3");
        let with_crlf = rendered.replace('\n', "\r\n");
        assert!(diff_source_offer(&with_crlf, &rendered).is_ok());
    }

    #[test]
    fn test_diff_source_offer_out_of_sync() {
        let rendered = render_source_offer("1.2.3");
        let old = render_source_offer("1.2.2");
        let err = diff_source_offer(&old, &rendered).unwrap_err();
        assert!(err.contains("SOURCE-OFFER.md desincronizado"));
    }

    /// Lock mínimo con dos versiones de `cfg-if` y un paquete ausente del metadato.
    const SAMPLE_LOCK: &str = r#"version = 4

[[package]]
name = "cfg-if"
version = "0.1.10"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "cfg-if"
version = "1.0.4"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "cxxbridge-cmd"
version = "1.0.199"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "foldhash"
version = "0.1.5"
source = "registry+https://github.com/rust-lang/crates.io-index"
"#;

    fn sample_licenses() -> HashMap<(String, String), Option<String>> {
        [
            ("cfg-if", "0.1.10", "MIT/Apache-2.0"),
            ("cfg-if", "1.0.4", "MIT OR Apache-2.0"),
            ("foldhash", "0.1.5", "Zlib"),
        ]
        .into_iter()
        .map(|(n, v, l)| ((n.to_string(), v.to_string()), Some(l.to_string())))
        .collect()
    }

    fn sample_region() -> String {
        render_licenses_inventory(&cargo_lock_entries(SAMPLE_LOCK), &sample_licenses()).unwrap()
    }

    fn sample_licenses_doc() -> String {
        format!(
            "# Licencias\n\nProsa a mano.\n\n{}\n{}{}\n\n## Regeneración\n",
            INVENTORY_START,
            sample_region(),
            INVENTORY_END
        )
    }

    #[test]
    fn test_cargo_lock_entries_one_per_version_in_order() {
        let entries = cargo_lock_entries(SAMPLE_LOCK);
        let expected: Vec<(String, String)> = [
            ("cfg-if", "0.1.10"),
            ("cfg-if", "1.0.4"),
            ("cxxbridge-cmd", "1.0.199"),
            ("foldhash", "0.1.5"),
        ]
        .iter()
        .map(|(n, v)| (n.to_string(), v.to_string()))
        .collect();
        assert_eq!(entries, expected);
    }

    #[test]
    fn test_license_family() {
        assert_eq!(license_family("MIT OR Apache-2.0"), Some("MIT"));
        assert_eq!(license_family("Apache-2.0 / MIT"), Some("MIT"));
        assert_eq!(license_family("Apache-2.0 AND ISC"), Some("Apache-2.0"));
        assert_eq!(license_family("BSD-3-Clause"), Some("BSD"));
        assert_eq!(license_family("Zlib"), Some("Zlib"));
        assert_eq!(license_family("GPL-3.0-or-later"), Some("GPL-3.0-or-later"));
        assert_eq!(license_family("WTFPL"), None);
    }

    #[test]
    fn test_render_inventory_two_versions_and_override() {
        let region = sample_region();
        assert!(region.contains("| `cfg-if` | 0.1.10 | MIT/Apache-2.0 | MIT |\n"));
        assert!(region.contains("| `cfg-if` | 1.0.4 | MIT OR Apache-2.0 | MIT |\n"));
        // cxxbridge-cmd no está en el metadato: su licencia sale de LICENSE_OVERRIDES.
        assert!(region.contains("| `cxxbridge-cmd` | 1.0.199 | MIT OR Apache-2.0 | MIT |\n"));
        assert!(region.contains("| `foldhash` | 0.1.5 | Zlib | Zlib |\n"));
    }

    #[test]
    fn test_render_inventory_count_and_summary() {
        let region = sample_region();
        assert!(region.starts_with(
            "Generado desde `Cargo.lock` (4 paquetes resueltos, 3 crates únicos, directos y transitivos).\n\
             Resumen por familia (paquetes resueltos): MIT 3, Zlib 1.\n"
        ));
    }

    #[test]
    fn test_render_inventory_fails_with_unknown_expression() {
        let mut licenses = sample_licenses();
        licenses.insert(
            ("foldhash".to_string(), "0.1.5".to_string()),
            Some("WTFPL".to_string()),
        );
        let err =
            render_licenses_inventory(&cargo_lock_entries(SAMPLE_LOCK), &licenses).unwrap_err();
        assert!(err.to_string().contains("foldhash"));
        assert!(err.to_string().contains("WTFPL"));
    }

    #[test]
    fn test_render_inventory_fails_without_license() {
        let mut licenses = sample_licenses();
        licenses.insert(("foldhash".to_string(), "0.1.5".to_string()), None);
        let err =
            render_licenses_inventory(&cargo_lock_entries(SAMPLE_LOCK), &licenses).unwrap_err();
        assert!(err.to_string().contains("foldhash"));
        // Ausente del metadato y sin override: también falla nombrando el paquete.
        let mut licenses = sample_licenses();
        licenses.remove(&("cfg-if".to_string(), "1.0.4".to_string()));
        let err =
            render_licenses_inventory(&cargo_lock_entries(SAMPLE_LOCK), &licenses).unwrap_err();
        assert!(err.to_string().contains("cfg-if 1.0.4"));
    }

    #[test]
    fn test_replace_inventory_region_preserves_outer_text() {
        let doc = format!(
            "antes\n{}\nviejo\n{}\ndespués\n",
            INVENTORY_START, INVENTORY_END
        );
        let out = replace_inventory_region(&doc, "nuevo\n").unwrap();
        assert_eq!(
            out,
            format!(
                "antes\n{}\nnuevo\n{}\ndespués\n",
                INVENTORY_START, INVENTORY_END
            )
        );
    }

    #[test]
    fn test_replace_inventory_region_fails_without_markers() {
        assert!(replace_inventory_region("sin marcadores\n", "x\n").is_err());
        let only_start = format!("{}\nx\n", INVENTORY_START);
        let err = replace_inventory_region(&only_start, "x\n").unwrap_err();
        assert!(err.to_string().contains(INVENTORY_END));
    }

    #[test]
    fn test_diff_licenses_inventory_synced_and_crlf() {
        let doc = sample_licenses_doc();
        assert!(diff_licenses_inventory(&doc, &sample_region()).is_ok());
        let with_crlf = doc.replace('\n', "\r\n");
        assert!(diff_licenses_inventory(&with_crlf, &sample_region()).is_ok());
    }

    #[test]
    fn test_diff_licenses_inventory_detects_version_and_license() {
        let doc = sample_licenses_doc();
        let old_version = doc.replace("| `foldhash` | 0.1.5 |", "| `foldhash` | 0.1.4 |");
        let err = diff_licenses_inventory(&old_version, &sample_region()).unwrap_err();
        assert!(err.contains("desincronizado"));
        assert!(err.contains("+| `foldhash` | 0.1.5 | Zlib | Zlib |"));
        let bad_license = doc.replace(
            "| `foldhash` | 0.1.5 | Zlib | Zlib |",
            "| `foldhash` | 0.1.5 | MIT OR Apache-2.0 | MIT |",
        );
        let err = diff_licenses_inventory(&bad_license, &sample_region()).unwrap_err();
        assert!(err.contains("-| `foldhash` | 0.1.5 | MIT OR Apache-2.0 | MIT |"));
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
