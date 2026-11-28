//! Módulo `cli.rs` — parseo clap, expansión de entradas, resolución de salidas y reporte.

use crate::errors::E;
use crate::ffmpeg_io::{resolve_audio_bitrate, verify_ffmpeg};
use crate::preset::{AudioBitrate, EgressParams, Preset, DEFAULT_PARAMS};
use avi_core::exit_codes::{CliError, ExitCode};
use avi_core::json_emitter::emit_raw_json;
use avi_term::progress::PhaseProgress;
use avi_term::report::{ContextValue, PhaseSpec, Pipeline};
use avi_term::style::{paint, Stream, Tone, ACTIVE};
use avi_term::{fmt, line, stage};
use serde::Serialize;
use serde_json::json;
use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Extensiones de video soportadas (case-insensitive).
pub const VIDEO_EXTENSIONS: &[&str] = &["mp4", "mov", "mkv", "webm", "avi"];

/// Extensiones de audio soportadas (case-insensitive).
pub const AUDIO_EXTENSIONS: &[&str] = &["wav", "mp3", "m4a", "aac", "flac", "ogg", "opus"];

#[derive(clap::Args, Debug, Clone)]
pub struct Cli {
    /// Rutas de entrada (archivos o directorios)
    #[arg(required_unless_present = "version", num_args = 1..)]
    pub input: Vec<PathBuf>,

    /// Nombre del archivo de salida (solo si lote == 1 archivo)
    #[arg(short = 'o', long = "output-name")]
    pub output_name: Option<String>,

    /// Directorio de salida
    #[arg(long = "output-dir")]
    pub output_dir: Option<PathBuf>,

    /// Prefijo para el nombre de salida
    #[arg(long = "prefix")]
    pub prefix: Option<String>,

    /// Sufijo para el nombre de salida (defecto: _denoised)
    #[arg(long = "suffix")]
    pub suffix: Option<String>,

    /// Escanear directorios recursivamente
    #[arg(long = "recursive")]
    pub recursive: bool,

    /// Sobrescribir archivos existentes
    #[arg(long = "overwrite", conflicts_with = "skip_existing")]
    pub overwrite: bool,

    /// Saltar archivos de salida que ya existan
    #[arg(long = "skip-existing", conflicts_with = "overwrite")]
    pub skip_existing: bool,

    /// Preset de calidad (ej: studio). Aplica una combinación predefinida de
    /// parámetros de empaquetado y post-procesamiento. Las flags individuales
    /// sobrescriben los valores del preset.
    #[arg(long = "preset", value_enum)]
    pub preset: Option<Preset>,

    /// Bitrate del audio de salida en kbps (64-320); si se omite, el preset decide por códec y los formatos sin pérdida lo ignoran
    #[arg(long = "audio-bitrate")]
    pub audio_bitrate: Option<u32>,

    /// Fracción de mezcla con el audio original (0.0 = 100% limpio, 1.0 = 100% original; defecto: 0.0)
    #[arg(long = "dry-blend")]
    pub dry_blend: Option<f32>,

    /// Nivel de atenuación en pausas sin voz en dB (-40 a 0, defecto: -25; 0 desactiva el gate)
    #[arg(long = "pause-gate-db")]
    pub pause_gate_db: Option<f64>,

    /// Activar filtro pasa-altos a 70 Hz para suprimir rumble subgrave
    #[arg(long = "highpass")]
    pub highpass: bool,

    /// Normalizar sonoridad a estándar de internet (-16 LUFS)
    #[arg(long = "normalize")]
    pub normalize: bool,

    /// Nivel de sonoridad objetivo en LUFS EBU R128 (-36 a -12, ej: -16 para web/podcast, -23 para broadcast)
    #[arg(long = "target-lufs")]
    pub target_lufs: Option<f64>,

    /// Preservar audio estéreo de 2 canales procesando cada canal independientemente
    #[arg(long = "stereo")]
    pub stereo: bool,

    /// Simular procesamiento sin modificar disco ni ejecutar IA
    #[arg(long = "dry-run")]
    pub dry_run: bool,

    /// Emitir un único envelope JSON por stdout; en AVI lo activa el `--json` global
    #[arg(skip)]
    pub json: bool,

    /// Salida detallada de depuración a stderr
    #[arg(long = "verbose")]
    pub verbose: bool,

    /// Imprimir versión detallada (denoise + modelo DPDFNet + ffmpeg)
    #[arg(short = 'V', long = "version", action = clap::ArgAction::SetTrue)]
    pub version: bool,
}

/// Representa un item planificado para procesar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedItem {
    pub input: PathBuf,
    pub output: PathBuf,
    pub is_duplicate_adjusted: bool,
}

/// Estado de un item tras simulación o ejecución.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ItemStatus {
    Ok,
    Failed,
    Skipped,
    DryRun,
}

/// Resultado de un archivo en el envelope JSON: la entrada, la salida, su
/// estado y un mensaje.
#[derive(Debug, Clone, Serialize)]
pub struct FileReport {
    pub input: String,
    pub output: String,
    pub status: ItemStatus,
    pub message: String,
}

/// Resumen final del lote: cuántos archivos salieron bien, fallaron o se
/// omitieron.
#[derive(Debug, Clone, Serialize, Default, PartialEq, Eq)]
pub struct BatchSummary {
    pub ok: usize,
    pub failed: usize,
    pub skipped: usize,
}

/// Resultado de ejecutar la CLI de archivos.
#[derive(Debug)]
pub enum BatchOutcome {
    /// El lote terminó sin fallos, o se mostró la versión. El lote ya emitió
    /// su envelope con `--json`.
    Done,
    /// La cancelación cooperativa detuvo el lote. La etapa del archivo en
    /// curso queda marcada como interrumpida en el registro, sin pintar: el
    /// llamador pinta la interrupción y emite el envelope de error.
    Cancelled,
    /// El lote terminó con fallos. `code` es el peor código de salida del
    /// lote y `error` el error que lo resume, con el `reason` del primer
    /// fallo de ese código. `files` y `summary` son el resultado por archivo y
    /// el resumen: el llamador los añade al envelope de error y escribe el
    /// error en texto.
    Failed {
        code: i32,
        error: CliError,
        files: Vec<FileReport>,
        summary: BatchSummary,
    },
}

/// Error de validación de argumentos: `usage_error` con código 2.
fn usage_error(message: impl Into<String>) -> CliError {
    CliError::new(ExitCode::InvalidInput, "usage_error", message)
}

/// Normaliza una ruta lexicalmente contra `cwd` sin tocar disco ni resolver symlinks.
pub fn normalize_lexical(path: &Path, cwd: &Path) -> PathBuf {
    let abs_path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };

    let mut components = Vec::new();
    for comp in abs_path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                if let Some(Component::Normal(_)) = components.last() {
                    components.pop();
                }
            }
            c => components.push(c),
        }
    }

    let mut result = PathBuf::new();
    for c in components {
        result.push(c.as_os_str());
    }
    result
}

/// Clave de deduplicación para rutas (case-insensitive en Windows, byte-exact en Unix).
pub fn dedup_key(path: &Path) -> String {
    let s = path.to_string_lossy();
    if cfg!(windows) {
        s.to_lowercase()
    } else {
        s.into_owned()
    }
}

fn has_extension(path: &Path, extensions: &[&str]) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| {
            let ext_lower = ext.to_lowercase();
            extensions.contains(&ext_lower.as_str())
        })
        .unwrap_or(false)
}

/// Comprueba si la extensión del archivo es de video soportado.
pub fn is_video(path: &Path) -> bool {
    has_extension(path, VIDEO_EXTENSIONS)
}

/// Comprueba si la extensión del archivo es de audio soportado.
pub fn is_audio(path: &Path) -> bool {
    has_extension(path, AUDIO_EXTENSIONS)
}

/// Comprueba si la extensión del archivo pertenece a un medio soportado (video o audio).
pub fn is_supported_input(path: &Path) -> bool {
    is_video(path) || is_audio(path)
}

/// Extensión de salida por defecto para un medio: `.mp4` para video y la
/// extensión original (normalizada a minúsculas) para audio.
fn default_output_extension(input: &Path) -> String {
    if is_video(input) {
        "mp4".to_string()
    } else {
        input
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.to_ascii_lowercase())
            .unwrap_or_else(|| "wav".to_string())
    }
}

/// Valida los caracteres permitidos en prefijos y sufijos: `[A-Za-z0-9._-]` y no `.` ni `..` exactos.
pub fn validate_affix(name: &str, affix_type: &str) -> Result<(), E> {
    if name.is_empty() {
        return Ok(());
    }
    if name == "." || name == ".." {
        return Err(E::EInvalidInput(format!(
            "El {} no puede ser «.» ni «..».",
            affix_type
        )));
    }
    for c in name.chars() {
        if !c.is_ascii_alphanumeric() && c != '.' && c != '_' && c != '-' {
            return Err(E::EInvalidInput(format!(
                "El {} «{}» contiene caracteres no permitidos; solo se admiten letras, números, «.», «_» y «-».",
                affix_type, name
            )));
        }
    }
    Ok(())
}

/// Expande las rutas de entrada determinísticamente.
pub fn expand_inputs(
    inputs: &[PathBuf],
    recursive: bool,
    output_dir: Option<&Path>,
    suffix: &str,
    verbose: bool,
    cwd: &Path,
) -> Result<Vec<PathBuf>, E> {
    let mut collected = Vec::new();
    let mut seen = HashSet::new();

    let norm_out_dir = output_dir.map(|od| normalize_lexical(od, cwd));
    let suffix_filter = suffix.to_lowercase();

    for input in inputs {
        let norm_input = normalize_lexical(input, cwd);

        if norm_input.is_dir() {
            let mut dir_files = Vec::new();
            collect_dir_entries(
                &norm_input,
                recursive,
                norm_out_dir.as_deref(),
                &suffix_filter,
                verbose,
                &mut dir_files,
            )?;

            // Orden determinista byte-wise UTF-8
            dir_files.sort_by(|a, b| a.to_string_lossy().cmp(&b.to_string_lossy()));

            for file in dir_files {
                let key = dedup_key(&file);
                if seen.insert(key) {
                    collected.push(file);
                }
            }
        } else if norm_input.is_file() {
            if !is_supported_input(&norm_input) {
                return Err(E::EInvalidInput(format!(
                    "El formato de {} no es compatible; se admiten video ({}) y audio ({}).",
                    input.display(),
                    VIDEO_EXTENSIONS.join(", "),
                    AUDIO_EXTENSIONS.join(", ")
                )));
            }
            let key = dedup_key(&norm_input);
            if seen.insert(key) {
                collected.push(norm_input);
            }
        } else {
            return Err(E::EInvalidInput(format!(
                "La ruta de entrada {} no existe.",
                input.display()
            )));
        }
    }

    if collected.is_empty() {
        return Err(E::EInvalidInput(
            "Las rutas indicadas no contienen archivos de audio ni de video compatibles."
                .to_string(),
        ));
    }

    // Orden final determinista byte-wise UTF-8
    collected.sort_by(|a, b| a.to_string_lossy().cmp(&b.to_string_lossy()));
    Ok(collected)
}

fn collect_dir_entries(
    dir: &Path,
    recursive: bool,
    norm_out_dir: Option<&Path>,
    suffix_filter: &str,
    verbose: bool,
    out: &mut Vec<PathBuf>,
) -> Result<(), E> {
    let read_dir = std::fs::read_dir(dir).map_err(E::EIo)?;

    for entry in read_dir {
        let entry = entry.map_err(E::EIo)?;
        let path = entry.path();

        if path.is_dir() {
            // Excluir output-dir si está anidado
            if let Some(out_d) = norm_out_dir {
                if path == out_d {
                    if verbose {
                        line::detail(&format!(
                            "Se excluye el directorio de salida anidado: {}",
                            path.display()
                        ));
                    }
                    continue;
                }
            }
            if recursive {
                collect_dir_entries(&path, recursive, norm_out_dir, suffix_filter, verbose, out)?;
            }
        } else if path.is_file() && is_supported_input(&path) {
            // Excluir en escaneos recursivos cualquier medio cuyo stem ya termine en el sufijo.
            if recursive && !suffix_filter.is_empty() {
                let stem = path
                    .file_stem()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_lowercase();
                if stem.ends_with(suffix_filter) {
                    if verbose {
                        line::detail(&format!(
                            "Se excluye el archivo ya procesado (*{}): {}",
                            suffix_filter,
                            path.display()
                        ));
                    }
                    continue;
                }
            }
            out.push(path);
        }
    }
    Ok(())
}

/// Resuelve los paths de salida para un lote expandido según precedencia.
#[allow(clippy::too_many_arguments)]
pub fn resolve_outputs(
    expanded: &[PathBuf],
    output_name: Option<&str>,
    output_dir: Option<&Path>,
    prefix: Option<&str>,
    suffix: Option<&str>,
    recursive: bool,
    cwd: &Path,
    verbose: bool,
) -> Result<Vec<ResolvedItem>, E> {
    if expanded.is_empty() {
        return Err(E::EInvalidInput(
            "No hay archivos que procesar.".to_string(),
        ));
    }

    let pfx = prefix.unwrap_or("");
    let sfx = suffix.unwrap_or("_denoised");

    validate_affix(pfx, "prefijo")?;
    validate_affix(sfx, "sufijo")?;

    // Regla 1: --output-name con lote == 1
    if let Some(name) = output_name {
        if expanded.len() > 1 {
            return Err(E::EInvalidInput(
                "--output-name solo se permite cuando el lote expandido tiene exactamente 1 archivo."
                    .to_string(),
            ));
        }

        let input = &expanded[0];
        let base_ext = default_output_extension(input);
        let final_name = match Path::new(name).extension().and_then(|ext| ext.to_str()) {
            Some(ext)
                if VIDEO_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str())
                    || AUDIO_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()) =>
            {
                name.to_string()
            }
            _ => format!("{}.{}", name, base_ext),
        };

        let target_dir = match output_dir {
            Some(od) => normalize_lexical(od, cwd),
            None => cwd.to_path_buf(),
        };
        let out_path = target_dir.join(&final_name);

        // si --output-name resuelve a la propia entrada sin prefix/suffix -> E_INVALID_INPUT exit 2 siempre
        if normalize_lexical(&out_path, cwd) == normalize_lexical(input, cwd) {
            return Err(E::EInvalidInput(
                "--output-name resuelve al mismo archivo de entrada.".to_string(),
            ));
        }

        return Ok(vec![ResolvedItem {
            input: input.clone(),
            output: out_path,
            is_duplicate_adjusted: false,
        }]);
    }

    // Si ambos prefijo y sufijo son vacíos y la salida es in-place -> E_INVALID_INPUT
    if pfx.is_empty() && sfx.is_empty() && output_dir.is_none() {
        return Err(E::EInvalidInput(
            "Prefijo y sufijo no pueden estar vacíos simultáneamente si la salida es en la misma carpeta."
                .to_string(),
        ));
    }

    let mut resolved_list = Vec::new();
    let mut used_outputs: HashSet<String> = HashSet::new();

    for input in expanded {
        let stem = input
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("media");
        let ext = default_output_extension(input);
        let filename = format!("{}{}{}.{}", pfx, stem, sfx, ext);

        let out_dir = match output_dir {
            Some(od) => {
                let base_out = normalize_lexical(od, cwd);
                if recursive {
                    // recrear árbol relativo a cwd
                    let norm_in = normalize_lexical(input, cwd);
                    if let Ok(rel) = norm_in.parent().unwrap_or(&norm_in).strip_prefix(cwd) {
                        base_out.join(rel)
                    } else {
                        if verbose {
                            line::detail(&format!(
                                "Entrada fuera del directorio actual; se usa un nombre plano: {}",
                                input.display()
                            ));
                        }
                        base_out
                    }
                } else {
                    base_out
                }
            }
            None => input.parent().unwrap_or(cwd).to_path_buf(),
        };

        let mut candidate_out = out_dir.join(&filename);
        let mut is_adjusted = false;

        // resolver colisiones intra-lote con _1, _2...
        let mut key = dedup_key(&candidate_out);
        if used_outputs.contains(&key) {
            let mut counter = 1;
            loop {
                let adjusted_filename = format!("{}{}{}_{}.{}", pfx, stem, sfx, counter, ext);
                let alt_path = out_dir.join(&adjusted_filename);
                let alt_key = dedup_key(&alt_path);
                if !used_outputs.contains(&alt_key) {
                    candidate_out = alt_path;
                    key = alt_key;
                    is_adjusted = true;
                    line::warn(format!(
                        "La salida de {} coincide con otra del lote y se renombra a {}",
                        input.display(),
                        candidate_out.display()
                    ));
                    break;
                }
                counter += 1;
            }
        }

        used_outputs.insert(key);

        resolved_list.push(ResolvedItem {
            input: input.clone(),
            output: candidate_out,
            is_duplicate_adjusted: is_adjusted,
        });
    }

    Ok(resolved_list)
}

/// Ejecuta el modo `--dry-run` puro sin I/O ni ffmpeg. Sin `--json` escribe el
/// plan por stdout, una línea por archivo; con `--json` emite un único
/// envelope `planned` con `files`, `summary` y `context`, sin etapas. La
/// cabecera y el cierre `Simulación completada` salen por stderr en los dos
/// modos.
pub fn run_dry_run(
    items: &[ResolvedItem],
    overwrite: bool,
    skip_existing: bool,
    json_mode: bool,
) -> Result<(), E> {
    line::context_with(
        &format!(
            "Limpieza de ruido · {}",
            fmt::count(items.len() as u64, "archivo", "archivos")
        ),
        &[("files", ContextValue::Int(items.len() as u64))],
    );
    let mut summary = BatchSummary::default();
    let mut files = Vec::with_capacity(items.len());

    for item in items {
        let exists = item.output.exists();
        // El estado y la razón de máquina, y el texto para personas, de cada caso.
        let (status, reason, human) = if exists {
            if overwrite {
                summary.ok += 1;
                (ItemStatus::DryRun, "would overwrite", "se sobrescribiría")
            } else if skip_existing {
                summary.skipped += 1;
                (ItemStatus::DryRun, "would skip", "se omitiría")
            } else {
                summary.failed += 1;
                (
                    ItemStatus::DryRun,
                    "would fail: E_OUTPUT_EXISTS",
                    "fallaría: ya existe la salida",
                )
            }
        } else {
            summary.ok += 1;
            (ItemStatus::DryRun, "would process", "se procesaría")
        };

        if json_mode {
            files.push(FileReport {
                input: item.input.display().to_string(),
                output: item.output.display().to_string(),
                status,
                message: reason.to_string(),
            });
        } else {
            println!(
                "{} → {} ({})",
                item.input.display(),
                item.output.display(),
                human
            );
        }
    }

    line::ok("Simulación completada · no se ha borrado nada");
    if json_mode {
        emit_raw_json(json!({ "status": "planned", "files": files, "summary": summary }));
    }

    Ok(())
}

/// Imprime la versión: `denoise 1.0.0 + modelo DPDFNet + ffmpeg <ver>`.
pub fn print_version(ffmpeg_version: Option<&str>) {
    let pkg_ver = env!("CARGO_PKG_VERSION");
    let ff_str = ffmpeg_version.unwrap_or("ffmpeg missing");
    println!("denoise {} + modelo DPDFNet + {}", pkg_ver, ff_str);
}

/// Ejecuta la CLI de archivos. El llamador decide cómo resolver ffmpeg; el
/// subcomando de AVI solo usa el ejecutable integrado. `studio_default` activa
/// la receta `studio` cuando no se pasa `--preset` (contrato del subcomando
/// AVI); `cancel_flag` conecta la cancelación con el handler del llamador, que
/// la CLI raíz ya instaló y la biblioteca no debe duplicar.
///
/// Un `Err` es un error de validación previo al lote: no se escribió ninguna
/// línea y el llamador lo presenta. Los fallos dentro del lote salen como
/// [`BatchOutcome::Failed`], porque el lote ya escribió sus líneas.
pub fn execute<F>(
    cli: Cli,
    json_mode: bool,
    studio_default: bool,
    resolve_ffmpeg: F,
    cancel_flag: Option<Arc<AtomicBool>>,
) -> Result<BatchOutcome, CliError>
where
    F: Fn() -> Result<PathBuf, E>,
{
    // Imprimir la versión si se pasa --version o -V.
    if cli.version {
        let ff_ver = resolve_ffmpeg()
            .ok()
            .and_then(|bin| verify_ffmpeg(&bin).ok());
        print_version(ff_ver.as_deref());
        return Ok(BatchOutcome::Done);
    }

    // Resolver parámetros de empaquetado/post-procesamiento con precedencia:
    // defaults → preset → flags individuales.
    let egress = resolve_egress_params_with(&cli, studio_default);

    // Validar bitrate explícito estructuralmente (64-320)
    if let AudioBitrate::Explicit(bitrate) = egress.audio_bitrate {
        if !(64..=320).contains(&bitrate) {
            return Err(usage_error(format!(
                "El bitrate de audio ({bitrate} kbps) está fuera del rango permitido, de 64 a 320 kbps."
            )));
        }
    }

    // Validar dry-blend (0.0 a 1.0)
    if !(0.0..=1.0).contains(&egress.dry_blend) {
        return Err(usage_error(format!(
            "El valor de --dry-blend ({}) está fuera del rango permitido, de 0 a 1.",
            egress.dry_blend
        )));
    }

    // Validar pause-gate-db (-40.0 a 0.0)
    if !(-40.0..=0.0).contains(&egress.pause_gate_db) {
        return Err(usage_error(format!(
            "El valor de --pause-gate-db ({} dB) está fuera del rango permitido, de -40 a 0 dB.",
            egress.pause_gate_db
        )));
    }

    // Validar target-lufs (-36.0 a -12.0) si está presente.
    if let Some(lufs) = egress.target_lufs {
        if !(-36.0..=-12.0).contains(&lufs) {
            return Err(usage_error(format!(
                "El valor de --target-lufs ({lufs:.1} LUFS) está fuera del rango permitido, de -36 a -12 LUFS."
            )));
        }
    }

    // Un prefijo o sufijo inválido es un error de uso, no de audio.
    for (value, kind) in [
        (cli.prefix.as_deref(), "prefijo"),
        (cli.suffix.as_deref(), "sufijo"),
    ] {
        if let Some(value) = value {
            validate_affix(value, kind).map_err(|e| usage_error(e.to_string()))?;
        }
    }

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    // Expansión
    let expanded = expand_inputs(
        &cli.input,
        cli.recursive,
        cli.output_dir.as_deref(),
        cli.suffix.as_deref().unwrap_or("_denoised"),
        cli.verbose,
        &cwd,
    )
    .map_err(|e| e.to_cli_error())?;

    // Resolución de nombres: sus errores son combinaciones de opciones que no
    // se pueden cumplir, es decir, errores de uso.
    let resolved = resolve_outputs(
        &expanded,
        cli.output_name.as_deref(),
        cli.output_dir.as_deref(),
        cli.prefix.as_deref(),
        cli.suffix.as_deref(),
        cli.recursive,
        &cwd,
        cli.verbose,
    )
    .map_err(|e| usage_error(e.to_string()))?;

    // El bitrate se valida contra el códec de cada salida antes del dry-run y
    // de ejecutar ffmpeg; un override incompatible no se ajusta en silencio.
    for item in &resolved {
        let ext = item
            .output
            .extension()
            .map(|f| f.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        resolve_audio_bitrate(&ext, egress.audio_bitrate)
            .map_err(|e| usage_error(e.to_string()))?;
    }

    // Dry run
    if cli.dry_run {
        run_dry_run(&resolved, cli.overwrite, cli.skip_existing, json_mode)
            .map_err(|e| e.to_cli_error())?;
        return Ok(BatchOutcome::Done);
    }

    // Localizar y verificar ffmpeg
    let ffmpeg_bin = resolve_ffmpeg().map_err(|e| e.to_cli_error())?;
    verify_ffmpeg(&ffmpeg_bin).map_err(|e| e.to_cli_error())?;

    let models_provider = crate::models::AviModelsProvider;

    let params = BatchParams {
        ffmpeg: &ffmpeg_bin,
        models_provider: &models_provider,
        audio_bitrate: egress.audio_bitrate,
        dry_blend: egress.dry_blend,
        pause_gate_db: egress.pause_gate_db,
        highpass: egress.highpass,
        stereo: egress.stereo,
        target_lufs: egress.target_lufs,
        overwrite: cli.overwrite,
        skip_existing: cli.skip_existing,
        json_mode,
        verbose: cli.verbose,
        cancel_flag,
    };

    Ok(process_batch(&resolved, &params))
}

/// Resuelve los parámetros con los valores por defecto (sin preset).
pub fn resolve_egress_params(cli: &Cli) -> EgressParams {
    resolve_egress_params_with(cli, false)
}

/// Resuelve los parámetros de empaquetado y post-procesamiento (egress)
/// con la precedencia: **defaults → preset → flags individuales**.
///
/// Con `studio_default`, la ausencia de `--preset` equivale a `studio`, que es
/// el contrato del subcomando de archivos de AVI. `--preset default` sigue
/// recuperando los valores originales.
pub fn resolve_egress_params_with(cli: &Cli, studio_default: bool) -> EgressParams {
    // 1. Valores por defecto.
    let mut p = EgressParams {
        audio_bitrate: DEFAULT_PARAMS.audio_bitrate,
        highpass: DEFAULT_PARAMS.highpass,
        target_lufs: DEFAULT_PARAMS.target_lufs,
        pause_gate_db: DEFAULT_PARAMS.pause_gate_db,
        dry_blend: DEFAULT_PARAMS.dry_blend,
        stereo: DEFAULT_PARAMS.stereo,
    };

    // 2. Valores del preset (explícito o `studio` por omisión en AVI).
    let preset = cli.preset.or(studio_default.then_some(Preset::Studio));
    if let Some(preset) = preset {
        let pp = preset.egress_params();
        p.audio_bitrate = pp.audio_bitrate;
        p.highpass = pp.highpass;
        p.target_lufs = pp.target_lufs;
        p.pause_gate_db = pp.pause_gate_db;
        p.dry_blend = pp.dry_blend;
        p.stereo = pp.stereo;
    }

    // 3. Flags individuales (si el usuario las pasa explícitamente).
    // Los booleans se combinan con OR ya que no hay forma limpia de anular
    // un booleano del preset en v1 (limitación documentada).
    if let Some(v) = cli.audio_bitrate {
        p.audio_bitrate = AudioBitrate::Explicit(v);
    }
    if let Some(v) = cli.dry_blend {
        p.dry_blend = v;
    }
    if let Some(v) = cli.pause_gate_db {
        p.pause_gate_db = v;
    }
    if cli.normalize {
        p.target_lufs = Some(-16.0);
    }
    if let Some(v) = cli.target_lufs {
        p.target_lufs = Some(v);
    }
    if cli.highpass {
        p.highpass = true;
    }
    if cli.stereo {
        p.stereo = true;
    }

    p
}

/// Parámetros de ejecución para el procesamiento de lotes.
pub struct BatchParams<'a> {
    pub ffmpeg: &'a Path,
    pub models_provider: &'a dyn crate::models::ModelsProvider,
    pub audio_bitrate: AudioBitrate,
    pub dry_blend: f32,
    pub pause_gate_db: f64,
    pub highpass: bool,
    pub stereo: bool,
    pub target_lufs: Option<f64>,
    pub overwrite: bool,
    pub skip_existing: bool,
    pub json_mode: bool,
    pub verbose: bool,
    pub cancel_flag: Option<Arc<AtomicBool>>,
}

/// Nombre de un archivo sin su carpeta, para las líneas de texto.
fn file_label(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Procesa un lote de elementos resueltos en orden. El código de salida más
/// grave del lote (en `BatchOutcome::Failed`) es 1 (fallo de ffmpeg, modelo o
/// E/S) sobre 2 (entrada, destino o ausencia de audio); la cancelación (3)
/// prevalece sobre ambos.
///
/// Abre un único indicador para todo el lote, con y sin `--json`: una etapa
/// por archivo que deja `[i/n] ✓ nombre  duración · tamaño`, y la ruta de cada
/// archivo generado sale como resultado `Archivo limpio: <ruta>`. Con un solo
/// archivo la etapa no deja línea propia y queda el cierre.
///
/// Con `--json` stdout lleva un único envelope (`completed`, o el de error si
/// el lote falló) con el resultado de cada archivo y el resumen; stderr es el
/// mismo.
pub fn process_batch(items: &[ResolvedItem], params: &BatchParams) -> BatchOutcome {
    let mut summary = BatchSummary::default();
    let mut files: Vec<FileReport> = Vec::with_capacity(items.len());
    let mut worst_exit_code = 0;
    // Primer fallo de cada código: el resumen del lote usa el del peor.
    let mut first_failure_1: Option<CliError> = None;
    let mut first_failure_2: Option<CliError> = None;
    let total_items = items.len();

    // Una etapa por archivo, en una sola fase sin título.
    avi_term::report::declare(Pipeline::new(vec![PhaseSpec {
        id: "files",
        title: "Limpieza",
        short: "Limpieza",
        stages: vec![&stage::DENOISING; total_items],
    }]));
    line::context_with(
        &format!(
            "Limpieza de ruido · {}",
            fmt::count(total_items as u64, "archivo", "archivos")
        ),
        &[("files", ContextValue::Int(total_items as u64))],
    );
    let progress = if total_items == 1 {
        PhaseProgress::new().single_stage()
    } else {
        PhaseProgress::new()
    };
    // Los nombres de la columna se rellenan al del archivo más largo, porque
    // el ancho del catálogo no los conoce.
    let name_width = items
        .iter()
        .map(|item| file_label(&item.input).chars().count())
        .max()
        .unwrap_or(0);

    for item in items {
        if let Some(flag) = &params.cancel_flag {
            if flag.load(Ordering::SeqCst) {
                return BatchOutcome::Cancelled;
            }
        }

        let in_str = item.input.display().to_string();
        let out_str = item.output.display().to_string();
        let name = file_label(&item.input);
        let column = format!("{name:<name_width$}");

        // Un archivo que se va a omitir no abre etapa: no hay trabajo que medir.
        let expects_skip = params.skip_existing && item.output.exists();
        if !expects_skip {
            progress.phase_with(&stage::DENOISING, &name);
        }

        let start_time = std::time::Instant::now();
        let cancel_flag_clone = params.cancel_flag.clone();

        let pipe_params = crate::pipeline::PipelineParams {
            ffmpeg: params.ffmpeg,
            input: &item.input,
            output: &item.output,
            audio_bitrate: params.audio_bitrate,
            dry_blend: params.dry_blend,
            pause_gate_db: params.pause_gate_db,
            highpass: params.highpass,
            stereo: params.stereo,
            target_lufs: params.target_lufs,
            models_provider: params.models_provider,
            overwrite: params.overwrite,
            skip_existing: params.skip_existing,
            verbose: params.verbose,
            cancel_flag: cancel_flag_clone,
        };

        let run = || {
            crate::pipeline::clean_file(&pipe_params, |pct, msg| {
                // `skipped` lo emite el pipeline antes de devolver
                // `Skipped`; ese caso no abre etapa y no se pinta.
                if !expects_skip {
                    progress.live(live_line(&name, pct, msg, start_time.elapsed()));
                }
            })
        };
        // El repintado periódico mantiene el tiempo y el latido sin terminal
        // aunque el pipeline tarde entre dos avisos de avance.
        let result = progress.while_running(run);

        let elapsed = start_time.elapsed();

        match result {
            Ok(crate::pipeline::ProcessStatus::Ok) => {
                summary.ok += 1;
                let size_bytes = std::fs::metadata(&item.output)
                    .map(|m| m.len())
                    .unwrap_or(0);
                let size_mb = size_bytes as f64 / (1024.0 * 1024.0);
                files.push(FileReport {
                    input: in_str,
                    output: out_str.clone(),
                    status: ItemStatus::Ok,
                    message: format!("{:.2} MB, {:.1}s", size_mb, elapsed.as_secs_f64()),
                });
                progress.done_as(&column);
                progress.note(&fmt::bytes(size_bytes));
                progress.result(Some("Archivo limpio"), &out_str);
            }
            Ok(crate::pipeline::ProcessStatus::Skipped) => {
                summary.skipped += 1;
                files.push(FileReport {
                    input: in_str,
                    output: out_str,
                    status: ItemStatus::Skipped,
                    message: "skipped".to_string(),
                });
                // Un archivo omitido no deja etapa viva, pero sí su línea `○`
                // con el motivo y su lugar en la numeración.
                progress.discard_stage();
                progress.skip_as(&stage::DENOISING, &name, "ya existe la salida");
            }
            Err(e) => {
                summary.failed += 1;

                let code = e.exit_code();
                if code == 3 {
                    // La cancelación deja la etapa como interrumpida; quien
                    // llama pinta `✗ <archivo> · interrumpido` y el cierre.
                    progress.done_as(&name);
                    progress.interrupt();
                    return BatchOutcome::Cancelled;
                }
                if code == 1 || (code == 2 && worst_exit_code != 1) {
                    worst_exit_code = code;
                }
                let slot = if code == 1 {
                    &mut first_failure_1
                } else {
                    &mut first_failure_2
                };
                if slot.is_none() {
                    *slot = Some(e.to_cli_error());
                }

                files.push(FileReport {
                    input: in_str,
                    output: out_str,
                    status: ItemStatus::Failed,
                    message: e.to_string(),
                });
                // La etapa queda como fallida con el nombre del archivo y su
                // causa, como aviso bajo ella.
                progress.done_as(&column);
                progress.fail_current();
                line::warn(fmt::sentence(&e.to_string()));
            }
        }
    }

    if worst_exit_code != 0 {
        let first = if worst_exit_code == 1 {
            first_failure_1
        } else {
            first_failure_2
        };
        let mut error = CliError::new(
            if worst_exit_code == 1 {
                ExitCode::Error
            } else {
                ExitCode::InvalidInput
            },
            first
                .as_ref()
                .map_or("denoise_failed", |first| first.reason.as_str())
                .to_string(),
            format!(
                "No se pudo limpiar {} de {}.",
                summary.failed,
                fmt::count(total_items as u64, "archivo", "archivos")
            ),
        );
        error.detail = first.and_then(|first| first.detail);
        // El indicador termina sin cierre: el error va detrás del separador.
        progress.close();
        return BatchOutcome::Failed {
            code: worst_exit_code,
            error,
            files,
            summary,
        };
    }

    let mut extra = fmt::count(summary.ok as u64, "archivo limpio", "archivos limpios");
    if summary.skipped > 0 {
        extra.push_str(&format!(
            ", {}",
            fmt::count(summary.skipped as u64, "omitido", "omitidos")
        ));
    }
    progress.finish("Limpieza de ruido completada", &[extra]);
    if params.json_mode {
        emit_raw_json(json!({ "status": "completed", "files": files, "summary": summary }));
    }

    BatchOutcome::Done
}

/// Línea activa de un archivo: `● nombre  42 %  mensaje  duración`. El prefijo
/// `[i/n]` lo añade el indicador.
fn live_line(label: &str, pct: u32, message: &str, elapsed: std::time::Duration) -> String {
    format!(
        "{ACTIVE} {label}  {pct:>3} %  {message}  {}",
        paint(Stream::Stderr, Tone::Dim, &fmt::duration(elapsed))
    )
}
