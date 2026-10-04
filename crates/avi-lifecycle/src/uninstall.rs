//! `self uninstall`: los nueve pasos, en orden, sobre la instalación
//! registrada y no sobre la posición del ejecutable.
//!
//! Una desinstalación anterior, implementada en el binario, tenía tres
//! defectos que R1, R2 y la operación de desinstalación condemnan:
//!
//! - En Unix borraba el **directorio padre del ejecutable** cuando este no era el
//!   canónico. Lanzado desde el árbol de desarrollo eso alcanzaba `target/debug`, que
//!   es un directorio del repositorio: R2 lo prohíbe y el bug era alcanzable desde el
//!   propio árbol.
//! - La integración de `PATH` se revertía con una función propia que **aplanaba
//!   `%VAR%`** al escribir, de modo que una entrada que el usuario había escrito como
//!   `%LOCALAPPDATA%\...` salía como la ruta expandida. La integración del `PATH` en
//!   Windows conserva el tipo y la forma, y la reversión exacta se deriva del recibo.
//! - La parada del daemon, la lectura del pidfile y el borrado diferido de Windows no
//!   compartían una regla común: cuatro sitios para una regla.
//!
//! **Sobre qué actúa.** Sobre la instalación registrada, sea cual sea la
//! copia del binario que invoque el comando. Las raíces efectivas salen del recibo
//! ([`Roots::from_receipt`](crate::cleanup::Roots::from_receipt)), de modo que
//! desinstalar funciona aunque `AVI_DATA_DIR` o `AVI_CACHE_DIR` ya no estén
//! definidas.
//!
//! **R2** es lo que gobierna el paso 8 y por eso tiene su propia función pública
//! ([`program_dir_is_removable`]): el directorio de programa solo se borra si contiene
//! el recibo o el ejecutable, y nunca si es la raíz de una unidad, `$HOME`, un
//! ancestro de `$HOME` o una de las otras raíces del producto. Ni una variable de
//! reubicación ni un recibo manipulado pueden ampliar el alcance.
//!
//! **Idempotencia**: sin instalación ni estado, éxito con `not_installed`.
//! **Residuo**: cero dentro de las raíces de propiedad exclusiva, y
//! lo compartido que no se borra se informa.

use crate::channel::Channel;
use crate::cleanup::{self, Category, DeletionPlan, Roots};
use crate::confirm::{self, Confirmation, Decision, Kind, PlanEntry};
use crate::daemon_stop::{self, ProcessControl};
use crate::path_unix;
use crate::receipt::{self, InstallReceipt, PathIntegration};
use crate::LifecycleError;
use std::path::{Path, PathBuf};

/// Comando de Homebrew que sustituye a la desinstalación de la copia del Cask
/// (paso 1).
pub const HOMEBREW_UNINSTALL: &str = "brew uninstall --cask --zap ai-voice-interconnector";

/// Opciones de `self uninstall`. El parseo se queda en el binario, que es quien ve
/// los argumentos.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Options {
    /// `--keep-data`: conserva modelos, voces y habla sintetizada.
    pub keep_data: bool,
    /// `--dry-run`: imprime el plan y no modifica el disco.
    pub dry_run: bool,
    /// `--yes`: omite la confirmación destructiva.
    pub assume_yes: bool,
}

/// Borrado del directorio de programa cuando el ejecutable en uso está dentro.
///
/// El **mecanismo** es de plataforma y vive en el binario —en Unix es
/// `remove_dir_all`, y en Windows el limpiador propio desacoplado—, porque el motor no
/// lo puede implementar sin arrastrar `avi-daemon`. Lo que sí es del motor es la
/// **decisión**: cuándo se borra ya y cuándo se programa, y que el resultado difiera
/// (`uninstalled` contra `removal_scheduled`). Por eso entra por un rasgo, igual
/// que [`crate::daemon_stop::ProcessControl`].
pub trait ProgramDirRemover {
    /// `true` si el ejecutable en uso está dentro de `program_dir`, que es el caso en
    /// que el borrado directo es imposible en Windows.
    fn exe_lives_inside(&self, program_dir: &Path) -> bool;
    /// Borra el directorio ya. En Unix es lo que hace siempre.
    fn remove_now(&self, program_dir: &Path) -> anyhow::Result<()>;
    /// Programa el borrado para cuando termine el proceso en curso. `Ok` significa que
    /// el limpiador está en marcha; si no puede garantizarse, `Err`. Es el caso diferido
    /// del paso 8.
    fn schedule(&self, program_dir: &Path, pid: u32) -> anyhow::Result<()>;
}

/// Plan del paso 2.
///
/// El estado **no** se recalcula: sale del planificador de `cleanup`, con **una**
/// diferencia deliberada —sin `--keep-data` el destino es la raíz de datos entera y no
/// la lista de categorías, porque al desinstalar desaparece el programa y con él las
/// voces de fábrica—. Dos alcances implementados por separado divergirían en cuanto
/// uno cambiara, así que la desviación se aplica una sola vez, en [`compose_plan`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    /// Estado de usuario: con `--keep-data`, el plan de `cleanup --all` filtrado; sin
    /// él, la raíz de datos entera.
    pub state: DeletionPlan,
    /// Directorio de programa con su tamaño, que es un destino propio de la
    /// desinstalación.
    pub program_dir: Option<PlanEntry>,
    /// Lo que se conserva a propósito, con el motivo: recursos compartidos (R3) y la
    /// integración de `PATH`, que se revierte en vez de borrarse.
    pub preserved: Vec<Preserved>,
}

impl Plan {
    /// Entradas con tamaños que hay que listar antes de una operación destructiva.
    pub fn entries(&self) -> Vec<PlanEntry> {
        let mut out: Vec<PlanEntry> = self
            .state
            .targets
            .iter()
            .map(|t| PlanEntry {
                path: t.path.clone(),
                size: Some(t.size),
            })
            .collect();
        if let Some(program_dir) = &self.program_dir {
            out.push(program_dir.clone());
        }
        out
    }
}

/// Recurso compartido que la operación deja intacto a propósito, con su motivo.
pub type Preserved = cleanup::Preserved;

/// Estado de partida de la operación, tal como lo ve el binario.
pub struct Env<'a> {
    /// Raíces efectivas sobre las que se opera. Las resuelve el binario con
    /// [`Roots::from_receipt`], que es donde el recibo manda sobre las variables de
    /// entorno: van como dato para que la operación sea una función de sus entradas y
    /// para que las pruebas aisladas puedan apartarlas a temporales.
    pub roots: Roots,
    /// Recibo de la instalación registrada, si lo hay. `None` es el caso
    /// `not_installed`.
    pub receipt: Option<&'a InstallReceipt>,
    /// Canal de la copia que se está ejecutando, que decide si la operación puede
    /// actuar.
    pub channel: Channel,
    /// Directorio de programa **registrado**, que es sobre el que se opera y no sobre
    /// la posición del ejecutable.
    pub program_dir: PathBuf,
    /// Dirección del protocolo de parada, con el mismo override que el resto del
    /// producto.
    pub daemon_addr: String,
    /// `$HOME` del usuario.
    ///
    /// Va como dato y no se lee del entorno porque los bloques delimitados del `PATH`
    /// se escribieron con el `$HOME` **de aquel momento**: `remove_block` reconstruye el
    /// texto del bloque para compararlo, y con el `$HOME` equivocado no lo quitaría. El
    /// llamador lo pasa porque es quien sabe cuál es —en producción, el mismo que usan
    /// las rutas del producto—.
    pub home: PathBuf,
}

/// Cómo se borró el directorio de programa.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Removal {
    /// Borrado ya, de forma síncrona.
    Now,
    /// Programado para cuando termine el proceso en curso (paso 8, en Windows).
    Scheduled,
}

/// Desenlace de `self uninstall`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outcome {
    /// `uninstalled`, `removal_scheduled`, `not_installed` o `cancelled`.
    pub status: &'static str,
    /// Rutas borradas en esta ejecución, incluido el directorio de programa.
    pub removed: Vec<String>,
    /// Del barrido transversal.
    pub swept: Vec<String>,
    /// Lo que no se pudo borrar.
    pub kept: Vec<String>,
    /// Recursos compartidos que se conservan, con el motivo.
    pub preserved: Vec<Preserved>,
    /// `true` si se invirtió la integración de `PATH`.
    pub path_reverted: bool,
    /// `true` si el directorio de programa se borró de verdad; `false` si quedó
    /// programado para después de salir, o si R2 lo impidió.
    pub program_dir_removed: bool,
    /// Mensaje (ruta y causa) si el paso 8 no pudo borrar ni programar el directorio de
    /// programa. La exclusión por R2 no cuenta: es preservación deliberada.
    pub program_dir_kept: Option<String>,
    pub dry_run: bool,
    pub daemon_was_running: bool,
    pub failed: Vec<String>,
}

impl Outcome {
    /// `reason` de contrato y código de salida cuando el paso 8 no pudo borrar ni programar
    /// el directorio de programa, y `None` en cualquier otro desenlace.
    ///
    /// Es éxito parcial con código propio: el resto de la desinstalación se completó y lo
    /// único que falta es ese directorio. La exclusión por R2 no es un fallo sino
    /// preservación deliberada, así que no llega aquí. Quien cablea emite el sobre con este
    /// `reason` y sale por veredicto con su código, como hace `self install`.
    pub fn lifecycle_error(&self) -> Option<LifecycleError> {
        self.program_dir_kept.as_ref().map(|detail| {
            LifecycleError::program_dir_kept(format!(
                "el resto de la desinstalación se completó, pero no se pudo borrar ni programar \n                 el borrado del directorio de programa ({detail}); bórralo a mano cuando ningún \n                 proceso lo use"
            ))
        })
    }
}

/// Ejecuta `self uninstall`.
///
/// El orden es el de los nueve pasos, y dos de ellos tienen una consecuencia que
/// conviene dejar escrita: la recuperación y el barrido (paso 1) ocurren **antes** de
/// que se componga el plan, para que el plan que el usuario ve y confirma sea el que
/// queda; y la parada del daemon (paso 5) ocurre **después** de la confirmación, de
/// modo que un `daemon_stop_failed` no deja nada borrado, que es lo que la operación
/// exige.
pub async fn run(
    env: &Env<'_>,
    options: &Options,
    remover: &dyn ProgramDirRemover,
    control: &dyn ProcessControl,
) -> anyhow::Result<Outcome> {
    // ── Paso 1. El canal decide si se puede actuar; luego, recuperación y bloqueo ─
    if env.channel == Channel::Homebrew {
        return Err(LifecycleError::new(
            "externally_managed",
            format!(
                "esta copia la gestiona Homebrew: ejecuta `{HOMEBREW_UNINSTALL}`, y \
                 `cleanup --all` para el estado de usuario"
            ),
        )
        .into());
    }

    let receipt = env.receipt;
    let roots = &env.roots;
    let program_dir = env.program_dir.clone();

    // Sin instalación ni estado: la operación es éxito con `not_installed`.
    if receipt.is_none() && !state_exists(roots) && !program_dir.is_dir() {
        return Ok(Outcome {
            status: "not_installed",
            ..Outcome::default()
        });
    }

    // ── Paso 3. `--dry-run`: plan impreso, disco intacto ──────────────────────────
    // Antes del bloqueo a propósito: tomar el bloqueo crea el archivo, y una
    // simulación que deja un archivo detrás no es una simulación (criterio 20).
    if options.dry_run {
        return Ok(simulate(roots, receipt, &program_dir, options));
    }

    // Ninguna operación pide elevación.
    let report = crate::privileges::ensure_per_user()?;
    if let Some(warning) = &report.warning {
        eprintln!("{warning}");
    }
    let lock_path = roots.lock_path();
    let lock = crate::lock::acquire_at(&lock_path)?;
    let recovery = crate::recovery::recover(roots.recovery_roots())?;

    // ── Paso 2. Plan ──────────────────────────────────────────────────────────────
    let plan = compose_plan(roots, receipt, &program_dir, options);
    let entries = plan.entries();

    // ── Paso 4. Confirmación destructiva ─────────────────────────────────────────
    let summary = compose_summary(&plan, options);
    let decision = confirm(&summary, &entries, options)?;
    if decision == Decision::Cancelled {
        return Ok(Outcome {
            status: "cancelled",
            preserved: plan.preserved,
            ..Outcome::default()
        });
    }

    // ── Paso 5. Parar el daemon; si falla, `daemon_stop_failed` sin borrar nada ──
    let daemon = daemon_stop::stop(&roots.data_dir, &env.daemon_addr, control).await;
    daemon_stop::require_stopped(&daemon)?;

    let mut removed = Vec::new();
    let mut failed = Vec::new();

    // ── Paso 6. Borrar el estado: la raíz de datos entera salvo `--keep-data` ─────
    for target in &plan.state.targets {
        match remove_path(&target.path) {
            Ok(()) => removed.push(target.path.display().to_string()),
            Err(e) => {
                eprintln!("  no se pudo borrar {}: {e}", target.path.display());
                failed.push(target.path.display().to_string());
            }
        }
    }

    // ── Paso 7. Revertir el `PATH` exactamente según el recibo ───────────────────
    let path_reverted = revert_path(receipt, &env.home);

    // ── Paso 8. Borrar el directorio de programa aplicando R2 ────────────────────
    let mut program_dir_kept = None;
    let (program_dir_removed, status) = if program_dir_is_removable(roots, &program_dir) {
        match remove_program_dir(remover, &program_dir) {
            Ok(Removal::Now) => {
                removed.push(program_dir.display().to_string());
                (true, "uninstalled")
            }
            Ok(Removal::Scheduled) => (false, "removal_scheduled"),
            Err(e) => {
                eprintln!("  no se pudo borrar {}: {e}", program_dir.display());
                failed.push(program_dir.display().to_string());
                program_dir_kept = Some(format!("{}: {e}", program_dir.display()));
                (false, "uninstalled")
            }
        }
    } else {
        eprintln!(
            "  no se borra {}: R2 no permite borrar un directorio de programa sin \
             recibo ni ejecutable, ni uno que sea raíz del sistema o del perfil",
            program_dir.display()
        );
        (false, "uninstalled")
    };

    // ── Paso 9. Borrar el archivo de bloqueo ──────────────────────────────────────
    // Vive en el hermano del directorio de programa, así que se borra también cuando
    // ese quedó programado: el bloqueo es del ciclo de vida, no del directorio.
    let _ = std::fs::remove_file(&lock_path);
    drop(lock);

    Ok(Outcome {
        status,
        removed,
        swept: recovery
            .removed_parked
            .iter()
            .chain(recovery.removed_stagings.iter())
            .chain(recovery.removed_temporaries.iter())
            .map(|p| p.display().to_string())
            .collect(),
        kept: recovery
            .kept
            .iter()
            .map(|p| p.display().to_string())
            .collect(),
        preserved: plan.preserved,
        path_reverted,
        program_dir_removed,
        program_dir_kept,
        dry_run: false,
        daemon_was_running: daemon.was_running,
        failed,
    })
}

/// Compone el plan del paso 2.
///
/// El estado se compone con el planificador de `cleanup` y no con un segundo alcance en
/// paralelo: dos implementaciones de un mismo alcance divergirían en cuanto una cambiara.
/// Hay **una** diferencia deliberada, y es la protección de las voces de fábrica:
///
/// `cleanup --all` no borra `voices/default` ni `voices/ryan` porque van embebidas en
/// el binario y el programa sigue instalado: si se borran, `setup` las vuelve a
/// materializar. En una desinstalación el programa **desaparece**, así que dejarlas
/// sería residuo dentro de una raíz de propiedad exclusiva, que es exactamente lo que
/// el criterio 17 prohíbe. Por eso, sin `--keep-data`, el destino del estado es la raíz
/// de datos **entera** —que R1 permite porque es exclusiva— en vez de la lista de
/// categorías. Con `--keep-data` sí se aplica el plan de `cleanup --all` filtrado, que es
/// lo coherente: el usuario ha pedido conservar el estado.
pub fn compose_plan(
    roots: &Roots,
    receipt: Option<&InstallReceipt>,
    program_dir: &Path,
    options: &Options,
) -> Plan {
    let models = cleanup::plan(
        roots,
        &cleanup::Options {
            model: true,
            ..Default::default()
        },
    );
    // Lo que la raíz de modelos **no** puede perder se conserva en los dos casos: con
    // `--keep-data` no es un destino, y sin él lo decide R3.
    let mut state = DeletionPlan {
        targets: Vec::new(),
        preserved: models.preserved,
    };
    if options.keep_data {
        let data = cleanup::plan(
            roots,
            &cleanup::Options {
                all: true,
                ..Default::default()
            },
        );
        // Criterio 18: `--keep-data` conserva **modelos, voces y habla**, así que las
        // tres categorías de datos quedan fuera. Del plan de `cleanup --all` solo se
        // queda lo que es estado de ejecución: configuración, logs y pidfile.
        state.targets = data
            .targets
            .into_iter()
            .filter(|t| {
                !matches!(
                    t.category,
                    Category::Model | Category::Voices | Category::SyntheticSpeech
                )
            })
            .collect();
        state.preserved.extend(data.preserved);
        state.preserved.push(Preserved {
            path: roots.models_dir.clone(),
            reason: "`--keep-data` conserva los modelos",
        });
        state.preserved.push(Preserved {
            path: roots.data_dir.join("voices"),
            reason: "`--keep-data` conserva las voces",
        });
        state.preserved.push(Preserved {
            path: roots.data_dir.join("speech"),
            reason: "`--keep-data` conserva el habla sintetizada",
        });
    } else {
        state.targets = models.targets;
        if roots.data_dir.is_dir() {
            state.targets.push(cleanup::Target {
                path: roots.data_dir.clone(),
                category: Category::Config,
                size: cleanup::path_size(&roots.data_dir),
            });
        }
    }
    let mut preserved = std::mem::take(&mut state.preserved);
    if let Some(receipt) = receipt {
        describe_path_integration(receipt, &mut preserved);
    }
    state.targets.sort_by(|a, b| {
        a.category
            .cmp(&b.category)
            .then_with(|| a.path.cmp(&b.path))
    });
    Plan {
        state,
        program_dir: program_dir.is_dir().then(|| PlanEntry::of(program_dir)),
        preserved,
    }
}

/// Anuncia la integración de `PATH` como lo que se va a **revertir**, no a borrar.
fn describe_path_integration(receipt: &InstallReceipt, preserved: &mut Vec<Preserved>) {
    if !receipt.path_integration.modify_path {
        return;
    }
    let integration: &PathIntegration = &receipt.path_integration;
    if let Some(symlink) = &integration.symlink {
        preserved.push(Preserved {
            path: symlink.clone(),
            reason: "se retira el enlace, no se borra",
        });
    }
    for block in integration.profile_blocks.iter().flatten() {
        preserved.push(Preserved {
            path: block.clone(),
            reason: "se retira el bloque delimitado, no se borra el archivo",
        });
    }
    if let Some(entry) = &integration.registry_entry {
        preserved.push(Preserved {
            path: entry.clone(),
            reason: "se retira la entrada del registro, conservando el tipo del valor",
        });
    }
}

/// Paso 3: la simulación.
pub fn simulate(
    roots: &Roots,
    receipt: Option<&InstallReceipt>,
    program_dir: &Path,
    options: &Options,
) -> Outcome {
    let plan = compose_plan(roots, receipt, program_dir, options);
    let preview = crate::recovery::preview(roots.recovery_roots());
    eprintln!(
        "Simulación: se muestra lo que {} borraría, no se ha modificado nada.",
        crate::APP_NAME
    );
    for entry in plan.entries() {
        match entry.size {
            Some(size) => eprintln!("  {} ({})", entry.path.display(), crate::human_bytes(size)),
            None => eprintln!("  {}", entry.path.display()),
        }
    }
    for path in preview.all() {
        eprintln!("  barrido {}", path.display());
    }
    for item in &plan.preserved {
        eprintln!("  no se tocará {}: {}", item.path.display(), item.reason);
    }
    let mut removed: Vec<String> = plan
        .entries()
        .iter()
        .map(|e| e.path.display().to_string())
        .collect();
    removed.extend(preview.all().iter().map(|p| p.display().to_string()));
    Outcome {
        status: "uninstalled",
        removed,
        swept: Vec::new(),
        kept: preview
            .temporaries_kept
            .iter()
            .map(|p| p.display().to_string())
            .collect(),
        preserved: plan.preserved,
        path_reverted: false,
        program_dir_removed: false,
        program_dir_kept: None,
        dry_run: true,
        daemon_was_running: false,
        failed: Vec::new(),
    }
}

/// Resumen legible del plan, que es lo que el paso 2 muestra antes de preguntar.
fn compose_summary(plan: &Plan, options: &Options) -> Vec<String> {
    let mut out = vec![format!(
        "Se desinstalará {}{}.",
        crate::APP_NAME,
        if options.keep_data {
            ", conservando modelos, voces y habla"
        } else {
            ", incluido el estado de usuario"
        }
    )];
    let entries = plan.entries();
    out.push(format!(
        "{} ruta(s) a tocar y {} que se conservan:",
        entries.len(),
        plan.preserved.len()
    ));
    for target in &plan.state.targets {
        out.push(format!(
            "  {} {} ({})",
            target.category.as_str(),
            target.path.display(),
            crate::human_bytes(target.size)
        ));
    }
    for item in &plan.preserved {
        out.push(format!(
            "  no se tocará {}: {}",
            item.path.display(),
            item.reason
        ));
    }
    out
}

/// Aplica la confirmación destructiva.
fn confirm(
    summary: &[String],
    entries: &[PlanEntry],
    options: &Options,
) -> anyhow::Result<Decision> {
    let stdin = std::io::stdin();
    let mut entry = stdin.lock();
    confirm::confirm(
        &Confirmation {
            kind: Kind::Destructive,
            summary,
            entries,
            assume_yes: options.assume_yes,
            dry_run: false,
            stdin_is_terminal: std::io::IsTerminal::is_terminal(&stdin),
        },
        &mut entry,
        &mut std::io::stderr(),
    )
}

/// Paso 7: revertir el `PATH` exactamente según el recibo.
///
/// Unix: el enlace, **solo si apunta al directorio de programa** —un enlace que apunta
/// a otro sitio pertenece a otra instalación y no se toca—, y los bloques delimitados
/// de los perfiles. Windows: la entrada del registro con comparación canónica, que
/// `path_windows` hace conservando el tipo del valor y propagando
/// `WM_SETTINGCHANGE`.
///
/// `--no-modify-path` dejó `modify_path: false` en el recibo: no hay nada que
/// revertir, y por eso el desenlace es `false` y no un error.
pub fn revert_path(receipt: Option<&InstallReceipt>, home: &Path) -> bool {
    let Some(receipt) = receipt else {
        return false;
    };
    if !receipt.path_integration.modify_path {
        return false;
    }
    let integration: &PathIntegration = &receipt.path_integration;
    let mut touched = false;

    // El enlace simbólico es un mecanismo de Unix: en Windows el recibo lo registra
    // como `null` y la integración es la entrada del registro de abajo.
    #[cfg(unix)]
    let program_exe = receipt.install_dir.join(executable_name(receipt));
    #[cfg(unix)]
    if let Some(symlink) = &integration.symlink {
        if path_unix::revert_symlink(symlink, &program_exe) {
            touched = true;
        }
    }
    if let Some(blocks) = &integration.profile_blocks {
        let bin_dir = receipt_bin_dir(receipt);
        for block in blocks {
            if path_unix::remove_block(block, &bin_dir, home).unwrap_or(false) {
                touched = true;
            }
        }
    }
    #[cfg(windows)]
    if let Some(entry) = &integration.registry_entry {
        if let Ok(outcome) = crate::path_windows::revert(crate::path_windows::ENV_SUBKEY, entry) {
            touched |= outcome.changed;
        }
    }
    touched
}

/// Nombre del ejecutable registrado, deducido del recibo.
///
/// Solo lo usa la reversión del enlace, que es un mecanismo de Unix; en Windows el
/// ejecutable no hace falta porque la reversión es la entrada del registro.
#[cfg(unix)]
///
/// El recibo guarda las rutas de los archivos colocados y el ejecutable es el
/// único cuyo nombre lleva el de la aplicación; se busca entre ellos y se cae al
/// nombre de la plataforma si el recibo es antiguo y no lo lista. Deducirlo del recibo
/// y no de `std::env::current_exe` es lo que permite que la reversión apunte a la
/// instalación registrada y no a la copia que invoca el comando.
fn executable_name(receipt: &InstallReceipt) -> String {
    receipt
        .files
        .iter()
        .find_map(|f| {
            let name = f.rsplit('/').next().unwrap_or(f);
            name.starts_with(crate::APP_NAME).then(|| name.to_string())
        })
        .unwrap_or_else(executable_name_default)
}

/// Nombre del ejecutable en la plataforma actual.
pub fn executable_name_default() -> String {
    if cfg!(windows) {
        format!("{}.exe", crate::APP_NAME)
    } else {
        crate::APP_NAME.to_string()
    }
}

/// Directorio del enlace que registró el recibo, deducido de la ruta del enlace.
///
/// Los bloques de perfil se escribieron con el `bin_dir` del momento de instalar, y
/// `remove_block` necesita exactamente ese valor para quitar el bloque: con el
/// `bin_dir` de hoy, un bloque escrito por una instalación anterior no se quitaría. El
/// recibo no lo guarda como campo propio —registra el enlace, no su
/// directorio—, así que se deduce de la ruta del enlace, que sí está.
fn receipt_bin_dir(receipt: &InstallReceipt) -> PathBuf {
    receipt
        .path_integration
        .symlink
        .as_ref()
        .and_then(|s| s.parent())
        .map(Path::to_path_buf)
        .unwrap_or_else(crate::bin_dir)
}

/// R2: ¿se puede borrar este directorio de programa?
///
/// Exige las dos mitades de la regla. La positiva: el directorio contiene el recibo o
/// el ejecutable, que es lo que lo convierte *en* el directorio de programa y no en un
/// directorio cualquiera. La negativa: nunca es la raíz de una unidad, `$HOME`, un
/// ancestro de `$HOME` ni coincide con otra raíz del producto, de modo que ni una
/// variable de reubicación ni un recibo manipulado puedan ampliar el alcance.
/// Colgar del perfil no protege: el directorio de programa vive bajo `$HOME` en
/// todas las plataformas, así que esa pertenencia no puede rechazarlo; lo que
/// nunca puede ser es el propio `$HOME` o un ancestro suyo.
pub fn program_dir_is_removable(roots: &Roots, program_dir: &Path) -> bool {
    if program_dir.as_os_str().is_empty() {
        return false;
    }
    // Raíz de una unidad o del sistema: `/`, `C:\`, `\\servidor\recurso`.
    if program_dir.parent().is_none() {
        return false;
    }
    // Las comparaciones van por la **clave canónica**, que normaliza separadores a `\`
    // y minúsculas en las dos plataformas, así que el prefijo se busca con ese
    // separador y no con el de la ruta: comparar con `/` en Windows, o con `Path::
    // starts_with` en Unix —donde la clave es un único componente porque lleva `\`, no
    // `/`— desactivaría la regla justo donde más importa.
    let key = crate::canonical_path_key(program_dir);
    for protected in [
        roots.data_dir.as_path(),
        roots.models_dir.as_path(),
        roots.temp_root.as_path(),
    ] {
        if protected.as_os_str().is_empty() {
            continue;
        }
        if is_same_or_descendant(&key, &crate::canonical_path_key(protected)) {
            return false;
        }
    }
    // `$HOME` o un ancestro suyo (`/`, `/home`, `C:\Users\ana`): borrarlo se lleva
    // el perfil entero. La igualdad se rechaza aquí porque el bucle de arriba ya no
    // cubre `$HOME` a propósito: el programa cuelga del perfil en todas las
    // plataformas y esa pertenencia no lo protege.
    if !roots.home.as_os_str().is_empty() {
        let home = crate::canonical_path_key(&roots.home);
        if home == key || is_same_or_descendant(&home, &key) {
            return false;
        }
    }
    // La parte positiva: el recibo o el ejecutable.
    receipt::receipt_path(program_dir).is_file()
        || program_dir.join(executable_name_default()).is_file()
}

/// Paso 8: borra el directorio de programa, o lo programa si el ejecutable en uso
/// está dentro.
fn remove_program_dir(
    remover: &dyn ProgramDirRemover,
    program_dir: &Path,
) -> anyhow::Result<Removal> {
    if !program_dir.is_dir() {
        return Ok(Removal::Now);
    }
    if remover.exe_lives_inside(program_dir) {
        // Windows: el ejecutable en uso impide el borrado directo. Se pide el
        // limpiador propio desacoplado con reintentos acotados; si no puede
        // programarse, el error sube y el directorio queda intacto: borrar aquí
        // sería un borrado parcial con el ejecutable en uso.
        remover.schedule(program_dir, std::process::id())?;
        eprintln!(
            "  el directorio se borrará al terminar este proceso: {}",
            program_dir.display()
        );
        return Ok(Removal::Scheduled);
    }
    remover.remove_now(program_dir)?;
    Ok(Removal::Now)
}

/// `true` si hay estado de usuario que conservar o borrar.
fn state_exists(roots: &Roots) -> bool {
    roots.data_dir.is_dir() || roots.models_dir.is_dir()
}

/// `true` si `key` es `ancestor` o cuelga de él, sobre claves canónicas.
///
/// El separador del prefijo es `\` porque es el que `canonical_path_key` produce en las
/// dos plataformas. Una clave vacía —la de la raíz, que `trim_matches` reduce a nada— no
/// es ancestro de nadie: sin esta guarda, `""` sería prefijo de todas.
fn is_same_or_descendant(key: &str, ancestor: &str) -> bool {
    if ancestor.is_empty() {
        return false;
    }
    key == ancestor || key.starts_with(&format!("{ancestor}\\"))
}

/// Borra una ruta, sea archivo o directorio. `NotFound` **es** éxito: el plan dice lo que
/// tiene que dejar de existir, y si ya no existe el objetivo está cumplido. No es un
/// caso teórico —la parada del daemon borra el pidfile cuando no había daemon vivo, y
/// el pidfile es un destino de `--all`—.
fn remove_path(path: &Path) -> std::io::Result<()> {
    let outcome = if path.is_dir() && !path.is_symlink() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    };
    match outcome {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

#[cfg(test)]
mod r2_tests {
    //! Regresión: el directorio de programa cuelga de `$HOME` en todas las
    //! plataformas, así que esa pertenencia no puede rechazarlo; lo que R2
    //! impide es que sea el propio `$HOME`, un ancestro suyo u otra raíz.

    use super::*;
    use std::path::PathBuf;

    fn sandbox(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "uninstall-r2-{}-{}-{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("reloj del sistema")
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).expect("se crea el temporal");
        root
    }

    fn roots(home: &Path, program: &Path) -> Roots {
        Roots {
            program_dir: program.to_path_buf(),
            data_dir: home.join("datos"),
            models_dir: home.join("modelos"),
            temp_root: home.join("temporales"),
            home: home.to_path_buf(),
            models_shared: false,
        }
    }

    fn with_receipt(program: &Path) {
        std::fs::create_dir_all(program).expect("se crea el programa");
        std::fs::write(receipt::receipt_path(program), "{}").expect("se escribe el recibo");
    }

    #[test]
    fn program_under_home_with_receipt_is_removable() {
        let root = sandbox("bajo-home");
        let home = root.join("home").join("ana");
        let program = home.join("programas").join("ai-voice-interconnector");
        with_receipt(&program);
        assert!(
            program_dir_is_removable(&roots(&home, &program), &program),
            "el caso real de todas las plataformas no puede rechazarse"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn home_as_program_dir_is_not_removable() {
        let root = sandbox("home-igual");
        let home = root.join("home").join("ana");
        with_receipt(&home);
        assert!(
            !program_dir_is_removable(&roots(&home, &home), &home),
            "el propio $HOME nunca es borrable"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn home_ancestor_is_not_removable() {
        let root = sandbox("ancestro");
        let home = root.join("home").join("ana");
        with_receipt(&root);
        assert!(
            !program_dir_is_removable(&roots(&home, &root), &root),
            "un ancestro de $HOME se llevaría el perfil"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn data_root_is_not_removable() {
        let root = sandbox("datos");
        let home = root.join("home").join("ana");
        let datos = home.join("datos");
        with_receipt(&datos);
        let r = roots(&home, &datos);
        assert!(
            !program_dir_is_removable(&r, &datos),
            "otra raíz del producto nunca es borrable"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn without_receipt_or_exe_is_not_removable() {
        let root = sandbox("sin-recibo");
        let home = root.join("home").join("ana");
        let program = home.join("programas").join("ai-voice-interconnector");
        std::fs::create_dir_all(&program).expect("se crea el programa");
        assert!(
            !program_dir_is_removable(&roots(&home, &program), &program),
            "la mitad positiva exige recibo o ejecutable"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
