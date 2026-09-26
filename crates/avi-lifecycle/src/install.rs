//! `self install` (§9.3): instalar, reparar y rechazar por bundle incompleto.
//!
//! Son los tres modos de §9.3, distinguidos por la posición del ejecutable que se
//! invoca, y los doce pasos de su flujo en orden. Ninguna de las dos cosas es nueva
//! en el producto: el instalador heredado no sabe qué debe haber alrededor del
//! binario, no sabe qué se integró en el `PATH`, no deja recibo, no repara una
//! instalación existente y no distingue ejecutar desde un bundle extraído a mano de
//! ejecutar desde la propia instalación.
//!
//! **La opción oculta `--channel dev` (§10.5) vive aquí.** El comando que la
//! invoca —`cargo xtask install`— es del Ciclo 4, y la otra mitad de §10.5
//! —ejecutar `package --no-compress` en un staging— no existe todavía. Lo que este
//! ciclo fija es el otro valor de la sección: **qué queda escrito en el recibo**, y que
//! el valor surte efecto **solo cuando el recibo se crea por primera vez**, de modo
//! que reparar una instalación no reescriba su canal.
//!
//! **El `PATH` se modifica en Unix por decisión cerrada D2**, se anuncia en el
//! resumen y se revierte exactamente al desinstalar; `--no-modify-path` lo
//! desactiva. Lo que se registra en el recibo es lo que se hizo de verdad, no lo que
//! se pensaba hacer: si el directorio del enlace ya estaba en el `PATH`, el recibo
//! dice que no se integró nada y `self uninstall` no toca el perfil.
//!
//! **El control de procesos entra por parámetro**, como en `daemon_stop`: el motor no
//! puede matar un árbol sin arrastrar `avi-daemon` y `avi-tts`. En T16 el binario
//! pasa el suyo, que son las funciones que ya endurecieron el protocolo.

use crate::channel::Channel;
use crate::daemon_stop::{self, ProcessControl, StopOutcome};
use crate::manifest;
use crate::path_unix;
use crate::receipt::{self, InstallReceipt, PathIntegration, Roots};
use crate::transaction;
use crate::{confirm, privileges, quarantine, setup, target, LifecycleError};
use std::path::{Path, PathBuf};

/// Los dos modos de ejecución de §9.3 por posición del ejecutable. El tercero —
/// ejecutable sin bundle alrededor— no es un modo sino un desenlace de la
/// validación del paso 2, y por eso lo produce [`crate::manifest::validate_bundle`]
/// con `bundle_invalid` y no [`Mode`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Fuera del directorio de programa: instalación, o reemplazo si ya hay una
    /// versión.
    Install,
    /// Dentro del directorio de programa: reparación. Se reaplican integración de
    /// `PATH`, permisos, cuarentena y recibo **sin copiar archivos**.
    Repair,
}

impl Mode {
    /// Literal del `status` del sobre, que es lo que el contrato de la CLI ve.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Install => "installed",
            Self::Repair => "repaired",
        }
    }
}

/// Entorno de la operación. Todo lo que §7 resuelve por rutas va aquí como dato y no
/// como llamada a `avi-store`, por dos razones: §13 exige que las pruebas aisladas
/// funcionen con las raíces reubicadas a temporales, y en Windows las Known Folders
/// ignoran `LOCALAPPDATA`, así que sin parámetros el sandbox no representaría nada
/// del mecanismo nuevo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Env {
    /// Ejecutable que se invoca, ya resuelto a ruta real: es lo que decide el modo.
    pub exe: PathBuf,
    pub version: String,
    /// Triple del target, que es lo que §9.3 paso 2 valida contra el manifiesto.
    pub target: String,
    /// Directorio de programa de §7.
    pub program_dir: PathBuf,
    /// Directorio del enlace. En Windows es el propio directorio de programa, y la
    /// entrada del `PATH` es esa misma ruta.
    pub bin_dir: PathBuf,
    /// Raíz de datos de §7, donde vive el pidfile del daemon.
    pub data_dir: PathBuf,
    /// Raíz de modelos de §7. Es la que se registra en el recibo y la que anuncia
    /// el resumen; el almacén la resuelve por `avi-store`, así que en una prueba
    /// reubicada tiene que venir de `AVI_CACHE_DIR` para que ambas coincidan.
    pub models_dir: PathBuf,
    /// Directorio de temporales del sistema, para el barrido de §9.1.
    pub temp_root: PathBuf,
    /// `$HOME`, del que cuelgan los perfiles de shell. En Windows existe para que
    /// `Env` tenga la misma forma en los cuatro targets; la parte de perfiles no se
    /// usa allí.
    pub home: PathBuf,
    /// `PATH` de la sesión en la que corre, para decidir si hace falta el bloque.
    pub path_env: String,
    /// Shell de `$SHELL`, para la tabla de archivos de arranque de §9.3.1.
    pub shell: path_unix::Shell,
    /// `$ZDOTDIR`, si el usuario la define.
    pub zdotdir: Option<PathBuf>,
    /// Subclave de registro donde se integra el `PATH` en Windows. Parámetro para que
    /// la prueba de integración opere sobre una clave propia, como pide §13. Vacía en
    /// Unix, donde no hay registro.
    pub registry_subkey: String,
    /// Dirección a la que se conecta el protocolo de parada cuando no hay pidfile.
    /// Parámetro por lo mismo que en `daemon_stop::stop`.
    pub daemon_addr: String,
    /// Origen del bundle para el recibo. El bootstrap de §9.2 lo estampa en la
    /// variable que se lee aquí; vacío para una reparación o el canal `dev`.
    pub source: Option<String>,
}

impl Env {
    /// Entorno con las raíces de §7 ya resueltas, que es lo que usa el binario.
    ///
    /// `exe` se guarda **tal cual**, con el prefijo verbatim que la API de Windows
    /// pueda devolver. No hace falta quitarlo aquí: `canonical_path_key` —la fuente
    /// única de la semántica de comparación de §7— lo hace antes de normalizar, así que
    /// `detect_mode` reconoce el directorio deprograma aunque las dos rutas lleguen en
    /// formas distintas. Normalizarlo aquí además duplicaría esa regla y la dejaría
    /// fuera de sitio el día que aparezca otro consumidor.
    pub fn from_current_exe(exe: PathBuf) -> anyhow::Result<Self> {
        let target = target::host_triple();
        target::ensure_supported(target)?;
        Ok(Self {
            exe,
            version: env!("CARGO_PKG_VERSION").to_string(),
            target: target.to_string(),
            program_dir: crate::install_dir(),
            bin_dir: crate::bin_dir(),
            data_dir: crate::data_dir(),
            models_dir: crate::models_cache_dir(),
            temp_root: std::env::temp_dir(),
            home: home_dir(),
            path_env: std::env::var("PATH").unwrap_or_default(),
            shell: path_unix::Shell::from_env(std::env::var("SHELL").ok().as_deref()),
            zdotdir: std::env::var("ZDOTDIR").ok().map(PathBuf::from),
            registry_subkey: registry_subkey_default(),
            daemon_addr: daemon_stop::DEFAULT_ADDR.to_string(),
            source: std::env::var("AVI_SOURCE_URL").ok(),
        })
    }
}

/// Subclave de registro del `PATH` del usuario, o cadena vacía donde no hay
/// registro.
#[cfg(windows)]
fn registry_subkey_default() -> String {
    crate::path_windows::ENV_SUBKEY.to_string()
}

#[cfg(not(windows))]
fn registry_subkey_default() -> String {
    String::new()
}

/// `$HOME` del usuario.
fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

/// Opciones de `self install`. El parseo se queda en el binario (§6.3).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Options {
    /// `--yes`: omite la confirmación. Instalar es no destructivo, así que sin
    /// terminal procede igual (§9.1); con terminal, `--yes` la salta.
    pub assume_yes: bool,
    /// `--no-setup`: no provisiona modelos. Es el criterio 6.
    pub no_setup: bool,
    /// `--no-modify-path`: no toca ningún perfil ni el registro.
    pub no_modify_path: bool,
    /// `--force`: resuelve un `path_conflict` (§9.3.1). No es el `--force` de
    /// `uninstall`, que desaparece en T16.
    pub force: bool,
    /// `--channel dev`, la opción oculta de §10.5. Solo surte efecto cuando el
    /// recibo se crea por primera vez.
    pub channel: Option<Channel>,
    /// `--with-voice-cloning`, que se pasa a `setup`.
    pub with_voice_cloning: bool,
}

impl Options {
    /// Opciones de una instalación desatendida, que es lo que el bootstrap de §9.2
    /// invoca.
    pub fn unattended() -> Self {
        Self {
            assume_yes: true,
            ..Self::default()
        }
    }
}

/// Estado de los modelos tras la operación: una de las cuatro líneas del resumen
/// final de §9.3.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelsState {
    /// `--no-setup`: no se provisionó nada.
    Skipped,
    /// Ya estaba todo provisionado y no se descargó nada.
    AlreadyProvisioned,
    /// Se provisionó lo que faltaba.
    Provisioned { count: usize },
    /// Se empezó a provisionar y falló. El programa queda instalado y basta reintentar
    /// con `setup`, que es lo que §9.1 llama éxito parcial de `setup_failed`.
    ///
    /// `cause` es el fallo **de la provisión**, con su propio `reason`: un fallo de
    /// descarga es `network_error` y un fallo de conversión de CT2 es
    /// `ct2_conversion_failed`. No es el `reason` de la operación, que es
    /// `setup_failed` y vive en [`Outcome::lifecycle_error`]: uno dice *qué* falló y el
    /// otro *qué dejó de completarse*, y confundirlos perdería el `reason` que la tabla
    /// de §9.1 reserva a cada caso.
    Failed { cause: LifecycleError },
}

impl ModelsState {
    /// Literal para el sobre.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Skipped => "skipped",
            Self::AlreadyProvisioned => "already_provisioned",
            Self::Provisioned { .. } => "provisioned",
            Self::Failed { .. } => "failed",
        }
    }
}

/// Desenlace de `self install`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// `installed` o `repaired`.
    pub status: &'static str,
    pub mode: Mode,
    /// Recibo escrito. En reparación es un recibo nuevo con los mismos valores de
    /// versión, canal y origen: reaplicar el recibo es uno de los pasos de §9.3.
    pub receipt: InstallReceipt,
    /// Resumen final de §9.3, paso 12.
    pub summary: Vec<String>,
    /// Resumen previo de §9.3, paso 4, el que se mostró **antes** de confirmar. Se
    /// conserva porque es el que anuncia los cambios y el que el motor de T16 compone
    /// el sobre; el final es el estado.
    pub summary_before: Vec<String>,
    /// Estado de la integración del `PATH`, tal como queda en el recibo y como lo
    /// revierte `self uninstall`. Registra el estado, no el diff de esta pasada.
    pub path_integration: PathIntegration,
    /// `true` si esta pasada **reescribió** el `PATH` —el enlace, el bloque de perfil o
    /// el valor del registro—. Es lo que decide si el resumen pide una terminal nueva,
    /// y no coincide con `path_integration.modify_path` cuando la entrada ya estaba.
    pub path_rewritten: bool,
    pub models: ModelsState,
    /// Desenlace de la parada del daemon, que el resumen necesita para indicar cómo
    /// reiniciarlo (§9.4, paso 11).
    pub daemon: StopOutcome,
    /// Una instalación ajena detectada, que §9.3 paso 3 solo avisa: coexistir con un
    /// Cask no bloquea.
    pub foreign_in_path: Option<PathBuf>,
    /// Archivos que la recuperación de §9.1 no pudo limpiar.
    pub recovery_kept: Vec<PathBuf>,
}

impl Outcome {
    /// `true` si la integración del `PATH` está en pie, esté o no la haya escrito
    /// esta pasada. Es el estado que el recibo registra.
    pub fn path_integrated(&self) -> bool {
        self.path_integration.modify_path
    }

    /// `true` si esta pasada reescribió el `PATH`, que es lo que hace que el resumen
    /// pida abrir una terminal nueva: el proceso que ejecutó `curl | sh` no puede
    /// cambiar el `PATH` de su shell padre (§9.3.1).
    pub fn path_changed(&self) -> bool {
        self.path_rewritten
    }

    /// `true` si el resumen tiene que pedir una terminal nueva.
    pub fn needs_new_terminal(&self) -> bool {
        self.path_changed()
    }

    /// `reason` de contrato y código de salida de la operación cuando el desenlace **no**
    /// es un éxito limpio, y `None` cuando lo es.
    ///
    /// §9.1 declara `setup_failed` como **éxito parcial** con código propio —`SetupFailed =
    /// 11` de la tabla cerrada—, no como error: el programa está instalado y lo único que
    /// falta es la provisión. Por eso vive aquí y no como `Err` de [`install`], que
    /// perdería el resumen del paso 12 y dejaría al usuario sin el estado de su
    /// instalación. Quien cablea decide qué hacer con él: emitir el sobre y salir por
    /// veredicto con el código, que es lo que hace `self install`.
    ///
    /// Es el **único** sitio donde se decide el `reason` de la operación, de modo que
    /// `self update` en el ciclo 2 no tenga que reinventarlo.
    pub fn lifecycle_error(&self) -> Option<LifecycleError> {
        match &self.models {
            ModelsState::Failed { cause } => Some(LifecycleError::setup_failed(
                setup_failed_message(&self.receipt.install_dir, cause),
            )),
            _ => None,
        }
    }

    /// Cancelación: el usuario dijo que no y §9.1 **no** la lista entre los `reason`,
    /// así que es salida 0 y ningún `reason`. El producto ya responde `Cancelado.`, y
    /// por eso esto es un desenlace y no un error.
    ///
    /// No hay recibo ni nada tocado en este punto: la confirmación es el paso 4 y el
    /// primer cambio en disco es del paso 6.
    fn cancelled(env: &Env, mode: Mode) -> Self {
        Self {
            status: "cancelled",
            mode,
            receipt: InstallReceipt::new(
                &env.version,
                &env.target,
                Channel::Script,
                &env.program_dir,
                Vec::new(),
                PathIntegration::none(),
                Roots {
                    data_dir: env.data_dir.clone(),
                    cache_dir: env.models_dir.clone(),
                },
                None,
            ),
            summary: vec!["Cancelado.".to_string()],
            summary_before: vec!["Cancelado.".to_string()],
            path_integration: PathIntegration::none(),
            path_rewritten: false,
            models: ModelsState::Skipped,
            daemon: StopOutcome::default(),
            foreign_in_path: None,
            recovery_kept: Vec::new(),
        }
    }
}

/// Modo de la operación, por posición del ejecutable.
///
/// La comparación es canónica y no de cadenas porque las dos rutas llegan de
/// resoluciones distintas —el ejecutable en ejecución suele venir ya resuelto por el
/// sistema y el directorio de programa por `AVI_INSTALL_DIR`— y una barra final de
/// más daría un modo equivocado sin dar ningún error.
pub fn detect_mode(exe: &Path, program_dir: &Path) -> Mode {
    let directorio = exe.parent().unwrap_or(Path::new(""));
    if crate::canonical_path_entry_matches(directorio, program_dir) {
        Mode::Repair
    } else {
        Mode::Install
    }
}

/// Compara dos versiones por sus tres números.
///
/// Devuelve `Ordering` y no un booleano porque el plan necesita distinguir los tres
/// casos: instalar una versión menor es una degradación y se confirma como operación
/// destructiva (§9.3, paso 4).
pub fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let numeros = |v: &str| -> Vec<u64> {
        v.split(['-', '+'])
            .next()
            .unwrap_or_default()
            .split('.')
            .map(|p| p.parse().unwrap_or(0))
            .collect()
    };
    let (va, vb) = (numeros(a), numeros(b));
    for i in 0..va.len().max(vb.len()) {
        let ord = va
            .get(i)
            .copied()
            .unwrap_or(0)
            .cmp(&vb.get(i).copied().unwrap_or(0));
        if ord != std::cmp::Ordering::Equal {
            return ord;
        }
    }
    std::cmp::Ordering::Equal
}

/// Directorio donde vive el bundle alrededor del ejecutable, que es el que §9.3
/// valida en el paso 2.
pub fn bundle_dir(exe: &Path) -> PathBuf {
    exe.parent().unwrap_or(Path::new(".")).to_path_buf()
}

/// Ejecuta `self install` completo.
pub async fn install(
    env: &Env,
    options: &Options,
    control: &dyn ProcessControl,
) -> anyhow::Result<Outcome> {
    // Ninguna operación pide elevación (§9.1, §12): en Unix `sudo` aborta aquí, antes
    // de tocar el disco, y en Windows se avisa por stderr.
    let report = privileges::ensure_per_user()?;
    if let Some(warning) = &report.warning {
        eprintln!("{warning}");
    }

    // ── Paso 1. Recuperación y bloqueo (§9.1) ────────────────────────────────────
    // La recuperación va con el bloqueo tomado: si no, el barrido podría llevarse por
    // delante el staging que otra operación está usando. Y el staging del que esta
    // operación va a instalar se declara en uso: §9.1 barre "stagings huérfanos", y
    // el bundle de §9.3 paso 2 todavía no es huérfano aunque esté recién extraído.
    let lock = crate::lock::acquire_at(&lock_path_for(&env.program_dir))?;
    let bundle = bundle_dir(&env.exe);
    let recovery = crate::recovery::recover(crate::recovery::Roots {
        program_dir: &env.program_dir,
        temp_root: &env.temp_root,
        in_use: Some(&bundle),
    })?;

    // ── Modo, por posición del ejecutable (§9.3) ─────────────────────────────────
    let mode = detect_mode(&env.exe, &env.program_dir);

    // ── Paso 2. Validar el bundle, sin modificar nada antes (§9.3) ───────────────
    let files = manifest::validate_bundle(&env.target, &bundle)?;
    let executable = manifest::target_section(&env.target)?.executable_path();

    // ── Paso 3. Detectar la instalación previa (§9.3) ───────────────────────────
    let previo = receipt::read_from(&env.program_dir).ok().flatten();
    let sustituye = previo.as_ref().map(|r| r.version.clone());
    // Una instalación ajena solo genera aviso de coexistencia y precedencia: §9.3
    // dice que no bloquea, y es T14 quien produce `externally_managed` para Homebrew.
    let foreign_in_path = previo
        .as_ref()
        .filter(|r| r.install_dir != env.program_dir)
        .map(|r| r.install_dir.clone());
    // En una reparación, lo que hay en el directorio de programa es la instalación,
    // así que exigir recibo no añade nada y convertiría una reparación de una
    // instalación en canal `dev` sin recibo —que §10.5 sí genera— en un error.
    let degradacion = previo
        .as_ref()
        .is_some_and(|r| compare_versions(&env.version, &r.version) == std::cmp::Ordering::Less);

    // ── Paso 4. Resumen previo y confirmación (§9.3) ────────────────────────────
    let pending = pending_models(options);
    let path_plan = plan_path(env, options);
    let previo_resumen = compose_summary(env, options, mode, &sustituye, &path_plan, &pending);
    // Una degradación se confirma como operación destructiva (§9.3, paso 4), aunque
    // instalar no borre nada por sí mismo.
    let kind = if degradacion {
        confirm::Kind::Destructive
    } else {
        confirm::Kind::NonDestructive
    };
    let entries: Vec<confirm::PlanEntry> = if degradacion {
        vec![confirm::PlanEntry::of(&env.program_dir)]
    } else {
        Vec::new()
    };
    let decision = confirmar(&previo_resumen, &entries, kind, options)?;
    if decision == confirm::Decision::Cancelled {
        return Ok(Outcome::cancelled(env, mode));
    }

    // ── Paso 5. Parar el daemon; si no se detiene, nada modificado (§9.3) ───────
    let daemon = daemon_stop::stop(&env.data_dir, &env.daemon_addr, control).await;
    daemon_stop::require_stopped(&daemon)?;

    // ── Paso 6. Reemplazo transaccional (§9.3, paso 6) ──────────────────────────
    // En reparación **no se copia nada**: se reaplican integración, permisos,
    // cuarentena y recibo sobre lo que ya está (§9.3, modo reparación).
    if mode == Mode::Install {
        let _ = transaction::replace(&env.program_dir, &bundle)?;
    }

    // ── Paso 7. Cuarentena, en macOS (§9.3, paso 7) ─────────────────────────────
    // No es un fallo de la instalación: la cuarentena que no se quita degrada el
    // arranque, y el resumen lo informa.
    let cuarentena = quarantine::strip(&env.program_dir);
    for (ruta, motivo) in &cuarentena.failed {
        eprintln!(
            "  no se pudo limpiar la cuarentena de {}: {motivo}",
            ruta.display()
        );
    }

    // ── Paso 8. Integración del PATH (§9.3.1) ──────────────────────────────────
    crate::faults::trip(crate::faults::FaultPoint::BeforePathIntegration)?;
    let program_exe = env.program_dir.join(&executable);
    let (path_integration, path_rewritten) = apply_path(env, options, &program_exe)?;

    // ── Paso 9. Aviso de instalación per-machine antigua, en Windows (§9.3) ─────
    // HKLM nunca se modifica, ni para escribir: solo se informa del comando exacto.
    let machine_warning = machine_path_warning(env);

    // ── Paso 10. Recibo atómico y liberación del bloqueo (§9.3) ─────────────────
    crate::faults::trip(crate::faults::FaultPoint::BeforeReceipt)?;
    let receipt = InstallReceipt::new(
        &env.version,
        &env.target,
        resolve_channel(options, previo.as_ref()),
        &env.program_dir,
        files,
        path_integration.clone(),
        Roots {
            data_dir: env.data_dir.clone(),
            cache_dir: env.models_dir.clone(),
        },
        source_of(env, previo.as_ref()),
    );
    receipt::write_to(&receipt, &env.program_dir)?;
    drop(lock);

    // ── Paso 11. `setup` en el mismo proceso, salvo `--no-setup` (§9.3) ─────────
    let models = if options.no_setup {
        ModelsState::Skipped
    } else {
        provision(&pending, options).await
    };

    // ── Paso 12. Resumen final (§9.3) ───────────────────────────────────────────
    let summary = final_summary(
        env,
        mode,
        &path_integration,
        path_rewritten,
        &previo_resumen,
        &models,
        &daemon,
        &machine_warning,
        foreign_in_path.as_ref(),
        &recovery.kept,
        !cuarentena.failed.is_empty(),
    );

    Ok(Outcome {
        status: mode.as_str(),
        mode,
        receipt,
        summary,
        summary_before: previo_resumen,
        path_integration,
        path_rewritten,
        models,
        daemon,
        foreign_in_path,
        recovery_kept: recovery.kept,
    })
}

/// Aplica la tabla de §9.1. `stdin` y `stderr` se toman aquí porque son la única
/// entrada y salida del prompt, y §9.1 exige que el prompt vaya a stderr para no
/// contaminar el sobre `--json`.
fn confirmar(
    resumen: &[String],
    entries: &[confirm::PlanEntry],
    kind: confirm::Kind,
    options: &Options,
) -> anyhow::Result<confirm::Decision> {
    let stdin = std::io::stdin();
    let mut entrada = stdin.lock();
    confirm::confirm(
        &confirm::Confirmation {
            kind,
            summary: resumen,
            entries,
            assume_yes: options.assume_yes,
            dry_run: false,
            stdin_is_terminal: std::io::IsTerminal::is_terminal(&stdin),
        },
        &mut entrada,
        &mut std::io::stderr(),
    )
}

/// Qué se va a hacer con el `PATH`, para el resumen previo de §9.3.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PathPlan {
    /// Primer archivo de arranque al que se añade el bloque, en Unix. Se registra
    /// solo el primero porque `write_block` es idempotente y el recibo necesita la
    /// lista, no el plan.
    pub block_file: Option<PathBuf>,
    /// `true` si se va a añadir la entrada del directorio de programa al registro.
    pub registry: bool,
    /// `true` si se va a crear el enlace simbólico.
    pub symlink: bool,
}

impl PathPlan {
    /// `true` si el plan no toca nada: el caso de `--no-modify-path` y el de
    /// `~/.local/bin` ya presente en el `PATH`.
    pub fn is_noop(&self) -> bool {
        !self.registry && !self.symlink && self.block_file.is_none()
    }
}

/// Calcula el plan de `PATH` sin tocar nada. Es el mismo cálculo que usa
/// [`apply_path`], para que el resumen previo anuncie lo que de verdad va a pasar.
pub fn plan_path(env: &Env, options: &Options) -> PathPlan {
    if options.no_modify_path {
        return PathPlan::default();
    }
    let mut plan = PathPlan {
        symlink: cfg!(unix),
        registry: cfg!(windows),
        block_file: None,
    };
    if cfg!(unix) && path_unix::needs_block(&env.path_env, &env.bin_dir, &env.home) {
        plan.block_file =
            path_unix::profile_targets(&env.home, env.zdotdir.as_deref(), env.shell, |p| {
                p.exists()
            })
            .into_iter()
            .next();
    }
    plan
}

/// Aplica la integración de `PATH` y devuelve **qué queda registrado en el recibo** y
/// **si el valor se ha reescrito**.
///
/// Son dos datos distintos y no se deben confundir. El recibo registra el **estado**
/// de la integración, no el diff de esta pasada: si registrara solo lo que se cambió,
/// una segunda instalación desde el mismo bundle escribiría un recibo sin entrada de
/// `PATH` y `self uninstall` no podría revertir la que puso la primera, dejando
/// residuo —el criterio 17—. El `bool` sí es el diff, y es lo que decide si el resumen
/// pide una terminal nueva.
pub fn apply_path(
    env: &Env,
    options: &Options,
    program_exe: &Path,
) -> anyhow::Result<(PathIntegration, bool)> {
    if options.no_modify_path {
        return Ok((PathIntegration::none(), false));
    }
    #[cfg(unix)]
    {
        let enlace = env.bin_dir.join(crate::APP_NAME);
        path_unix::create_symlink(&enlace, program_exe, options.force)?;
        let plan = plan_path(env, options);
        let mut bloques = Vec::new();
        let mut cambio = false;
        if let Some(bloque) = plan.block_file {
            if path_unix::write_block(&bloque, &env.bin_dir, &env.home).map_err(|e| {
                LifecycleError::new(
                    "path_conflict",
                    14,
                    format!("no se pudo escribir {}: {e}", bloque.display()),
                )
            })? {
                cambio = true;
            }
            // Se registra aunque no haya cambiado: el bloque está en el perfil, y
            // revertirlo es lo que hace `self uninstall`.
            bloques.push(bloque);
        }
        Ok((PathIntegration::unix(enlace, bloques), cambio))
    }
    #[cfg(windows)]
    {
        let _ = program_exe;
        let outcome =
            crate::path_windows::integrate(&env.registry_subkey, &env.bin_dir).map_err(|e| {
                LifecycleError::new("path_conflict", 14, format!("integración del PATH: {e}"))
            })?;
        // La entrada se registra siempre que la integración se aplicó: después de la
        // operación está en el `PATH`, y eso es lo que hay que revertir. El
        // `changed` es el diff, y vive aparte.
        Ok((
            PathIntegration::windows(env.bin_dir.clone()),
            outcome.changed,
        ))
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (env, program_exe);
        Ok((PathIntegration::none(), false))
    }
}

/// Detecta una instalación de Homebrew por la ruta del ejecutable (§8.2). Lo usa el
/// resumen previo para el aviso de coexistencia; `externally_managed` es de T14.
pub fn foreign_installation(exe: &Path) -> Option<PathBuf> {
    crate::channel::is_homebrew_path(exe).then(|| bundle_dir(exe))
}

/// Aviso de §9.3, paso 9: si el `PATH` de la sesión lleva una entrada de una
/// instalación per-machine antigua, se muestra el comando exacto para quitarla desde
/// una PowerShell de administrador. **HKLM nunca se modifica** y ni siquiera se lee:
/// el motor no pide elevación (§9.1), así que la información disponible es la del
/// `PATH` de la sesión, que es la misma que ve el usuario.
fn machine_path_warning(env: &Env) -> Option<String> {
    if !cfg!(windows) {
        return None;
    }
    let por_machine = env.path_env.split(';').any(|entrada| {
        let entrada = entrada.to_lowercase();
        !entrada.is_empty()
            && entrada.contains("program files")
            && entrada.contains("ai-voice-interconnector")
    });
    por_machine.then(|| {
        format!(
            "  Aviso: el PATH del sistema parece llevar una instalación antigua de \
             {}. HKLM no se modifica nunca. Para quitarla, en una PowerShell de \
             administrador:\n    [Environment]::SetEnvironmentVariable('Path', \
             (([Environment]::GetEnvironmentVariable('Path','Machine')).Split(';') | \
             Where-Object {{ $_ -notlike '*ai-voice-interconnector*' }}) -join ';', 'Machine')",
            crate::APP_NAME
        )
    })
}

/// Canal del recibo nuevo: `--channel dev` (§10.5) solo si el recibo se crea por
/// primera vez. Reparar una instalación existente conserva su canal, que es lo que
/// hace que `self update` siga trato una instalación `dev` como tal.
pub fn resolve_channel(options: &Options, previo: Option<&InstallReceipt>) -> Channel {
    previo.map_or_else(|| options.channel.unwrap_or(Channel::Script), |r| r.channel)
}

/// Origen del bundle en el recibo: el de la instalación anterior si la había, y si no
/// el que el bootstrap de §9.2 estampó. Un bundle extraído a mano no lo tiene, y ahí
/// `source` queda vacío.
fn source_of(env: &Env, previo: Option<&InstallReceipt>) -> Option<String> {
    previo
        .and_then(|r| r.source.clone())
        .or_else(|| env.source.clone())
}

/// Modelos pendientes de provisionar, calculados **antes** de descargar (§9.7).
///
/// El almacén se construye por `avi-store`, que resuelve la raíz de modelos por
/// `AVI_CACHE_DIR`; `Env::models_dir` tiene que coincidir con ella, y en producción
/// ambas salen de `crate::models_cache_dir()`. En una prueba reubicada, las dos
/// salen de la variable, que es justo el mecanismo que §7 declara para el aislamiento.
fn pending_models(options: &Options) -> setup::Pending {
    let store = avi_store::ModelStore::new();
    setup::pending(&store, &setup_options(options))
}

/// Opciones de `setup` que se derivan de las de `self install`.
fn setup_options(options: &Options) -> setup::Options {
    setup::Options {
        with_voice_cloning: options.with_voice_cloning,
        force_update: false,
        assume_yes: options.assume_yes,
        called_from_lifecycle: true,
    }
}

/// Paso 11: `setup` en el mismo proceso.
///
/// Un fallo **no** es un `Err`: §9.1 define `setup_failed` como éxito parcial con el
/// programa instalado y reintentable con `setup`, así que es un estado del desenlace y
/// no un fallo de la instalación. El `reason` de la operación lo decide
/// [`Outcome::lifecycle_error`]; lo que viaja aquí es el **fallo de la provisión**, con su
/// propio `reason`, que es lo que §9.1 reserva a cada caso: `network_error` para un fallo
/// de descarga y `ct2_conversion_failed` para uno de conversión.
async fn provision(pending: &setup::Pending, options: &Options) -> ModelsState {
    if pending.is_empty() {
        return ModelsState::AlreadyProvisioned;
    }
    let store = avi_store::ModelStore::new();
    let mut count = 0usize;
    for nombre in &pending.models {
        if let Err(e) = avi_store::ModelStore::ensure_downloaded(nombre).await {
            return ModelsState::Failed {
                cause: setup::map_download_failure(nombre, &e),
            };
        }
        count += 1;
    }
    for pair in &pending.ct2 {
        let Some(snapshot) = store.model_snapshot_path(&format!("marian-{pair}")) else {
            return ModelsState::Failed {
                cause: ct2_failure(pair, &format!("el snapshot de marian-{pair} no resuelve")),
            };
        };
        if let Err(e) = setup::convert(&snapshot, &avi_store::ct2_model_dir(pair)) {
            return ModelsState::Failed {
                cause: ct2_failure(pair, &format!("{e:#}")),
            };
        }
        count += 1;
    }
    let _ = options;
    ModelsState::Provisioned { count }
}

/// Fallo de conversión de un derivado CT2, con el `reason` que `docs/CLI/commands/SETUP.md`
/// publica para él y el **código genérico**, porque §9.1 no le declara fila propia y la
/// tabla cerrada del plan reserva eso al ciclo que lo declare.
///
/// El código de este `reason` nunca es el código de salida del proceso: es un `reason`
/// anidado, y el código de salida es el de la operación —`SetupFailed = 11`—. Anidarlo con
/// un 11 haría que un consumidor leyera un 11 donde §9.1 no lo promises.
fn ct2_failure(pair: &str, motivo: &str) -> LifecycleError {
    LifecycleError::new("ct2_conversion_failed", 1, format!("CT2 {pair}: {motivo}"))
}

/// Prose de `setup_failed`, en las palabras de §9.1: el programa queda instalado y basta
/// reintentar con `setup`.
///
/// **Una sola fuente** para el resumen del paso 12 y para el `reason` del sobre, para que
/// las dos salidas no puedan divergir: es la razón de que el mensaje viva aquí y no en las
/// dos ramas que lo imprimen.
fn setup_failed_message(program_dir: &Path, cause: &LifecycleError) -> String {
    format!(
        "la provisión de modelos no se completó ({}: {}). El programa queda instalado en {} \
         y basta reintentar con setup",
        cause.reason,
        cause.message,
        program_dir.display()
    )
}

/// Resumen previo de §9.3, paso 4, con el formato que la especificación muestra.
fn compose_summary(
    env: &Env,
    options: &Options,
    mode: Mode,
    sustituye: &Option<String>,
    path_plan: &PathPlan,
    pending: &setup::Pending,
) -> Vec<String> {
    let verbo = match mode {
        Mode::Install => "Se instalará",
        Mode::Repair => "Se reparará",
    };
    let mut out = vec![format!(
        "{verbo} {} {} ({})",
        crate::APP_NAME,
        env.version,
        env.target
    )];
    out.push(match (mode, sustituye) {
        (Mode::Install, Some(v)) => {
            format!(
                "  Programa:  {}   (reemplaza {v})",
                env.program_dir.display()
            )
        }
        (Mode::Install, None) => format!("  Programa:  {}", env.program_dir.display()),
        (Mode::Repair, _) => format!(
            "  Programa:  {}   (reparación: no se copian archivos)",
            env.program_dir.display()
        ),
    });
    out.push(format!(
        "  Comando:   {}",
        env.bin_dir.join(crate::APP_NAME).display()
    ));
    out.push(if options.no_modify_path {
        "  PATH:      no se modifica (--no-modify-path); invoca el comando por su ruta \
         completa."
            .to_string()
    } else if path_plan.is_noop() {
        format!(
            "  PATH:      {} ya está en el PATH; no se modifica",
            env.bin_dir.display()
        )
    } else if let Some(bloque) = &path_plan.block_file {
        format!(
            "  PATH:      se añadirá {} en {}",
            env.bin_dir.display(),
            bloque.display()
        )
    } else if path_plan.registry {
        format!(
            "  PATH:      se añadirá {} en el PATH del usuario",
            env.bin_dir.display()
        )
    } else {
        format!(
            "  PATH:      se creará el enlace {}",
            env.bin_dir.join(crate::APP_NAME).display()
        )
    });
    out.push(if pending.is_empty() {
        format!(
            "  Modelos:   ya están provisionados en {}",
            env.models_dir.display()
        )
    } else {
        format!(
            "  Modelos:   se descargarán ~{} GB en {}",
            pending.estimated_bytes() / 1_000_000_000 + 1,
            env.models_dir.display()
        )
    });
    out
}

/// Resumen final de §9.3, paso 12: versión, rutas, estado del `PATH` —con la
/// indicación de abrir una terminal nueva cuando corresponda— y estado de los
/// modelos.
#[allow(clippy::too_many_arguments)]
fn final_summary(
    env: &Env,
    mode: Mode,
    integracion: &PathIntegration,
    path_rewritten: bool,
    previo: &[String],
    models: &ModelsState,
    daemon: &StopOutcome,
    machine_warning: &Option<String>,
    foreign: Option<&PathBuf>,
    recovery_kept: &[PathBuf],
    cuarentena_incompleta: bool,
) -> Vec<String> {
    let mut out = vec![
        format!(
            "{} {}: {}",
            crate::APP_NAME,
            match mode {
                Mode::Install => "instalado",
                Mode::Repair => "reparado",
            },
            env.version
        ),
        format!("  Programa:  {}", env.program_dir.display()),
        format!(
            "  Comando:   {}",
            env.bin_dir.join(crate::APP_NAME).display()
        ),
    ];
    // El resumen final lleva **una** línea del `PATH`, y es la del **estado**, no la del
    // plan. Copiar aquí la línea del resumen previo —que dice lo que se va a hacer— y
    // añadir debajo la del estado imprimía las dos, y en una installation correcta el
    // usuario leía "se añadirá X" y "se añadió X" en el mismo bloque, que parece una
    // contradicción y no es más que el mismo hecho contado dos veces.
    //
    // El estado tiene tres casos y un cuarto que no ocurre: "se añadió" solo cuando esta
    // pasada **reescribió** el `PATH`, porque si ya estaba decirlo sería mentir sobre lo
    // que acaba de pasar; "ya estaba integrado" cuando la integración está en pie y esta
    // pasada no la tocó; y el texto del plan cuando no se va a integrar nada
    // (`--no-modify-path`), donde el plan *es* el estado porque no hubo escritura.
    if path_rewritten {
        out.push(format!(
            "  PATH:      se añadió {}; abre una terminal nueva para que el comando \
             se encuentre.",
            env.bin_dir.display()
        ));
    } else if integracion.modify_path {
        out.push(format!(
            "  PATH:      {} ya estaba integrado; no se ha modificado.",
            env.bin_dir.display()
        ));
    } else if let Some(linea) = previo.iter().find(|l| l.trim_start().starts_with("PATH:")) {
        out.push(format!(
            "  PATH:      {}",
            linea.trim_start().trim_start_matches("PATH:").trim()
        ));
    }
    out.push(format!("  Modelos:   {}", models.as_str()));
    if let ModelsState::Failed { cause } = models {
        // La misma frase que `Outcome::lifecycle_error` pone en el `reason` del sobre, y
        // con la causa debajo: el usuario ve qué falló, no solo que algo falló.
        out.push(format!(
            "  Aviso: {}.",
            setup_failed_message(&env.program_dir, cause)
        ));
        out.push(format!("  Causa:  {} ({})", cause.message, cause.reason));
    }
    if let Some(warning) = machine_warning {
        out.push(warning.clone());
    }
    if let Some(raiz) = foreign {
        out.push(format!(
            "  Aviso: hay otra instalación de {} en {}; en el PATH tiene precedencia \
             la primera que aparezca.",
            crate::APP_NAME,
            raiz.display()
        ));
    }
    if !recovery_kept.is_empty() {
        out.push(format!(
            "  Aviso: quedaron {} recurso(s) de una operación anterior sin poder \
             limpiarse; `doctor` los informa.",
            recovery_kept.len()
        ));
    }
    if cuarentena_incompleta {
        out.push(
            "  Aviso: la cuarentena de macOS no se pudo limpiar de todo el directorio \
             de programa."
                .to_string(),
        );
    }
    if daemon.was_running {
        out.push(
            "  Daemon: estaba en ejecución y se ha parado; se relanza con el comando \
             habitual."
                .to_string(),
        );
    }
    out
}

/// Ruta del archivo de bloqueo de §7: hermano del directorio de programa, que es
/// donde la tabla de rutas lo coloca en los cuatro targets. Es la misma que
/// [`crate::lock::lock_path`], calculada desde el directorio de programa que trae la
/// operación en vez de del del entorno.
/// Ruta del archivo de bloqueo: la regla vive en [`crate::lock`] porque las tres
/// operaciones destructivas la necesitan y §7 no admite tres copias.
fn lock_path_for(program_dir: &Path) -> PathBuf {
    crate::lock::lock_path_for(program_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// El modo depende **solo** de la posición del ejecutable, y la comparación es
    /// canónica: una barra final de más no puede dar un modo equivocado sin dar ningún
    /// error.
    #[test]
    fn mode_depends_only_on_the_executable_position() {
        let programa = Path::new("/home/ana/.local/opt/ai-voice-interconnector");
        let dentro = programa.join("ai-voice-interconnector");
        assert_eq!(detect_mode(&dentro, programa), Mode::Repair);
        assert_eq!(
            detect_mode(
                &dentro,
                Path::new("/home/ana/.local/opt/ai-voice-interconnector/")
            ),
            Mode::Repair,
            "una barra final de más no cambia el modo"
        );
        assert_eq!(
            detect_mode(
                &dentro,
                Path::new("/home/ana/.local/opt/ai-voice-interconnector/./")
            ),
            Mode::Install,
            "un `./` final **sí** cambia el resultado, y es una limitación declarada: \
             `avi_store::canonical_path_key` normaliza por cadenas —separadores, \
             mayúsculas, `trim`— y no por componentes de `Path`, así que no resuelve \
             `.` ni `..`. No lo resuelve nadie: la ruta del ejecutable la da el \
             sistema, ya normalizada, y la del directorio de programa viene de una \
             variable de reubicación. Cambiarlo sería tocar `avi-store`, que es de T1."
        );
        // El prefijo verbatim de Windows lo quita `canonical_path_key`, que es la
        // fuente única de la semántica de comparación de §7. Antes lo hacía este
        // módulo, y el arreglo permanente está en `avi-store`.
        //
        // La rama va con `#[cfg(windows)]` y no con `cfg!(windows)`: en Unix el prefijo
        // no existe, `canonical_path_key` no lo quita, y la aserción sería falsa por
        // construcción en lugar de estar ausente.
        #[cfg(windows)]
        {
            assert_eq!(
                detect_mode(
                    Path::new(
                        r"\\?\C:\home\ana\.local\opt\ai-voice-interconnector\ai-voice-interconnector"
                    ),
                    Path::new(r"C:\home\ana\.local\opt\ai-voice-interconnector")
                ),
                Mode::Repair,
                "con el prefijo solo en el ejecutable, que es como llega de la API de \
                 Windows, el modo sigue siendo reparación"
            );
            assert_eq!(
                detect_mode(
                    Path::new(
                        r"\\?\C:\home\ana\.local\opt\ai-voice-interconnector\ai-voice-interconnector"
                    ),
                    Path::new(r"\\?\C:\home\ana\.local\opt\ai-voice-interconnector")
                ),
                Mode::Repair,
                "y también cuando las dos rutas lo llevan"
            );
        }
        assert_eq!(
            detect_mode(Path::new("/opt/staging/ai-voice-interconnector"), programa),
            Mode::Install,
            "un staging hermano es instalación"
        );
        assert_eq!(
            detect_mode(
                Path::new("/repo/target/release/ai-voice-interconnector"),
                programa
            ),
            Mode::Install,
            "`target/release` no es el directorio de programa"
        );
        assert_eq!(Mode::Install.as_str(), "installed");
        assert_eq!(Mode::Repair.as_str(), "repaired");
        assert_eq!(
            bundle_dir(&dentro),
            programa,
            "el bundle es el directorio del exe"
        );
    }

    /// La degradación se distingue de la actualización y de la misma versión, que es
    /// lo que §9.3 paso 4 necesita para decidir la clase de confirmación.
    #[test]
    fn version_ordering_separates_downgrade() {
        use std::cmp::Ordering;
        assert_eq!(compare_versions("0.24.0", "0.23.1"), Ordering::Greater);
        assert_eq!(
            compare_versions("0.23.1", "0.24.0"),
            Ordering::Less,
            "degradación"
        );
        assert_eq!(compare_versions("0.24.0", "0.24.0"), Ordering::Equal);
        assert_eq!(
            compare_versions("0.24.0", "0.24.0-rc.1"),
            Ordering::Equal,
            "el sufijo de precompilado no cuenta"
        );
        assert_eq!(compare_versions("1.0.0", "0.99.99"), Ordering::Greater);
        assert_eq!(
            compare_versions("0.24", "0.24.0"),
            Ordering::Equal,
            "un componente falta es 0"
        );
        assert_eq!(compare_versions("0.24.1", "0.24"), Ordering::Greater);
        assert_eq!(compare_versions("", "0.0.0"), Ordering::Equal);
    }

    /// `--channel dev` de §10.5 solo surte efecto cuando el recibo se crea por
    /// primera vez. Reparar una instalación existente conserva su canal: si no, una
    /// reparación reescribiría el canal y `self update` dejaría de reconocer la
    /// instalación como `dev`.
    #[test]
    fn channel_dev_only_on_first_receipt() {
        let dev = Options {
            channel: Some(Channel::Dev),
            ..Options::default()
        };
        assert_eq!(resolve_channel(&dev, None), Channel::Dev);
        assert_eq!(resolve_channel(&Options::default(), None), Channel::Script);

        for canal in [Channel::Dev, Channel::Script, Channel::Unmanaged] {
            let recibo = recibo_de(canal);
            assert_eq!(
                resolve_channel(&dev, Some(&recibo)),
                canal,
                "reparar conserva el canal {canal:?} aunque venga `--channel dev`"
            );
        }
    }

    /// El recibo registra el origen de la instalación anterior, no el de la
    /// reparación: una reparación no cambia de dónde vino el bundle.
    #[test]
    fn receipt_source_is_preserved_across_repair() {
        let env = env_de_prueba();
        let previo = InstallReceipt::new(
            "0.23.1",
            &env.target,
            Channel::Script,
            &env.program_dir,
            vec![env.target.clone()],
            PathIntegration::none(),
            Roots {
                data_dir: env.data_dir.clone(),
                cache_dir: env.models_dir.clone(),
            },
            Some("https://example.invalid/bundle.tar.gz".to_string()),
        );
        assert_eq!(
            source_of(&env, Some(&previo)),
            previo.source,
            "el origen se conserva"
        );

        let mut sin_origen = env.clone();
        sin_origen.source = Some("https://otro.invalid/bundle.tar.gz".to_string());
        assert_eq!(
            source_of(&sin_origen, Some(&previo)),
            previo.source,
            "el del entorno no pisa el del recibo"
        );
        assert_eq!(
            source_of(&sin_origen, None),
            sin_origen.source,
            "sin recibo previo, manda el del entorno"
        );
        assert_eq!(
            source_of(&env, None),
            None,
            "un bundle a mano no tiene origen"
        );
    }

    /// `--no-modify-path` no toca nada en ninguna plataforma, y sin él el plan nombra
    /// el archivo de arranque que se va a tocar: es la línea que el resumen anuncia.
    #[test]
    fn path_plan_respects_no_modify_path() {
        let env = env_de_prueba();
        let bloqueado = Options {
            no_modify_path: true,
            ..Options::default()
        };
        assert!(
            plan_path(&env, &bloqueado).is_noop(),
            "`--no-modify-path` no toca nada en ninguna plataforma"
        );
        let (integracion, cambiado) = apply_path(&env, &bloqueado, Path::new("/x")).unwrap();
        assert_eq!(integracion, PathIntegration::none());
        assert!(
            !integracion.modify_path,
            "y el recibo dice que no se integró nada"
        );
        assert!(!cambiado, "ni se reescribió nada");
    }

    fn recibo_de(canal: Channel) -> InstallReceipt {
        let env = env_de_prueba();
        InstallReceipt::new(
            "0.23.1",
            &env.target,
            canal,
            &env.program_dir,
            vec!["ai-voice-interconnector".to_string()],
            PathIntegration::none(),
            Roots {
                data_dir: env.data_dir.clone(),
                cache_dir: env.models_dir.clone(),
            },
            None,
        )
    }

    /// `Env` de pruebas. Las rutas son de Unix a propósito: solo se usan en cálculos
    /// que no tocan el disco, y los valores absolutos no tienen que existir para que
    /// la comparación canónica funcione.
    pub(crate) fn env_de_prueba() -> Env {
        Env {
            exe: PathBuf::from("/opt/staging/ai-voice-interconnector"),
            version: "0.24.0".to_string(),
            target: "x86_64-unknown-linux-gnu".to_string(),
            program_dir: PathBuf::from("/home/ana/.local/opt/ai-voice-interconnector"),
            bin_dir: PathBuf::from("/home/ana/.local/bin"),
            data_dir: PathBuf::from("/home/ana/.local/share/ai-voice-interconnector"),
            models_dir: PathBuf::from("/home/ana/.cache/ai-voice-interconnector/models"),
            temp_root: PathBuf::from("/tmp"),
            home: PathBuf::from("/home/ana"),
            path_env: "/usr/bin:/bin".to_string(),
            shell: path_unix::Shell::Bash,
            zdotdir: None,
            registry_subkey: String::new(),
            daemon_addr: "127.0.0.1:1".to_string(),
            source: None,
        }
    }
}
