// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! `cleanup` y `self uninstall` de punta a punta sobre raíces reubicadas a
//! temporales.
//!
//! Todas las raíces llegan como dato, no por variable de entorno: en Windows las
//! Known Folders ignoran `LOCALAPPDATA`, así que un sandbox que dependiera de ellas no
//! estaría probando el mecanismo de reubicación. El directorio de programa, la raíz de
//! datos, la raíz de modelos, el directorio de temporales y `$HOME` son todos del test,
//! y la condición de "raíz de modelos compartida" también —que es lo que hace que estas
//! pruebas no toquen el entorno del proceso y puedan correr en paralelo sin candado.

#![allow(clippy::disallowed_methods)]

#[path = "support/inert.rs"]
mod inert;
#[path = "support/inspect.rs"]
mod inspect;
#[path = "support/registered.rs"]
mod registered;
#[path = "support/repo.rs"]
mod repo;
#[path = "support/runtime.rs"]
mod runtime;
#[path = "support/shared.rs"]
mod shared;
mod support;

use avi_core::exit_codes::ExitCode;
use avi_lifecycle::channel::Channel;
use avi_lifecycle::cleanup::{self, Options as CleanupOptions};
use avi_lifecycle::receipt::PathIntegration;
use avi_lifecycle::uninstall::{self, Options as UninstallOptions};
use inert::Inert;
use inspect::exists;
use registered::Now;
use runtime::runtime;
use std::path::{Path, PathBuf};
use support::{write, Sandbox};

/// Removedor diferido: no borra nada y dice que lo programa. Es el caso del paso 8 de la
/// desinstalación,
/// en Windows.
struct Deferred;

impl uninstall::ProgramDirRemover for Deferred {
    fn exe_lives_inside(&self, _program_dir: &Path) -> anyhow::Result<bool> {
        Ok(true)
    }
    fn remove_now(&self, _program_dir: &Path) -> anyhow::Result<()> {
        anyhow::bail!("remove_now no debe llamarse con el ejecutable dentro")
    }
    fn schedule(&self, program_dir: &Path, wait: &avi_process::OwnerToken) -> anyhow::Result<()> {
        // El limpiador real lanza PowerShell; aquí basta con registrar que se
        // pidió, que es lo que el motor decide.
        write(
            &program_dir.join("scheduled-removal"),
            &wait.pid().to_string(),
        );
        Ok(())
    }
}

/// Removedor sin auxiliar disponible: el ejecutable está dentro y `schedule` falla. Es el
/// paso 8 cuando el borrado diferido no puede garantizarse, en cualquier plataforma.
struct Unschedulable;

impl uninstall::ProgramDirRemover for Unschedulable {
    fn exe_lives_inside(&self, _program_dir: &Path) -> anyhow::Result<bool> {
        Ok(true)
    }
    fn remove_now(&self, _program_dir: &Path) -> anyhow::Result<()> {
        anyhow::bail!("remove_now no debe llamarse con el ejecutable dentro")
    }
    fn schedule(&self, _program_dir: &Path, _wait: &avi_process::OwnerToken) -> anyhow::Result<()> {
        anyhow::bail!("el auxiliar terminó sin ejecutar su script")
    }
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

/// `--yes` para que la ejecución real no espere terminal: las pruebas afirman el
/// resultado, no el prompt.
fn with_yes(options: CleanupOptions) -> CleanupOptions {
    CleanupOptions {
        assume_yes: true,
        ..options
    }
}

/// Las rutas del plan, como cadenas, que es la forma en que el envelope las publica.
fn paths(plan: &cleanup::DeletionPlan) -> Vec<String> {
    plan.paths()
        .iter()
        .map(|p| p.display().to_string())
        .collect()
}

/// Un directorio del barrido que existe y no se puede listar (aquí, un fichero en lugar
/// de la raíz del scratch) sale en `unreadable`, tanto en la simulación como en la
/// ejecución real.
#[test]
fn cleanup_reports_unlistable_sweep_directories() {
    let sandbox = Sandbox::new("unlistable");
    let temp_file = sandbox.root.join("temp-is-a-file");
    write(&temp_file, "no es un directorio");
    let mut roots = sandbox.roots();
    roots.scratch_root = temp_file.clone();
    let options = with_yes(CleanupOptions {
        voices: true,
        ..Default::default()
    });

    let planned = cleanup::simulate(&roots, &options);
    assert_eq!(planned.unreadable, vec![temp_file.display().to_string()]);

    let executed = runtime()
        .block_on(cleanup::run(&roots, &options, &Inert))
        .expect("la limpieza se ejecuta");
    assert_eq!(executed.unreadable, vec![temp_file.display().to_string()]);
    assert!(
        executed.kept.contains(&temp_file.display().to_string()),
        "y queda pendiente: {:?}",
        executed.kept
    );
}

/// Cada categoría borra **exactamente** su alcance, y `--model` en la raíz
/// exclusiva borra el directorio entero con `xet` dentro.
#[test]
fn cleanup_categories_are_scoped() {
    let sandbox = Sandbox::new("scoped");
    sandbox.seed_state();
    // El programa instalado existe: `cleanup` lo conserva y lo dice.
    sandbox.install_registered(PathIntegration::none());
    let roots = sandbox.roots();
    let voices = sandbox.data_dir.join("voices");
    let speech = sandbox.data_dir.join("speech");

    // --model: la raíz de modelos entera, y nada del estado. El contenedor de caché
    // queda vacío con ella (aquí no hay temporales) y el plan anuncia su poda.
    let model_only = cleanup::plan(
        &roots,
        &CleanupOptions {
            model: true,
            ..Default::default()
        },
    );
    assert_eq!(
        paths(&model_only),
        vec![
            sandbox.models_dir.display().to_string(),
            sandbox.cache_dir.display().to_string()
        ]
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
    assert!(planned_removed.contains(&voices.join("mine").display().to_string()));
    assert!(
        planned_removed.contains(&speech.join("mine").display().to_string()),
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
        voices.join("mine").display().to_string(),
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
    assert!(!exists(&voices.join("mine")));
    assert!(!exists(&speech));
    assert!(!exists(&sandbox.data_dir.join("config.json")));
    assert!(!exists(&sandbox.data_dir.join("logs")));
    assert!(!exists(&sandbox.data_dir.join("daemon.pid")));
    // Lo que no está en el alcance sobrevive.
    assert!(exists(&sandbox.program_dir), "el programa sobrevive");
    assert!(exists(&voices.join("default")));
}

/// El gate sin categoría: `usage_error`, sin borrar nada.
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
    assert_eq!(
        ExitCode::from_reason(lifecycle.reason).code(),
        2,
        "error de uso"
    );
    assert_eq!(
        sandbox.snapshot(),
        before,
        "y no borra nada, ni siquiera el barrido"
    );
}

/// `--dry-run` lista rutas con tamaños y no modifica el disco, y la lista
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
}

/// Con la raíz de modelos compartida manda la regla de recursos compartidos: se borran
/// los repos propios y sus locks, y **no** `xet`, ni el `.locks` completo, ni el repo de
/// otra herramienta.
///
/// Y la misma lista es la que anuncian el plan, la simulación y la ejecución: bajo
/// raíz compartida el plan y el ejecutor tienen que coincidir.
#[test]
fn shared_hf_cache_keeps_foreign_entries() {
    let sandbox = Sandbox::new_shared("shared");
    sandbox.seed_state();
    let hub = sandbox.models_dir.clone();
    let foreign = hub.join("models--other--tool");
    write(
        &hub.join("models--CristianRojaas--opus-mt-es-en-ct2-int8")
            .join("snapshots")
            .join("abc")
            .join("config.json"),
        "{}",
    );

    let roots = sandbox.roots();
    assert_eq!(
        roots.shared_models_dir.as_ref(),
        Some(&hub),
        "el sandbox declara la raíz compartida"
    );

    let options = with_yes(CleanupOptions {
        model: true,
        ..Default::default()
    });
    let plan = cleanup::plan(&roots, &options);
    let planned_removed = paths(&plan);

    for not_removable in [hub.join("xet"), hub.join(".locks"), foreign.clone()] {
        assert!(
            !planned_removed.contains(&not_removable.display().to_string()),
            "{} no puede estar en el plan: {planned_removed:?}",
            not_removable.display()
        );
    }
    assert!(
        planned_removed.contains(
            &hub.join("models--CristianRojaas--opus-mt-es-en-ct2-int8")
                .display()
                .to_string()
        ),
        "el repo propio sí está: {planned_removed:?}"
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

    // Y la ejecución coincide con el plan bajo raíz compartida.
    let real = runtime()
        .block_on(cleanup::run(&roots, &options, &Inert))
        .expect("cleanup se ejecuta");
    assert_eq!(
        real.removed, planned_removed,
        "el plan y la ejecución coinciden bajo raíz compartida"
    );

    assert!(!exists(
        &hub.join("models--CristianRojaas--opus-mt-es-en-ct2-int8")
    ));
    assert!(exists(&hub.join("xet")), "`xet` sobrevive");
    assert!(
        exists(&hub.join(".locks")),
        "el `.locks` completo sobrevive"
    );
    assert!(exists(&foreign), "el repo de otra herramienta sobrevive");
    assert!(exists(&hub), "la raíz compartida no se borra entera");
}

/// El scratch de un proceso muerto se barre con la identidad real del sistema, y lo que
/// no es un scratch (un nombre que no es un token) no se toca: cualquier invocación barre
/// los residuos huérfanos, que es justo lo que deja una clonación o una síntesis
/// interrumpida.
#[test]
fn orphan_scratch_of_a_dead_owner_is_swept() {
    let sandbox = Sandbox::new("temp");
    // Un PID que el sistema no puede asignar: su dueño está muerto.
    let temp = sandbox.scratch_root.join("4294967000-1");
    write(&temp.join("avi_clone_x_1.qvoice"), "audio");
    let foreign = sandbox.scratch_root.join("lifecycle-test-foreign.txt");
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
        "el scratch de un dueño muerto se barre: {:?}",
        outcome.swept
    );
    assert!(
        outcome.swept.contains(&temp.display().to_string()),
        "y se informa como barrido: {:?}",
        outcome.swept
    );
    assert!(exists(&foreign), "lo ajeno al producto no se toca");
}

/// `self uninstall` deja residuo cero en las raíces de propiedad exclusiva y termina
/// con éxito.
#[test]
fn uninstall_leaves_no_residue_in_exclusive_roots() {
    let sandbox = Sandbox::new("uninstall");
    let receipt = sandbox.install_registered(PathIntegration::none());
    sandbox.seed_state();

    let outcome = runtime()
        .block_on(uninstall::run(
            &sandbox.env_uninstall(Some(&receipt), Channel::Script),
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
    assert!(
        !exists(&avi_lifecycle::lock::lock_path_for(&sandbox.program_dir)),
        "el archivo de bloqueo no queda"
    );
    assert!(
        !exists(&avi_lifecycle::lock::gate_path_for(&sandbox.program_dir)),
        "la compuerta no queda"
    );
    assert!(!exists(&sandbox.models_dir), "modelos: residuo cero");
    assert!(
        !exists(&sandbox.data_dir),
        "datos: residuo cero, voces de fábrica incluidas: el programa ya no está para \
         re-materializarlas"
    );
}

/// Removedor diferido que anota el destino que se le pide programar, sin borrar nada.
struct DeferredRecording(std::sync::Mutex<Option<PathBuf>>);

impl uninstall::ProgramDirRemover for DeferredRecording {
    fn exe_lives_inside(&self, _program_dir: &Path) -> anyhow::Result<bool> {
        Ok(true)
    }
    fn remove_now(&self, _program_dir: &Path) -> anyhow::Result<()> {
        anyhow::bail!("remove_now no debe llamarse con el ejecutable dentro")
    }
    fn schedule(&self, target: &Path, _wait: &avi_process::OwnerToken) -> anyhow::Result<()> {
        *self.0.lock().expect("se anota el destino") = Some(target.to_path_buf());
        Ok(())
    }
}

/// Desinstala con el directorio de programa dentro de `container` (un contenedor del
/// árbol de propiedad) y devuelve el sandbox tras la operación.
fn uninstall_with_program_in(tag: &str, container: impl Fn(&Sandbox) -> PathBuf) -> Sandbox {
    let mut sandbox = Sandbox::new(tag);
    sandbox.program_dir = container(&sandbox).join("bin");
    let receipt = sandbox.install_registered(PathIntegration::none());
    sandbox.seed_state();
    let outcome = runtime()
        .block_on(uninstall::run(
            &sandbox.env_uninstall(Some(&receipt), Channel::Script),
            &UninstallOptions {
                assume_yes: true,
                ..Default::default()
            },
            &Now,
            &Inert,
        ))
        .expect("uninstall se ejecuta");
    assert!(
        outcome.failed.is_empty(),
        "nada falló: {:?}",
        outcome.failed
    );
    assert!(outcome.program_dir_removed);
    sandbox
}

/// Con el directorio de programa dentro del contenedor raíz de la aplicación (p. ej.
/// `AVI_INSTALL_DIR` personalizado), la desinstalación no deja el contenedor: se poda
/// una vez retirados el programa y sus archivos de bloqueo.
#[test]
fn uninstall_prunes_app_container_holding_the_program() {
    let sandbox = uninstall_with_program_in("program-in-app", |s| s.app_dir.clone());
    assert!(!exists(&sandbox.program_dir));
    assert!(
        !exists(&sandbox.app_dir),
        "el contenedor raíz no queda como residuo"
    );
}

/// Igual con el programa dentro del contenedor de caché: se podan él y su ancestro.
#[test]
fn uninstall_prunes_cache_container_holding_the_program() {
    let sandbox = uninstall_with_program_in("program-in-cache", |s| s.cache_dir.clone());
    assert!(
        !exists(&sandbox.cache_dir),
        "el contenedor de caché no queda"
    );
    assert!(!exists(&sandbox.app_dir), "ni el contenedor raíz");
}

/// Una entrada ajena junto al programa conserva el contenedor: la poda nunca es
/// recursiva sobre lo que no es de la aplicación.
#[test]
fn uninstall_keeps_container_with_foreign_entry_beside_the_program() {
    let mut sandbox = Sandbox::new("program-foreign");
    sandbox.program_dir = sandbox.app_dir.join("bin");
    let receipt = sandbox.install_registered(PathIntegration::none());
    sandbox.seed_state();
    let foreign = sandbox.app_dir.join("ajeno.txt");
    write(&foreign, "ajeno");
    runtime()
        .block_on(uninstall::run(
            &sandbox.env_uninstall(Some(&receipt), Channel::Script),
            &UninstallOptions {
                assume_yes: true,
                ..Default::default()
            },
            &Now,
            &Inert,
        ))
        .expect("uninstall se ejecuta");
    assert!(exists(&foreign), "lo ajeno sobrevive");
    assert!(!exists(&sandbox.program_dir));
}

/// Con el borrado diferido, el destino programado es el contenedor más externo que
/// queda solo con el programa, y no el programa: así el contenedor no es residuo.
#[test]
fn deferred_removal_targets_the_outermost_container_left_with_the_program() {
    let mut sandbox = Sandbox::new("program-deferred");
    sandbox.program_dir = sandbox.cache_dir.join("bin");
    let receipt = sandbox.install_registered(PathIntegration::none());
    sandbox.seed_state();
    let remover = DeferredRecording(std::sync::Mutex::new(None));
    let outcome = runtime()
        .block_on(uninstall::run(
            &sandbox.env_uninstall(Some(&receipt), Channel::Script),
            &UninstallOptions {
                assume_yes: true,
                ..Default::default()
            },
            &remover,
            &Inert,
        ))
        .expect("uninstall se ejecuta");
    assert_eq!(outcome.status, "removal_scheduled");
    assert_eq!(
        remover.0.lock().unwrap().as_deref(),
        Some(sandbox.app_dir.as_path())
    );
}

/// Con un comando de uso en curso (bloqueo compartido tomado), `cleanup` y
/// `self uninstall` terminan con `lifecycle_locked` sin borrar nada.
#[test]
fn destructive_operations_refuse_while_a_usage_command_runs() {
    let sandbox = Sandbox::new("usage-lock");
    let receipt = sandbox.install_registered(PathIntegration::none());
    sandbox.seed_state();
    let usage = avi_lifecycle::lock::acquire_shared(&sandbox.program_dir)
        .expect("el comando de uso toma el bloqueo compartido");
    let before = sandbox.snapshot();

    let error = runtime()
        .block_on(cleanup::run(
            &sandbox.roots(),
            &with_yes(CleanupOptions {
                all: true,
                ..Default::default()
            }),
            &Inert,
        ))
        .expect_err("cleanup no corre con un uso en curso");
    let failure = error
        .downcast_ref::<avi_lifecycle::LifecycleError>()
        .expect("el fallo declara un `reason`");
    assert_eq!(failure.reason, "lifecycle_locked");

    let error = runtime()
        .block_on(uninstall::run(
            &sandbox.env_uninstall(Some(&receipt), Channel::Script),
            &UninstallOptions {
                assume_yes: true,
                ..Default::default()
            },
            &Now,
            &Inert,
        ))
        .expect_err("uninstall no corre con un uso en curso");
    let failure = error
        .downcast_ref::<avi_lifecycle::LifecycleError>()
        .expect("el fallo declara un `reason`");
    assert_eq!(failure.reason, "lifecycle_locked");

    assert_eq!(sandbox.snapshot(), before, "no se borró nada");
    drop(usage);
}

/// `--keep-data` conserva modelos, voces y habla.
#[test]
fn uninstall_keep_data_preserves_models_voices_and_speech() {
    let sandbox = Sandbox::new("keep-data");
    let receipt = sandbox.install_registered(PathIntegration::none());
    sandbox.seed_state();
    let voices = sandbox.data_dir.join("voices");

    let outcome = runtime()
        .block_on(uninstall::run(
            &sandbox.env_uninstall(Some(&receipt), Channel::Script),
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
    assert!(exists(&voices.join("mine")), "las voces se quedan");
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
}

/// Repetido sobre un sistema limpio termina con éxito y `not_installed`.
#[test]
fn uninstall_is_idempotent() {
    let sandbox = Sandbox::new("idempotent");
    let receipt = sandbox.install_registered(PathIntegration::none());
    sandbox.seed_state();
    let runtime = runtime();

    let first = runtime
        .block_on(uninstall::run(
            &sandbox.env_uninstall(Some(&receipt), Channel::Script),
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
            &sandbox.env_uninstall(None, Channel::Unmanaged),
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
}

/// El canal `homebrew` es `externally_managed` con el comando correcto, y nada se toca.
#[test]
fn uninstall_homebrew_is_externally_managed() {
    let sandbox = Sandbox::new("homebrew");
    let receipt = sandbox.install_registered(PathIntegration::none());
    sandbox.seed_state();
    let before = sandbox.snapshot();

    let error = runtime()
        .block_on(uninstall::run(
            &sandbox.env_uninstall(Some(&receipt), Channel::Homebrew),
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
    assert_eq!(ExitCode::from_reason(lifecycle.reason).code(), 12);
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
}

/// El directorio de programa no se borra si no contiene recibo ni ejecutable, ni si es
/// una raíz del sistema, `$HOME` o un ancestro de `$HOME`. Ni una variable de
/// reubicación ni un recibo manipulado pueden ampliar el alcance.
#[test]
fn uninstall_refuses_unsafe_program_dir() {
    let sandbox = Sandbox::new("program-dir-rule");
    let roots = sandbox.roots();
    let exe = uninstall::executable_name_default();
    let receipt = sandbox.program_dir.join("install-receipt.json");

    // Con recibo: se puede borrar.
    write(&receipt, "{}");
    assert!(
        uninstall::program_dir_is_removable(&roots, &sandbox.program_dir),
        "con recibo, sí"
    );
    std::fs::remove_file(&receipt).expect("se retira el recibo de la premisa");
    // Con ejecutable: se puede borrar.
    write(&sandbox.program_dir.join(&exe), "binario");
    assert!(
        uninstall::program_dir_is_removable(&roots, &sandbox.program_dir),
        "con ejecutable, también"
    );
    // Sin ninguno de los dos: no.
    std::fs::remove_file(sandbox.program_dir.join(&exe))
        .expect("se retira el ejecutable de la premisa");
    assert!(
        !uninstall::program_dir_is_removable(&roots, &sandbox.program_dir),
        "sin recibo ni ejecutable, no: no es el directorio de programa"
    );

    // Con recibo, pero en una raíz prohibida: tampoco. La raíz del sistema se prueba
    // sin escribir nada en ella —`program_dir_is_removable` rechaza por estructura,
    // antes de mirar el recibo— y las demás se prueban con el recibo escrito, que es
    // lo que un recibo manipulado intentaría.
    for protected in [
        sandbox.home.clone(),
        sandbox.home.parent().unwrap().to_path_buf(),
        sandbox.data_dir.clone(),
        sandbox.models_dir.clone(),
        sandbox.scratch_root.clone(),
    ] {
        write(&protected.join("install-receipt.json"), "{}");
        assert!(
            !uninstall::program_dir_is_removable(&roots, &protected),
            "no se puede borrar {} como directorio de programa, aunque tenga recibo",
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
}

/// En Windows, con el ejecutable en uso dentro del directorio de programa, el borrado se
/// programa y el desenlace es `removal_scheduled`, que es un éxito.
#[test]
fn uninstall_schedules_removal_when_the_executable_is_inside() {
    let sandbox = Sandbox::new("deferred");
    let receipt = sandbox.install_registered(PathIntegration::none());
    sandbox.seed_state();

    let outcome = runtime()
        .block_on(uninstall::run(
            &sandbox.env_uninstall(Some(&receipt), Channel::Script),
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
        sandbox.program_dir.join("scheduled-removal").is_file(),
        "el borrado quedó programado con el PID del proceso"
    );
    // El estado sí se borró: el programa es lo único que espera.
    assert!(!exists(&sandbox.data_dir));
}

/// Si el borrado del directorio de programa no se puede programar, el desenlace es
/// `program_dir_kept` con el código 22 y nunca un éxito limpio: el resto de la
/// desinstalación se completó y el directorio queda intacto, sin borrado parcial.
#[test]
fn uninstall_reports_program_dir_kept_when_removal_cannot_be_scheduled() {
    let sandbox = Sandbox::new("unschedulable");
    let receipt = sandbox.install_registered(PathIntegration::none());
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
            &sandbox.env_uninstall(Some(&receipt), Channel::Script),
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
    assert_eq!(ExitCode::from_reason(error.reason).code(), 22);
    assert_eq!(
        names_of(&sandbox.program_dir),
        before,
        "el directorio de programa sigue intacto"
    );
    // El estado de usuario sí se borró: solo el programa quedó en disco.
    assert!(!exists(&sandbox.data_dir));
}

/// Si no puede determinarse si el ejecutable está dentro del directorio de programa,
/// el error no se convierte en «fuera»: el directorio se conserva como `program_dir_kept`
/// con la causa en el mensaje y sin borrado parcial.
#[test]
fn uninstall_reports_program_dir_kept_when_exe_location_is_undeterminable() {
    struct Undeterminable;
    impl uninstall::ProgramDirRemover for Undeterminable {
        fn exe_lives_inside(&self, _program_dir: &Path) -> anyhow::Result<bool> {
            anyhow::bail!("no se resolvió la ruta del ejecutable")
        }
        fn remove_now(&self, _program_dir: &Path) -> anyhow::Result<()> {
            anyhow::bail!("remove_now no debe llamarse sin saber dónde está el ejecutable")
        }
        fn schedule(
            &self,
            _program_dir: &Path,
            _wait: &avi_process::OwnerToken,
        ) -> anyhow::Result<()> {
            anyhow::bail!("schedule no debe llamarse sin saber dónde está el ejecutable")
        }
    }
    let sandbox = Sandbox::new("exe-undeterminable");
    let receipt = sandbox.install_registered(PathIntegration::none());
    sandbox.seed_state();

    let outcome = runtime()
        .block_on(uninstall::run(
            &sandbox.env_uninstall(Some(&receipt), Channel::Script),
            &UninstallOptions {
                assume_yes: true,
                ..Default::default()
            },
            &Undeterminable,
            &Inert,
        ))
        .expect("uninstall se ejecuta");

    assert!(!outcome.program_dir_removed);
    let error = outcome
        .lifecycle_error()
        .expect("hay un reason de contrato");
    assert_eq!(error.reason, "program_dir_kept");
    assert!(
        error
            .message
            .contains("no se resolvió la ruta del ejecutable"),
        "el mensaje conserva la causa: {}",
        error.message
    );
    assert!(sandbox.program_dir.exists(), "el directorio sigue ahí");
}

/// `--dry-run` de `self uninstall` imprime el plan y no modifica el disco.
#[test]
fn uninstall_dry_run_touches_nothing() {
    let sandbox = Sandbox::new("uninstall-dry");
    let receipt = sandbox.install_registered(PathIntegration::none());
    sandbox.seed_state();
    let before = sandbox.snapshot();

    let outcome = runtime()
        .block_on(uninstall::run(
            &sandbox.env_uninstall(Some(&receipt), Channel::Script),
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
}

/// La reversión del `PATH` sale del recibo, no de donde esté el ejecutable (paso 7
/// de la desinstalación). En Unix: el enlace, solo si apunta al directorio de programa, y
/// los bloques
/// delimitados de los perfiles.
#[cfg(unix)]
#[test]
fn uninstall_reverts_path_from_the_receipt() {
    use avi_lifecycle::receipt::{self, InstallReceipt};
    use std::os::unix::fs::symlink;

    let sandbox = Sandbox::new("path-unix");
    let bin_dir = sandbox.home.join(".local").join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let link = bin_dir.join(avi_lifecycle::APP_NAME);
    let program_exe = sandbox
        .program_dir
        .join(uninstall::executable_name_default());
    sandbox.install_registered(PathIntegration::none());
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
            cache_dir: sandbox.cache_dir.clone(),
        },
    );
    receipt::write_to(&receipt, &sandbox.program_dir).unwrap();

    let outcome = runtime()
        .block_on(uninstall::run(
            &sandbox.env_uninstall(Some(&receipt), Channel::Script),
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
    let other = sandbox.root.join("other-install");
    write(&other.join(uninstall::executable_name_default()), "binario");
    let foreign_link = bin_dir.join("foreign");
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
            cache_dir: sandbox.cache_dir.clone(),
        },
    );
    let revert_report = uninstall::revert_path(Some(&foreign_receipt), &sandbox.home);
    assert!(
        !revert_report.touched,
        "un enlace que no apunta al programa no se toca"
    );
    assert!(
        revert_report.failures.is_empty(),
        "{:?}",
        revert_report.failures
    );
    assert!(
        foreign_link.is_symlink(),
        "el enlace ajeno sigue siendo un enlace"
    );
    assert_eq!(
        std::fs::read_link(&foreign_link).expect("se lee el destino del enlace ajeno"),
        other.join(uninstall::executable_name_default()),
        "y sigue apuntando a su destino"
    );
}

// ─── Árbol de propiedad: poda de contenedores ───────────────────────────────────────

/// Desinstala con `--yes` y, si procede, `--keep-data`.
fn run_uninstall(
    sandbox: &Sandbox,
    receipt: &avi_lifecycle::receipt::InstallReceipt,
    keep_data: bool,
) -> uninstall::Outcome {
    runtime()
        .block_on(uninstall::run(
            &sandbox.env_uninstall(Some(receipt), Channel::Script),
            &UninstallOptions {
                keep_data,
                assume_yes: true,
                ..Default::default()
            },
            &Now,
            &Inert,
        ))
        .expect("uninstall se ejecuta")
}

/// Ninguna ruta aparece dos veces entre lo que el plan conserva: cada aviso se imprime
/// una sola vez porque el plan lo lleva una sola vez.
fn assert_preserved_unique(preserved: &[cleanup::Preserved]) {
    let mut seen = std::collections::BTreeSet::new();
    for item in preserved {
        assert!(
            seen.insert(item.path.clone()),
            "{} aparece más de una vez entre lo conservado: {preserved:?}",
            item.path.display()
        );
    }
}

/// Tras `self uninstall` desaparecen los contenedores raíz y de caché, con su scratch.
#[test]
fn uninstall_removes_every_container() {
    let sandbox = Sandbox::new("containers-uninstall");
    let receipt = sandbox.install_registered(PathIntegration::none());
    sandbox.seed_state();
    write(
        &sandbox.scratch_root.join("4294967000-1").join("clip.wav"),
        "x",
    );

    let outcome = run_uninstall(&sandbox, &receipt, false);

    assert!(outcome.failed.is_empty(), "{:?}", outcome.failed);
    assert!(
        !exists(&sandbox.cache_dir),
        "el contenedor de caché no queda"
    );
    assert!(!exists(&sandbox.app_dir), "el contenedor raíz no queda");
    for container in [&sandbox.cache_dir, &sandbox.app_dir] {
        assert!(
            outcome.removed.contains(&container.display().to_string()),
            "{} se publica como borrado: {:?}",
            container.display(),
            outcome.removed
        );
    }
    assert!(
        exists(sandbox.root.as_path()),
        "el padre común de los contenedores no es un nodo y no se poda"
    );
}

/// `cleanup --all` poda el contenedor de caché; el de datos sigue porque la hoja de datos
/// no se borra entera (conserva las voces de fábrica).
#[test]
fn cleanup_all_removes_the_cache_container() {
    let sandbox = Sandbox::new("containers-cleanup");
    sandbox.install_registered(PathIntegration::none());
    sandbox.seed_state();
    write(
        &sandbox.scratch_root.join("4294967000-1").join("clip.wav"),
        "x",
    );

    let outcome = runtime()
        .block_on(cleanup::run(
            &sandbox.roots(),
            &with_yes(CleanupOptions {
                all: true,
                ..Default::default()
            }),
            &Inert,
        ))
        .expect("cleanup se ejecuta");

    assert!(outcome.failed.is_empty(), "{:?}", outcome.failed);
    assert!(
        !exists(&sandbox.cache_dir),
        "caché, modelos y scratch fuera"
    );
    assert!(
        exists(&sandbox.data_dir.join("voices").join("default")),
        "las voces de fábrica sobreviven a `cleanup`"
    );
    assert!(exists(&sandbox.app_dir), "y con ellas el contenedor raíz");
}

/// Un archivo ajeno dentro de un contenedor lo conserva en pie, se informa una vez y
/// jamás se borra; el contenedor padre no se informa porque lo retiene un nodo.
#[test]
fn foreign_file_keeps_its_container_and_is_reported() {
    let sandbox = Sandbox::new("containers-foreign");
    let receipt = sandbox.install_registered(PathIntegration::none());
    sandbox.seed_state();
    let stranger = sandbox.cache_dir.join("ajeno.txt");
    write(&stranger, "no es nuestro");

    let options = UninstallOptions {
        assume_yes: true,
        ..Default::default()
    };
    let plan = uninstall::compose_plan(
        &sandbox.roots(),
        Some(&receipt),
        &sandbox.program_dir,
        &options,
    );
    assert_preserved_unique(&plan.preserved);
    assert!(
        plan.preserved.iter().any(|p| p.path == sandbox.cache_dir),
        "el contenedor con entradas ajenas se informa: {:?}",
        plan.preserved
    );
    assert!(
        !plan.preserved.iter().any(|p| p.path == sandbox.app_dir),
        "el ancestro lo retiene un nodo y no se informa: {:?}",
        plan.preserved
    );

    let outcome = run_uninstall(&sandbox, &receipt, false);

    assert!(outcome.failed.is_empty(), "{:?}", outcome.failed);
    assert!(exists(&stranger), "lo ajeno jamás se borra");
    assert!(exists(&sandbox.cache_dir) && exists(&sandbox.app_dir));
    assert!(!exists(&sandbox.models_dir) && !exists(&sandbox.data_dir));
    assert!(
        outcome
            .preserved
            .iter()
            .any(|p| p.path == sandbox.cache_dir),
        "y el desenlace conserva el aviso del plan: {:?}",
        outcome.preserved
    );
}

/// La raíz HF compartida no se poda ni se borra entera: ni con `self uninstall` ni con
/// `cleanup --all`. Los contenedores de la aplicación sí desaparecen.
#[test]
fn shared_hf_root_is_never_pruned() {
    for uninstalling in [true, false] {
        let sandbox = Sandbox::new_shared(if uninstalling {
            "containers-shared-uninstall"
        } else {
            "containers-shared-cleanup"
        });
        let receipt = sandbox.install_registered(PathIntegration::none());
        sandbox.seed_state();
        let hub = sandbox.shared_hub.clone().expect("modo compartido");
        let foreign = hub.join("models--other--tool");

        if uninstalling {
            let plan = uninstall::compose_plan(
                &sandbox.roots(),
                Some(&receipt),
                &sandbox.program_dir,
                &UninstallOptions::default(),
            );
            assert_preserved_unique(&plan.preserved);
            let outcome = run_uninstall(&sandbox, &receipt, false);
            assert!(outcome.failed.is_empty(), "{:?}", outcome.failed);
            assert!(!exists(&sandbox.app_dir), "los contenedores propios caen");
        } else {
            let outcome = runtime()
                .block_on(cleanup::run(
                    &sandbox.roots(),
                    &with_yes(CleanupOptions {
                        all: true,
                        ..Default::default()
                    }),
                    &Inert,
                ))
                .expect("cleanup se ejecuta");
            assert!(outcome.failed.is_empty(), "{:?}", outcome.failed);
            assert!(!exists(&sandbox.cache_dir), "el contenedor de caché cae");
        }
        assert!(exists(&hub), "la raíz compartida no se borra entera");
        assert!(exists(&hub.join("xet")), "`xet` sobrevive");
        assert!(exists(&foreign), "lo de otra herramienta sobrevive");
        assert!(
            !exists(&hub.join("models--CristianRojaas--opus-mt-es-en-ct2-int8")),
            "el repo propio sí se borra"
        );
    }
}

/// `--keep-data` no poda los contenedores que retienen hojas conservadas, no los
/// informa, y ningún aviso se repite.
#[test]
fn keep_data_leaves_containers_in_place_silently() {
    let sandbox = Sandbox::new("containers-keep-data");
    let receipt = sandbox.install_registered(PathIntegration::none());
    sandbox.seed_state();
    write(
        &sandbox.scratch_root.join("4294967000-1").join("clip.wav"),
        "x",
    );

    let plan = uninstall::compose_plan(
        &sandbox.roots(),
        Some(&receipt),
        &sandbox.program_dir,
        &UninstallOptions {
            keep_data: true,
            ..Default::default()
        },
    );
    assert_preserved_unique(&plan.preserved);
    assert!(
        !plan
            .preserved
            .iter()
            .any(|p| p.path == sandbox.cache_dir || p.path == sandbox.app_dir),
        "los contenedores retenidos por hojas conservadas no se informan: {:?}",
        plan.preserved
    );
    assert!(plan.state.ownership.prune.is_empty());

    let outcome = run_uninstall(&sandbox, &receipt, true);

    assert!(outcome.failed.is_empty(), "{:?}", outcome.failed);
    assert!(exists(&sandbox.app_dir) && exists(&sandbox.cache_dir));
    assert!(exists(&sandbox.models_dir) && exists(&sandbox.data_dir.join("voices")));
    assert!(!exists(&sandbox.scratch_root), "los temporales sí caen");
}
