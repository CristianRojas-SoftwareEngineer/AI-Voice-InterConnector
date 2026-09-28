//! `cleanup` y `self uninstall` de punta a punta sobre raíces reubicadas a
//! temporales, criterios 17, 18, 20, 21, 22 y 23.
//!
//! Todas las raíces llegan como dato, no por variable de entorno: en Windows las
//! Known Folders ignoran `LOCALAPPDATA`, así que un sandbox que dependiera de ellas no
//! estaría probando el mecanismo de reubicación. El directorio de programa, la raíz de
//! datos, la raíz de modelos, el directorio de temporales y `$HOME` son todos del test,
//! y la condición de "raíz de modelos compartida" también —que es lo que hace que estas
//! pruebas no toquen el entorno del proceso y puedan correr en paralelo sin candado.

#![allow(clippy::disallowed_methods)]

use avi_lifecycle::channel::Channel;
use avi_lifecycle::cleanup::{self, Options as CleanupOptions, Roots};
use avi_lifecycle::daemon_stop::ProcessControl;
use avi_lifecycle::receipt::{self, InstallReceipt, PathIntegration};
use avi_lifecycle::uninstall::{self, Env as UninstallEnv, Options as UninstallOptions};
use std::path::{Path, PathBuf};

/// Control de procesos inerte: no hay daemon en el sandbox, así que la parada es un
/// no-op. Es el mismo `ProcessControl` que `daemon_stop` espera.
struct Inert;

impl ProcessControl for Inert {
    fn pid_alive(&self, _pid: u32) -> bool {
        false
    }
    fn kill_tree_by_pid(&self, _pid: u32) -> bool {
        false
    }
    fn resident_pid_alive(&self, _pid: u32) -> bool {
        false
    }
    fn kill_tree_resident_by_pid(&self, _pid: u32) -> bool {
        false
    }
    fn sweep_resident_by_image(&self) -> bool {
        false
    }
}

/// Removedor que borra ya. El caso diferido de Windows se ejercita con [`Diferido`],
/// que es el mismo mecanismo que implementa el binario.
struct Now;

impl uninstall::ProgramDirRemover for Now {
    fn exe_lives_inside(&self, _program_dir: &Path) -> bool {
        false
    }
    fn remove_now(&self, program_dir: &Path) -> anyhow::Result<()> {
        std::fs::remove_dir_all(program_dir)?;
        Ok(())
    }
    fn schedule(&self, _program_dir: &Path, _pid: u32) -> anyhow::Result<()> {
        Ok(())
    }
}

/// Removedor diferido: no borra nada y dice que lo programa. Es el caso del paso 8 de la
/// desinstalación,
/// en Windows.
struct Deferred;

impl uninstall::ProgramDirRemover for Deferred {
    fn exe_lives_inside(&self, _program_dir: &Path) -> bool {
        true
    }
    fn remove_now(&self, _program_dir: &Path) -> anyhow::Result<()> {
        anyhow::bail!("remove_now no debe llamarse con el ejecutable dentro")
    }
    fn schedule(&self, program_dir: &Path, pid: u32) -> anyhow::Result<()> {
        // El helper real escribe un script en el temporal del sistema; aquí basta con
        // registrar que se pidió, que es lo que el motor decide.
        write(&program_dir.join("borrado-programado"), &pid.to_string());
        Ok(())
    }
}

/// Removedor sin auxiliar disponible: el ejecutable está dentro y `schedule` falla. Es el
/// paso 8 cuando el borrado diferido no puede garantizarse, en cualquier plataforma.
struct Unschedulable;

impl uninstall::ProgramDirRemover for Unschedulable {
    fn exe_lives_inside(&self, _program_dir: &Path) -> bool {
        true
    }
    fn remove_now(&self, _program_dir: &Path) -> anyhow::Result<()> {
        anyhow::bail!("remove_now no debe llamarse con el ejecutable dentro")
    }
    fn schedule(&self, _program_dir: &Path, _pid: u32) -> anyhow::Result<()> {
        anyhow::bail!("el auxiliar terminó sin ejecutar su script")
    }
}

/// Sandbox con las cinco raíces.
struct Sandbox {
    root: PathBuf,
    program_dir: PathBuf,
    data_dir: PathBuf,
    models_dir: PathBuf,
    temp_root: PathBuf,
    home: PathBuf,
    /// La raíz de modelos es la caché HF compartida que eligió el usuario.
    models_shared: bool,
}

impl Sandbox {
    fn new(tag: &str) -> Self {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or_default();
        let root =
            std::env::temp_dir().join(format!("cleanup-e2e-{}-{tag}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let sandbox = Self {
            program_dir: root.join("opt").join("ai-voice-interconnector"),
            data_dir: root.join("data"),
            models_dir: root.join("models"),
            temp_root: root.join("tmp"),
            home: root.join("home").join("ana"),
            models_shared: false,
            root,
        };
        for dir in [
            &sandbox.program_dir,
            &sandbox.data_dir,
            &sandbox.models_dir,
            &sandbox.temp_root,
            &sandbox.home,
        ] {
            std::fs::create_dir_all(dir).expect("se crea el sandbox");
        }
        sandbox
    }

    fn roots(&self) -> Roots {
        Roots {
            program_dir: self.program_dir.clone(),
            data_dir: self.data_dir.clone(),
            models_dir: self.models_dir.clone(),
            temp_root: self.temp_root.clone(),
            home: self.home.clone(),
            models_shared: self.models_shared,
        }
    }

    /// Snapshot del contenido del sandbox, para afirmar que una simulación no modificó
    /// nada.
    fn snapshot(&self) -> Vec<(String, u64)> {
        let mut out = Vec::new();
        let mut stack = vec![self.root.clone()];
        while let Some(dir) = stack.pop() {
            let entries = match std::fs::read_dir(&dir) {
                Ok(entries) => entries,
                Err(_) => continue,
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let meta = match entry.metadata() {
                    Ok(meta) => meta,
                    Err(_) => continue,
                };
                let rel = path
                    .strip_prefix(&self.root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/");
                if meta.is_dir() {
                    out.push((rel, 0));
                    stack.push(path);
                } else {
                    out.push((rel, meta.len()));
                }
            }
        }
        out.sort();
        out
    }

    /// Planta el estado completo: modelos, voces de fábrica y de usuario, habla
    /// sintetizada, configuración, logs y pidfile.
    fn seed_state(&self) {
        // Modelos: un repo fijado, el derivado CT2, los locks, `xet` y un repo ajeno.
        write(
            &self
                .models_dir
                .join("models--Helsinki-NLP--opus-mt-es-en")
                .join("model.safetensors"),
            "pesos",
        );
        write(
            &self
                .models_dir
                .join("ct2")
                .join("opus-mt-es-en")
                .join("model.bin"),
            "ct2",
        );
        write(
            &self
                .models_dir
                .join(".locks")
                .join("models--x--y")
                .join("lock"),
            "",
        );
        write(&self.models_dir.join("xet").join("shard"), "xet");
        write(
            &self
                .models_dir
                .join("models--otra--herramienta")
                .join("otro.safetensors"),
            "ajeno",
        );
        // Voces: dos de fábrica y una de usuario, con su namespace de habla.
        for voice in ["default", "ryan", "mia"] {
            write(
                &self
                    .data_dir
                    .join("voices")
                    .join(voice)
                    .join("reference.qvoice"),
                "voz",
            );
        }
        for voice in ["default", "mia"] {
            write(
                &self.data_dir.join("speech").join(voice).join("hola.wav"),
                "wav",
            );
        }
        // Configuración, logs y estado del daemon.
        write(&self.data_dir.join("config.json"), "{}");
        write(&self.data_dir.join("logs").join("daemon.log"), "log");
        write(&self.data_dir.join("daemon.pid"), "{}");
    }

    /// Recibo de una instalación registrada, con las raíces del sandbox.
    fn receipt(&self) -> InstallReceipt {
        InstallReceipt::new(
            "0.24.0",
            avi_lifecycle::target::host_triple(),
            Channel::Script,
            &self.program_dir,
            vec![uninstall::executable_name_default()],
            PathIntegration::none(),
            receipt::Roots {
                data_dir: self.data_dir.clone(),
                cache_dir: self.models_dir.clone(),
            },
            None,
        )
    }

    /// Instala: escribe el ejecutable y el recibo. Las raíces llegan como dato y el
    /// directorio de programa lo da el recibo si lo hay: la operación actúa sobre la
    /// instalación registrada, no sobre la posición del ejecutable.
    fn install(&self) -> InstallReceipt {
        let receipt = self.receipt();
        write(
            &self.program_dir.join(uninstall::executable_name_default()),
            "binario",
        );
        receipt::write_to(&receipt, &self.program_dir).expect("se escribe el recibo");
        receipt
    }
}

/// Escribe un fichero, creando los directorios intermedios.
fn write(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("se crea el directorio padre");
    }
    std::fs::write(path, content).expect("se escribe el fichero");
}

/// Raíz de la unidad o del sistema de la plataforma, sin escribir nada en ella.
fn system_root() -> PathBuf {
    if cfg!(windows) {
        std::env::var("SystemDrive")
            .map(|d| PathBuf::from(format!("{d}:\\")))
            .unwrap_or_else(|_| PathBuf::from("C:\\"))
    } else {
        PathBuf::from("/")
    }
}

/// `true` si la ruta existe, includedo como enlace roto.
fn exists(path: &Path) -> bool {
    path.exists() || path.symlink_metadata().is_ok()
}

/// `--yes` para que la ejecución real no espere terminal: las pruebas affirmed el
/// resultado, no el prompt.
fn with_yes(options: CleanupOptions) -> CleanupOptions {
    CleanupOptions {
        assume_yes: true,
        ..options
    }
}

/// Runtime de un solo hilo: las operaciones del motor son asíncronas pero no
/// necesitan concurrencia, y un runtime por prueba evita el coste y el
/// `block_on` anidado de un runtime global.
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
}

/// `Env` de desinstalación con las raíces del sandbox y el directorio de programa
/// registrado que corresponde. Las raíces llegan como dato y el directorio de
/// programa lo da el recibo si lo hay: la operación actúa sobre la
/// instalación registrada, no sobre la posición del ejecutable.
fn env_uninstall<'a>(
    sandbox: &Sandbox,
    receipt: Option<&'a InstallReceipt>,
    channel: Channel,
) -> UninstallEnv<'a> {
    UninstallEnv {
        roots: sandbox.roots(),
        program_dir: receipt
            .map(|r| r.install_dir.clone())
            .unwrap_or_else(|| sandbox.program_dir.clone()),
        receipt,
        channel,
        daemon_addr: "127.0.0.1:0".to_string(),
        home: sandbox.home.clone(),
    }
}

/// Las rutas del plan, como cadenas, que es la forma en que el sobre las publica.
fn paths(plan: &cleanup::DeletionPlan) -> Vec<String> {
    plan.targets
        .iter()
        .map(|t| t.path.display().to_string())
        .collect()
}

/// Cada categoría borra **exactamente** su alcance, y `--model` en la raíz
/// exclusiva borra el directorio entero con `xet` dentro.
#[test]
fn cleanup_categories_are_scoped() {
    let sandbox = Sandbox::new("scoped");
    sandbox.seed_state();
    let roots = sandbox.roots();
    let voices = sandbox.data_dir.join("voices");
    let speech = sandbox.data_dir.join("speech");

    // --model: la raíz de modelos entera, y nada del estado.
    let model_only = cleanup::plan(
        &roots,
        &CleanupOptions {
            model: true,
            ..Default::default()
        },
    );
    assert_eq!(
        paths(&model_only),
        vec![sandbox.models_dir.display().to_string()]
    );
    assert!(sandbox.models_dir.join("xet").exists(), "antes de borrar");

    // --voices: las voces de usuario y el arrastre de su habla, y nada más.
    let voices_only = cleanup::plan(
        &roots,
        &CleanupOptions {
            voices: true,
            ..Default::default()
        },
    );
    let planned_removed = paths(&voices_only);
    assert!(planned_removed.contains(&voices.join("mia").display().to_string()));
    assert!(
        planned_removed.contains(&speech.join("mia").display().to_string()),
        "el arrastre de la locución de la voz: {planned_removed:?}"
    );
    assert!(
        !planned_removed.contains(&voices.join("default").display().to_string()),
        "las voces de fábrica no se borran: van embebidas"
    );
    assert!(
        !planned_removed.contains(&speech.join("default").display().to_string()),
        "ni sus locuciones"
    );

    // --synthetic-speech: la raíz de habla entera, `default` incluida.
    let speech_only = cleanup::plan(
        &roots,
        &CleanupOptions {
            synthetic_speech: true,
            ..Default::default()
        },
    );
    assert_eq!(paths(&speech_only), vec![speech.display().to_string()]);

    // --all: la unión más configuración, logs y pidfile, y **nunca** el programa.
    let all_plan = cleanup::plan(
        &roots,
        &CleanupOptions {
            all: true,
            ..Default::default()
        },
    );
    let planned_removed = paths(&all_plan);
    for expected in [
        sandbox.models_dir.display().to_string(),
        voices.join("mia").display().to_string(),
        speech.display().to_string(),
        sandbox.data_dir.join("config.json").display().to_string(),
        sandbox.data_dir.join("logs").display().to_string(),
        sandbox.data_dir.join("daemon.pid").display().to_string(),
    ] {
        assert!(
            planned_removed.contains(&expected),
            "falta {expected} en {planned_removed:?}"
        );
    }
    assert!(
        !planned_removed.contains(&sandbox.program_dir.display().to_string()),
        "el programa no lo borra `cleanup`: eso es `self uninstall`"
    );
    assert!(
        all_plan
            .preserved
            .iter()
            .any(|p| p.path == sandbox.program_dir),
        "y se dice explícitamente que se conserva"
    );

    // Y la ejecución real respeta el plan.
    let outcome = runtime()
        .block_on(cleanup::run(
            &roots,
            &with_yes(CleanupOptions {
                all: true,
                ..Default::default()
            }),
            &Inert,
        ))
        .expect("cleanup se ejecuta");
    assert_eq!(outcome.status, "cleanup_complete");
    assert!(
        outcome.failed.is_empty(),
        "nada falló: {:?}",
        outcome.failed
    );
    assert!(!exists(&sandbox.models_dir), "los modelos se van enteros");
    assert!(!exists(&voices.join("mia")));
    assert!(!exists(&speech));
    assert!(!exists(&sandbox.data_dir.join("config.json")));
    assert!(!exists(&sandbox.data_dir.join("logs")));
    assert!(!exists(&sandbox.data_dir.join("daemon.pid")));
    // Lo que no está en el alcance sobrevive.
    assert!(exists(&sandbox.program_dir), "el programa sobrevive");
    assert!(exists(&voices.join("default")));

    let _ = std::fs::remove_dir_all(&sandbox.root);
}

/// El gate sin categoría: `usage_error`, sin borrar nada (criterio 22).
#[test]
fn cleanup_without_category_is_usage_error() {
    let sandbox = Sandbox::new("gate");
    sandbox.seed_state();
    let before = sandbox.snapshot();

    let error = runtime()
        .block_on(cleanup::run(
            &sandbox.roots(),
            &CleanupOptions {
                assume_yes: true,
                ..Default::default()
            },
            &Inert,
        ))
        .expect_err("sin categoría es un error");

    let lifecycle = error
        .downcast_ref::<avi_lifecycle::LifecycleError>()
        .expect("es un LifecycleError");
    assert_eq!(lifecycle.reason, "usage_error");
    assert_eq!(lifecycle.exit_code, 2, "error de uso");
    assert_eq!(
        sandbox.snapshot(),
        before,
        "y no borra nada, ni siquiera el barrido"
    );

    let _ = std::fs::remove_dir_all(&sandbox.root);
}

/// `--dry-run` lista rutas con tamaños y no modifica el disco (criterio 20), y la lista
/// que anuncia es la misma que ejecutaría.
#[test]
fn dry_run_lists_paths_without_touching_disk() {
    let sandbox = Sandbox::new("dry-run");
    sandbox.seed_state();
    let before = sandbox.snapshot();
    let roots = sandbox.roots();

    let options = with_yes(CleanupOptions {
        model: true,
        voices: true,
        ..Default::default()
    });
    let plan = cleanup::plan(&roots, &options);
    let simulated = cleanup::simulate(&roots, &options);

    assert_eq!(
        sandbox.snapshot(),
        before,
        "una simulación no deja ni el archivo de bloqueo detrás"
    );
    assert!(simulated.dry_run);
    // El plan y la simulación vienen de la misma función, así que coinciden.
    let planned_removed = paths(&plan);
    for path in &planned_removed {
        assert!(
            simulated.removed.contains(path),
            "la simulación anuncia {path}, que el plan tiene"
        );
    }
    // Y los tamaños son los de la operación, no cero.
    assert!(
        plan.targets.iter().all(|t| t.size > 0),
        "todo destino tiene tamaño medido: {:?}",
        paths(&plan)
    );

    // Ahora sí, la ejecución real borra lo mismo que la simulación anunció.
    let real = runtime()
        .block_on(cleanup::run(&roots, &options, &Inert))
        .expect("cleanup se ejecuta");
    let real_removed: Vec<String> = real
        .removed
        .iter()
        .filter(|r| planned_removed.contains(r))
        .cloned()
        .collect();
    assert_eq!(
        real_removed, planned_removed,
        "la ejecución borra exactamente lo que el plan dice"
    );

    let _ = std::fs::remove_dir_all(&sandbox.root);
}

/// Con la raíz de modelos compartida, R3 manda: se borran los repos propios, el
/// derivado y sus locks, y **no** `xet`, ni el `.locks` completo, ni el repos de otra
/// herramienta (criterio 23).
///
/// Y la misma lista es la que anuncian el plan, la simulación y la ejecución: el
/// defecto que este lote absorbe era precisamente que el plan y el ejecutor no
/// coincidieran bajo raíz compartida.
#[test]
fn shared_hf_cache_keeps_foreign_entries() {
    let mut sandbox = Sandbox::new("shared");
    sandbox.models_shared = true;
    sandbox.seed_state();
    let hub = sandbox.models_dir.clone();
    let foreign = hub.join("models--otra--herramienta");
    write(
        &hub.join("models--Helsinki-NLP--opus-mt-es-en")
            .join("snapshots")
            .join("abc")
            .join("config.json"),
        "{}",
    );

    let roots = sandbox.roots();
    assert!(roots.models_shared, "el sandbox declara la raíz compartida");

    let options = with_yes(CleanupOptions {
        model: true,
        ..Default::default()
    });
    let plan = cleanup::plan(&roots, &options);
    let planned_removed = paths(&plan);

    for not_removable in [hub.join("xet"), hub.join(".locks"), foreign.clone()] {
        assert!(
            !planned_removed.contains(&not_removable.display().to_string()),
            "R3: {} no puede estar en el plan: {planned_removed:?}",
            not_removable.display()
        );
    }
    assert!(
        planned_removed.contains(
            &hub.join("models--Helsinki-NLP--opus-mt-es-en")
                .display()
                .to_string()
        ),
        "el repo propio sí está: {planned_removed:?}"
    );
    assert!(
        planned_removed.contains(&hub.join("ct2").display().to_string()),
        "y el derivado CT2, que es atribuible a la aplicación"
    );

    // Lo que se conserva se anuncia **por regla**: la raíz compartida, `xet` y el
    // `.locks` completo. El repos ajeno no se anuncia uno a uno —una caché real tiene
    // cientos— sino que queda cubierto por la regla de la raíz.
    for shared in [&hub, &hub.join("xet"), &hub.join(".locks")] {
        assert!(
            plan.preserved
                .iter()
                .any(|p| p.path.as_path() == shared.as_path()),
            "y {} se anuncia como compartido",
            shared.display()
        );
    }

    // La simulación dice lo mismo.
    let simulated = cleanup::simulate(&roots, &options);
    for path in &planned_removed {
        assert!(
            simulated.removed.contains(path),
            "la simulación dice {path}"
        );
    }

    // Y la ejecución coincide con el plan bajo raíz compartida: este es el caso que
    // fallaba antes de este lote.
    let real = runtime()
        .block_on(cleanup::run(&roots, &options, &Inert))
        .expect("cleanup se ejecuta");
    assert_eq!(
        real.removed, planned_removed,
        "el plan y la ejecución coinciden bajo raíz compartida"
    );

    assert!(!exists(&hub.join("models--Helsinki-NLP--opus-mt-es-en")));
    assert!(!exists(&hub.join("ct2")));
    assert!(exists(&hub.join("xet")), "R3: `xet` sobrevive");
    assert!(
        exists(&hub.join(".locks")),
        "R3: el `.locks` completo sobrevive"
    );
    assert!(exists(&foreign), "el repo de otra herramienta sobrevive");
    assert!(exists(&hub), "la raíz compartida no se borra entera");

    let _ = std::fs::remove_dir_all(&sandbox.root);
}

/// Un temporal propio sin PID se barre: la regla exige que cualquier invocación barra los
/// temporales propios huérfanos, y uno sin PID no pertenece a ningún proceso vivo.
///
/// Este es el segundo punto heredado que este lote absorbe: el barrido solo borraba los
/// que llevaban PID muerto, así que un temporal huérfano sin PID —justo el que deja una
/// clonación o una síntesis— no se recogía nunca.
#[test]
fn orphan_temporary_without_pid_is_swept() {
    let sandbox = Sandbox::new("temp");
    let temp = sandbox.temp_root.join("avi_clone_x_1.qvoice");
    write(&temp, "audio");
    let foreign = sandbox.temp_root.join("lifecycle-test-ajeno.txt");
    write(&foreign, "no es del producto");

    let outcome = runtime()
        .block_on(cleanup::run(
            &sandbox.roots(),
            &with_yes(CleanupOptions {
                voices: true,
                ..Default::default()
            }),
            &Inert,
        ))
        .expect("cleanup se ejecuta");

    assert!(
        !exists(&temp),
        "el temporal propio sin PID se barre: {:?}",
        outcome.swept
    );
    assert!(
        outcome.swept.contains(&temp.display().to_string()),
        "y se informa como barrido: {:?}",
        outcome.swept
    );
    assert!(exists(&foreign), "lo ajeno al producto no se toca");

    let _ = std::fs::remove_dir_all(&sandbox.root);
}

/// `self uninstall` deja residuo cero en las raíces de propiedad exclusiva y termina
/// con éxito (criterio 17).
#[test]
fn uninstall_leaves_no_residue_in_exclusive_roots() {
    let sandbox = Sandbox::new("uninstall");
    let receipt = sandbox.install();
    sandbox.seed_state();

    let outcome = runtime()
        .block_on(uninstall::run(
            &env_uninstall(&sandbox, Some(&receipt), Channel::Script),
            &UninstallOptions {
                assume_yes: true,
                ..Default::default()
            },
            &Now,
            &Inert,
        ))
        .expect("uninstall se ejecuta");

    assert_eq!(outcome.status, "uninstalled");
    assert!(
        outcome.failed.is_empty(),
        "nada falló: {:?}",
        outcome.failed
    );
    assert!(outcome.program_dir_removed);
    assert!(!exists(&sandbox.program_dir), "programa: residuo cero");
    assert!(!exists(&sandbox.models_dir), "modelos: residuo cero");
    assert!(
        !exists(&sandbox.data_dir),
        "datos: residuo cero, voces de fábrica incluidas: el programa ya no está para \
         re-materializarlas"
    );

    let _ = std::fs::remove_dir_all(&sandbox.root);
}

/// `--keep-data` conserva modelos, voces y habla (criterio 18).
#[test]
fn uninstall_keep_data_preserves_models_voices_and_speech() {
    let sandbox = Sandbox::new("keep-data");
    let receipt = sandbox.install();
    sandbox.seed_state();
    let voices = sandbox.data_dir.join("voices");

    let outcome = runtime()
        .block_on(uninstall::run(
            &env_uninstall(&sandbox, Some(&receipt), Channel::Script),
            &UninstallOptions {
                keep_data: true,
                assume_yes: true,
                ..Default::default()
            },
            &Now,
            &Inert,
        ))
        .expect("uninstall se ejecuta");

    assert_eq!(outcome.status, "uninstalled");
    assert!(!exists(&sandbox.program_dir), "el programa sí se va");
    assert!(
        exists(&sandbox.models_dir.join("xet")),
        "los modelos se quedan: {:?}",
        outcome.preserved
    );
    assert!(exists(&voices.join("mia")), "las voces se quedan");
    assert!(
        exists(&sandbox.data_dir.join("speech").join("default")),
        "el habla se queda"
    );
    assert!(
        !exists(&sandbox.data_dir.join("logs")),
        "pero la configuración y los logs sí caen: son estado de ejecución, no datos"
    );
    for reason in ["modelos", "voces", "habla"] {
        assert!(
            outcome.preserved.iter().any(|p| p.reason.contains(reason)),
            "`{reason}` se anuncia como conservado: {:?}",
            outcome.preserved
        );
    }

    let _ = std::fs::remove_dir_all(&sandbox.root);
}

/// Repetido sobre un sistema limpio termina con éxito y `not_installed` (criterio 21).
#[test]
fn uninstall_is_idempotent() {
    let sandbox = Sandbox::new("idempotente");
    let receipt = sandbox.install();
    sandbox.seed_state();
    let runtime = runtime();

    let first = runtime
        .block_on(uninstall::run(
            &env_uninstall(&sandbox, Some(&receipt), Channel::Script),
            &UninstallOptions {
                assume_yes: true,
                ..Default::default()
            },
            &Now,
            &Inert,
        ))
        .expect("la primera se ejecuta");
    assert_eq!(first.status, "uninstalled");
    assert!(first.program_dir_removed);
    assert!(first.failed.is_empty(), "{:?}", first.failed);

    // Sin recibo y sin estado: éxito con `not_installed`, que es un desenlace y no un
    // error.
    let second = runtime
        .block_on(uninstall::run(
            &env_uninstall(&sandbox, None, Channel::Unmanaged),
            &UninstallOptions {
                assume_yes: true,
                ..Default::default()
            },
            &Now,
            &Inert,
        ))
        .expect("la segunda no es un error");
    assert_eq!(
        second.status, "not_installed",
        "repetido sobre un sistema limpio termina en éxito con not_installed"
    );
    assert!(second.removed.is_empty());

    let _ = std::fs::remove_dir_all(&sandbox.root);
}

/// El canal `homebrew` es `externally_managed` con el comando correcto, y nada se toca.
#[test]
fn uninstall_homebrew_is_externally_managed() {
    let sandbox = Sandbox::new("homebrew");
    let receipt = sandbox.install();
    sandbox.seed_state();
    let before = sandbox.snapshot();

    let error = runtime()
        .block_on(uninstall::run(
            &env_uninstall(&sandbox, Some(&receipt), Channel::Homebrew),
            &UninstallOptions {
                assume_yes: true,
                ..Default::default()
            },
            &Now,
            &Inert,
        ))
        .expect_err("homebrew es un error");

    let lifecycle = error
        .downcast_ref::<avi_lifecycle::LifecycleError>()
        .expect("es un LifecycleError");
    assert_eq!(lifecycle.reason, "externally_managed");
    assert_eq!(lifecycle.exit_code, 12);
    assert!(
        lifecycle.message.contains(uninstall::HOMEBREW_UNINSTALL),
        "el mensaje lleva el comando correcto: {}",
        lifecycle.message
    );
    assert!(
        lifecycle.message.contains("cleanup --all"),
        "y sugiere `cleanup --all` para el estado"
    );
    assert_eq!(sandbox.snapshot(), before, "nada se toca");

    let _ = std::fs::remove_dir_all(&sandbox.root);
}

/// R2: el directorio de programa no se borra si no contiene recibo ni ejecutable, ni si
/// es una raíz del sistema, `$HOME` o un ancestro de `$HOME`. Ni una variable de
/// reubicación ni un recibo manipulado pueden ampliar el alcance.
#[test]
fn uninstall_refuses_unsafe_program_dir() {
    let sandbox = Sandbox::new("r2");
    let roots = sandbox.roots();
    let exe = uninstall::executable_name_default();
    let receipt = sandbox.program_dir.join("install-receipt.json");

    // Con recibo: se puede borrar.
    write(&receipt, "{}");
    assert!(
        uninstall::program_dir_is_removable(&roots, &sandbox.program_dir),
        "con recibo, sí"
    );
    std::fs::remove_file(&receipt).ok();
    // Con ejecutable: se puede borrar.
    write(&sandbox.program_dir.join(&exe), "binario");
    assert!(
        uninstall::program_dir_is_removable(&roots, &sandbox.program_dir),
        "con ejecutable, también"
    );
    // Sin ninguno de los dos: no.
    std::fs::remove_file(sandbox.program_dir.join(&exe)).ok();
    assert!(
        !uninstall::program_dir_is_removable(&roots, &sandbox.program_dir),
        "sin recibo ni ejecutable, no: no es el directorio de programa"
    );

    // Con recibo, pero en una raíz prohibida: tampoco. La raíz del sistema se prueba
    // sin escribir nada en ella —`program_dir_is_removable` rechaza por estructura,
    // antes de mirar el recibo— y las demás se prueban con el recibo plantado, que es
    // lo que un recibo manipulado intentaría.
    for protected in [
        sandbox.home.clone(),
        sandbox.home.parent().unwrap().to_path_buf(),
        sandbox.data_dir.clone(),
        sandbox.models_dir.clone(),
        sandbox.temp_root.clone(),
    ] {
        write(&protected.join("install-receipt.json"), "{}");
        assert!(
            !uninstall::program_dir_is_removable(&roots, &protected),
            "R2 impide borrar {}, aunque tenga recibo",
            protected.display()
        );
    }
    assert!(
        !uninstall::program_dir_is_removable(&roots, &system_root()),
        "una raíz de unidad no se borra nunca"
    );
    assert!(
        !uninstall::program_dir_is_removable(&roots, Path::new("")),
        "una ruta vacía no es un directorio de programa"
    );

    let _ = std::fs::remove_dir_all(&sandbox.root);
}

/// En Windows, con el ejecutable en uso dentro del directorio de programa, el borrado se
/// programa y el desenlace es `removal_scheduled`, que es un éxito.
#[test]
fn uninstall_schedules_removal_when_the_executable_is_inside() {
    let sandbox = Sandbox::new("diferido");
    let receipt = sandbox.install();
    sandbox.seed_state();

    let outcome = runtime()
        .block_on(uninstall::run(
            &env_uninstall(&sandbox, Some(&receipt), Channel::Script),
            &UninstallOptions {
                assume_yes: true,
                ..Default::default()
            },
            &Deferred,
            &Inert,
        ))
        .expect("uninstall se ejecuta");

    assert_eq!(
        outcome.status, "removal_scheduled",
        "es un éxito, no un error"
    );
    assert!(!outcome.program_dir_removed, "y no se borró todavía");
    assert!(
        sandbox.program_dir.exists(),
        "el directorio sigue ahí, pendiente del proceso auxiliar"
    );
    assert!(
        sandbox.program_dir.join("borrado-programado").is_file(),
        "el borrado quedó programado con el PID del proceso"
    );
    // El estado sí se borró: el programa es lo único que espera.
    assert!(!exists(&sandbox.data_dir));

    let _ = std::fs::remove_dir_all(&sandbox.root);
}

/// Si el borrado del directorio de programa no se puede programar, el desenlace es
/// `program_dir_kept` con el código 22 y nunca un éxito limpio: el resto de la
/// desinstalación se completó y el directorio queda intacto, sin borrado parcial.
#[test]
fn uninstall_reports_program_dir_kept_when_removal_cannot_be_scheduled() {
    let sandbox = Sandbox::new("no-programable");
    let receipt = sandbox.install();
    sandbox.seed_state();
    let names_of = |dir: &Path| {
        let mut names: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        names.sort();
        names
    };
    let before = names_of(&sandbox.program_dir);
    assert!(
        !before.is_empty(),
        "la instalación de partida tiene archivos"
    );

    let outcome = runtime()
        .block_on(uninstall::run(
            &env_uninstall(&sandbox, Some(&receipt), Channel::Script),
            &UninstallOptions {
                assume_yes: true,
                ..Default::default()
            },
            &Unschedulable,
            &Inert,
        ))
        .expect("uninstall se ejecuta");

    assert_eq!(outcome.status, "uninstalled");
    assert!(!outcome.program_dir_removed);
    let error = outcome
        .lifecycle_error()
        .expect("hay un reason de contrato");
    assert_eq!(error.reason, "program_dir_kept");
    assert_eq!(error.exit_code, 22);
    assert_eq!(
        names_of(&sandbox.program_dir),
        before,
        "el directorio de programa sigue intacto"
    );
    // El estado de usuario sí se borró: solo el programa quedó en disco.
    assert!(!exists(&sandbox.data_dir));

    let _ = std::fs::remove_dir_all(&sandbox.root);
}

/// `--dry-run` de `self uninstall` imprime el plan y no modifica el disco (criterio 20).
#[test]
fn uninstall_dry_run_touches_nothing() {
    let sandbox = Sandbox::new("uninstall-dry");
    let receipt = sandbox.install();
    sandbox.seed_state();
    let before = sandbox.snapshot();

    let outcome = runtime()
        .block_on(uninstall::run(
            &env_uninstall(&sandbox, Some(&receipt), Channel::Script),
            &UninstallOptions {
                dry_run: true,
                assume_yes: true,
                ..Default::default()
            },
            &Now,
            &Inert,
        ))
        .expect("uninstall se ejecuta");

    assert!(outcome.dry_run);
    assert_eq!(sandbox.snapshot(), before, "no se modifica el disco");
    assert!(
        outcome
            .removed
            .iter()
            .any(|r| r == &sandbox.program_dir.display().to_string()),
        "y el programa aparece en el plan: {:?}",
        outcome.removed
    );

    let _ = std::fs::remove_dir_all(&sandbox.root);
}

/// La reversión del `PATH` sale del recibo, no de donde esté el ejecutable (paso 7
/// de la desinstalación). En Unix: el enlace, solo si apunta al directorio de programa, y
/// los bloques
/// delimitados de los perfiles.
#[cfg(unix)]
#[test]
fn uninstall_reverts_path_from_the_receipt() {
    use std::os::unix::fs::symlink;

    let sandbox = Sandbox::new("path-unix");
    let bin_dir = sandbox.home.join(".local").join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let link = bin_dir.join(avi_lifecycle::APP_NAME);
    let program_exe = sandbox
        .program_dir
        .join(uninstall::executable_name_default());
    sandbox.install();
    write(&program_exe, "binario");
    symlink(&program_exe, &link).unwrap();

    let profile = sandbox.home.join(".profile");
    write(&profile, "# inicio\n");
    avi_lifecycle::path_unix::write_block(&profile, &bin_dir, &sandbox.home).unwrap();

    let receipt = InstallReceipt::new(
        "0.24.0",
        avi_lifecycle::target::host_triple(),
        Channel::Script,
        &sandbox.program_dir,
        vec![uninstall::executable_name_default()],
        PathIntegration::unix(link.clone(), vec![profile.clone()]),
        receipt::Roots {
            data_dir: sandbox.data_dir.clone(),
            cache_dir: sandbox.models_dir.clone(),
        },
        None,
    );
    receipt::write_to(&receipt, &sandbox.program_dir).unwrap();

    let outcome = runtime()
        .block_on(uninstall::run(
            &env_uninstall(&sandbox, Some(&receipt), Channel::Script),
            &UninstallOptions {
                assume_yes: true,
                ..Default::default()
            },
            &Now,
            &Inert,
        ))
        .expect("uninstall se ejecuta");

    assert!(outcome.path_reverted);
    assert!(!exists(&link), "el enlace se retira");
    let text = std::fs::read_to_string(&profile).unwrap();
    assert_eq!(
        text, "# inicio\n",
        "el bloque delimitado se quita y el resto del perfil no se toca: {text:?}"
    );

    // Un enlace que apunta a otro sitio es de otra instalación y no se toca.
    let other = sandbox.root.join("otra-instalacion");
    write(&other.join(uninstall::executable_name_default()), "binario");
    let foreign_link = bin_dir.join("ajeno");
    symlink(
        other.join(uninstall::executable_name_default()),
        &foreign_link,
    )
    .unwrap();
    let foreign_receipt = InstallReceipt::new(
        "0.24.0",
        avi_lifecycle::target::host_triple(),
        Channel::Script,
        &sandbox.program_dir,
        vec![uninstall::executable_name_default()],
        PathIntegration::unix(foreign_link.clone(), Vec::new()),
        receipt::Roots {
            data_dir: sandbox.data_dir.clone(),
            cache_dir: sandbox.models_dir.clone(),
        },
        None,
    );
    assert!(
        !uninstall::revert_path(Some(&foreign_receipt), &sandbox.home),
        "un enlace que no apunta al programa no se toca"
    );
    assert!(foreign_link.symlink_metadata().is_ok());

    let _ = std::fs::remove_dir_all(&sandbox.root);
}
