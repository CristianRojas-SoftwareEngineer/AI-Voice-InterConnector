// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! Interrupción de la transacción y recuperación en la siguiente operación.
//!
//! La garantía que se demuestra es que una
//! interrupción en cualquier punto **deja la versión anterior operativa** y la siguiente
//! operación de ciclo de vida **completa la recuperación**, sin residuo.
//!
//! Hay dos capas, y la distinción no es de estilo sino de qué se puede comprobar en esta
//! puerta:
//!
//! 1. `interrupted_transaction_is_recovered_next_run` escribe, con la API pública de
//!    `transaction`, el estado **exacto** en que una operación deja el directorio de
//!    programa al morir en cada uno de los cinco estados del diario, y comprueba que la
//!    recuperación lo resuelve como la regla dice: los cuatro estados sin confirmar se
//!    revierten y devuelven la versión anterior; el confirmado se completa como commit.
//!    Luego, en un sandbox nuevo por estado, comprueba que una **operación real** —
//!    `self install`— completa la recuperación por su cuenta y no deja residuo.
//! 2. `interrupted_install_recovers_next_run` provoca la interrupción **de verdad**,
//!    armando los puntos de inyección del feature `faults`, y comprueba el desenlace real
//!    de `self install` en cada punto.
//!
//! La segunda capa solo existe compilada con el feature `faults`, y no es un `skip`: es
//! una función que **no existe** sin él. `cargo test --all` lo activa —la dev-dependency
//! del paquete raíz lo declara—, mientras que `cargo test -p avi-lifecycle` no lo hace, y
//! por eso la primera capa no lo necesita: demuestra la recuperación sin él.

#![allow(clippy::disallowed_methods)]

#[path = "support/bundle.rs"]
mod bundle;
#[path = "support/inert.rs"]
mod inert;
#[path = "support/inspect.rs"]
mod inspect;
#[path = "support/install_env.rs"]
mod install_env;
#[path = "support/listing.rs"]
mod listing;
#[path = "support/net.rs"]
mod net;
#[path = "support/process_env.rs"]
mod process_env;
#[path = "support/registered.rs"]
mod registered;
#[path = "support/repo.rs"]
mod repo;
#[path = "support/runtime.rs"]
mod runtime;
mod support;

use avi_lifecycle::channel::Channel;
use avi_lifecycle::install;
use avi_lifecycle::receipt::{self, InstallReceipt, PathIntegration};
use avi_lifecycle::recovery::{self, RecoveryOutcome};
use avi_lifecycle::transaction::{Journal, JournalState, JOURNAL_SCHEMA_VERSION};
use avi_lifecycle::uninstall;
use inert::Inert;
use registered::Now;
use std::path::{Path, PathBuf};
use support::Sandbox;

/// Los cinco estados del diario de la transacción: el punto del flujo en el que la
/// operación
/// muere, si ese estado se revierte o se completa como commit, y un nombre estable para
/// el sandbox y los mensajes.
const POINTS: [(&str, &str, JournalState, bool); 5] = [
    ("start", "antes de aparcar", JournalState::Started, false),
    ("parked", "después de aparcar", JournalState::Parked, false),
    ("placed", "después de colocar", JournalState::Placed, false),
    (
        "permissions",
        "después de los permisos",
        JournalState::Permissions,
        false,
    ),
    (
        "committed",
        "después de confirmar",
        JournalState::Committed,
        true,
    ),
];

/// Versión anterior, la que se aparca al empezar la operación interrumpida.
const PREVIOUS_VERSION: &str = "0.23.1";

/// Recibo de la versión anterior, con la forma que escribe el instalador.
fn previous_receipt(sandbox: &Sandbox) -> InstallReceipt {
    InstallReceipt::new(
        PREVIOUS_VERSION,
        avi_lifecycle::target::host_triple(),
        Channel::Script,
        &sandbox.program_dir,
        vec![uninstall::executable_name_default()],
        PathIntegration::none(),
        receipt::Roots {
            data_dir: sandbox.data_dir.clone(),
            cache_dir: sandbox.cache_dir.clone(),
        },
    )
}

/// Instala la versión anterior en el directorio de programa, sin pasar por `self install`:
/// lo que se aparca en una operación real es un árbol ya instalado con su recibo.
fn seed_previous_version(sandbox: &Sandbox) {
    support::write(
        &sandbox
            .program_dir
            .join(uninstall::executable_name_default()),
        "binario de la versión anterior\n",
    );
    support::write(&sandbox.program_dir.join("LICENSE"), "licencia anterior\n");
    let receipt = previous_receipt(sandbox);
    receipt::write_to(&receipt, &sandbox.program_dir).expect("se escribe el recibo anterior");
}

/// Estado en el que una operación deja el directorio de programa al morir en `estado`.
///
/// Se construye con la API pública de `transaction` —el diario es un dato serializable y
/// la recuperación define su esquema— replicando el orden real del algoritmo: escribir el
/// diario, aparcar
/// por renombrado, y colocar moviendo desde el origen. Devuelve la ruta del aparcado, que
/// lleva el token de este proceso como lo haría la operación que murió.
fn interrupt_at(sandbox: &Sandbox, state: JournalState) -> PathBuf {
    let owner = avi_process::current_owner_token().expect("este proceso se puede observar");
    let parked = sandbox
        .program_dir
        .join(avi_shared::staging_names::parked::name(&owner, 18));
    let mut journal = Journal {
        schema_version: JOURNAL_SCHEMA_VERSION,
        txid: format!("{}-18", owner.format()),
        state: JournalState::Started,
        parked_dir: None,
        placed: Vec::new(),
        source_dir: Some(sandbox.staging().clone()),
    };

    // El bundle nuevo está en el staging, que es donde lo deja el bootstrap, y la
    // versión anterior está en el directorio de programa, que es lo que hay.
    sandbox.write_bundle(&sandbox.staging());
    seed_previous_version(sandbox);

    // El diario se escribe **antes** de tocar nada, y el aparcado se salta el
    // diario, igual que hace el motor.
    write_journal(&sandbox.program_dir, &journal);
    if state == JournalState::Started {
        return parked;
    }

    // 1. Aparcar por renombrado.
    std::fs::create_dir_all(&parked).expect("se crea el aparcado");
    for name in listing::list(&sandbox.program_dir) {
        if name.ends_with('/') || name.starts_with(avi_lifecycle::PARKED_DIR_PREFIX) {
            continue;
        }
        std::fs::rename(sandbox.program_dir.join(&name), parked.join(&name))
            .expect("se aparca el contenido anterior");
    }
    journal.parked_dir = Some(parked.clone());
    journal.state = JournalState::Parked;
    write_journal(&sandbox.program_dir, &journal);
    if state == JournalState::Parked {
        return parked;
    }

    // 2. Colocar. El diario declara qué se va a colocar **antes** de colocar nada, que es
    //    lo que permite a una interrupción a mitad del paso saber qué retirar.
    let mut placed = listing::list(&sandbox.staging());
    placed.retain(|n| !n.ends_with('/'));
    journal.placed = placed.clone();
    journal.state = state;
    write_journal(&sandbox.program_dir, &journal);
    for relative in &placed {
        let from = sandbox.staging().join(relative);
        let dest = sandbox.program_dir.join(relative);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).expect("se crea el directorio del archivo colocado");
        }
        std::fs::rename(&from, &dest).expect("se coloca el archivo del bundle");
    }
    parked
}

/// Escribe el diario como lo haría la operación: el motor usa temporal hermano y
/// renombrado, y desde fuera el resultado es el mismo fichero.
fn write_journal(program_dir: &Path, journal: &Journal) {
    std::fs::write(
        avi_lifecycle::transaction::journal_path(program_dir),
        serde_json::to_string(journal).expect("el diario es serializable"),
    )
    .expect("se escribe el diario");
}

/// Raíces de la recuperación del sandbox. El staging de la operación lleva el token de
/// este proceso, vivo, así que el barrido no lo toca: barrerse a sí mismo entre el paso 1 y
/// el paso 2 dejaría a `self install` instalando un bundle vacío.
fn roots(sandbox: &Sandbox) -> recovery::Roots<'_> {
    recovery::Roots {
        program_dir: &sandbox.program_dir,
        scratch_root: &sandbox.scratch_root,
        data_dir: &sandbox.data_dir,
        bin_dir: &sandbox.bin_dir,
    }
}

/// Residuo de una operación de ciclo de vida: aparcados y scratch. El staging no se
/// cuenta, porque durante una operación es suyo y no huérfano, y porque borrarlo es el
/// paso 10 del bootstrap, que corresponde al bootstrap y no al motor.
fn residue(sandbox: &Sandbox) -> Vec<String> {
    let mut out: Vec<String> = listing::list(&sandbox.program_dir)
        .into_iter()
        .filter(|n| n.starts_with(avi_lifecycle::PARKED_DIR_PREFIX))
        .collect();
    out.extend(listing::list(&sandbox.scratch_root));
    out.sort();
    out
}

/// `true` si el directorio de programa tiene el ejecutable y un recibo de la versión
/// anterior, es decir, si la versión anterior es la operativa.
fn operational_previous_version(sandbox: &Sandbox) -> bool {
    sandbox
        .program_dir
        .join(uninstall::executable_name_default())
        .is_file()
        && receipt::read_from(&sandbox.program_dir)
            .ok()
            .flatten()
            .is_some_and(|r| r.version == PREVIOUS_VERSION)
}

/// Estado del sandbox sin el archivo de bloqueo, que la recuperación crea al tomar el
/// bloqueo —el
/// paso 1, antes de la recuperación— y que no es «nada modificado» sino el mecanismo que
/// serializa las operaciones.
fn state_without_lock(sandbox: &Sandbox) -> Vec<(String, u64)> {
    let mut out: Vec<(String, u64)> = sandbox
        .snapshot()
        .into_iter()
        .filter(|(path, _)| {
            ![
                avi_lifecycle::LIFECYCLE_LOCK_NAME,
                avi_lifecycle::LIFECYCLE_GATE_NAME,
            ]
            .contains(&path.rsplit('/').next().unwrap_or_default())
        })
        .collect();
    out.sort();
    out
}

/// **La recuperación**, en cada punto del algoritmo de la transacción.
///
/// Dos afirmaciones por punto. La primera es sobre la **recuperación**, a la que se le
/// escribe el estado: los cuatro estados sin confirmar se revierten y devuelven la versión
/// anterior al directorio de programa —con su ejecutable y su recibo—, el confirmado se
/// completa como commit y conserva lo nuevo, y en todos los casos no queda diario ni
/// aparcado. La segunda es sobre la **operación siguiente**: en un sandbox nuevo con el
/// mismo estado escrito, `self install` completa la recuperación por su cuenta, instala y
/// no deja residuo. Sin la segunda, la primera solo probaría una función.
#[test]
fn interrupted_transaction_is_recovered_next_run() {
    let _guard = process_env::exclusively();
    let runtime = runtime::runtime();

    for (slug, point, state, commit) in POINTS {
        // ── Capa 1: la recuperación resuelve el estado escrito ───────────────────
        let sandbox = Sandbox::new(&format!("recovery-{slug}"));
        sandbox.seed_env();
        let parked = interrupt_at(&sandbox, state);
        assert!(
            avi_lifecycle::transaction::read_journal(&sandbox.program_dir)
                .expect("se lee el diario")
                .is_some(),
            "punto «{point}»: hay transacción pendiente"
        );
        if commit {
            assert!(
                parked.is_dir(),
                "punto «{point}»: el aparcado sigue, porque el commit aún no se \
                 ha completado"
            );
        } else if state == JournalState::Started {
            assert!(
                operational_previous_version(&sandbox),
                "punto «{point}»: la versión anterior nunca llegó a moverse"
            );
        } else {
            assert!(
                !operational_previous_version(&sandbox),
                "punto «{point}»: antes de recuperar, la versión anterior no está \
                 en su sitio: eso es lo que la reversión tiene que arreglar"
            );
        }

        let outcome: RecoveryOutcome =
            recovery::recover(roots(&sandbox)).expect("la recuperación se ejecuta");

        assert_eq!(
            outcome.rolled_back, !commit,
            "punto «{point}»: el estado del diario decide, y solo el confirmado se \
             completa como commit"
        );
        assert_eq!(
            outcome.committed, commit,
            "punto «{point}»: y no se hacen las dos cosas"
        );
        assert!(
            outcome.is_clean(),
            "punto «{point}»: no queda nada pendiente: {:?}",
            outcome.kept
        );
        assert!(
            avi_lifecycle::transaction::read_journal(&sandbox.program_dir)
                .expect("se relee el diario")
                .is_none(),
            "punto «{point}»: el diario desaparece"
        );
        assert!(
            !parked.exists(),
            "punto «{point}»: el aparcado desaparece, restaurado o borrado"
        );
        if commit {
            // El commit **conserva** lo colocado y borra el aparcado. No hay recibo todavía:
            // el recibo se escribe después de la transacción, así que en este
            // punto la instalación está colocada pero sin registrar. Lo que se afirma es que
            // el bundle entero está en su sitio, que es lo que distingue un commit de un
            // rollback a medias.
            let placed = listing::list(&sandbox.program_dir);
            let section =
                avi_lifecycle::manifest::target_section(avi_lifecycle::target::host_triple())
                    .expect("el target del host tiene sección en el manifiesto");
            for relative in &section.required {
                assert!(
                    placed.contains(relative),
                    "punto «{point}»: el commit conserva lo colocado y {relative} \
                     está en su sitio: {placed:?}"
                );
            }
            assert!(
                receipt::read_from(&sandbox.program_dir)
                    .expect("se lee el recibo")
                    .is_none(),
                "punto «{point}»: y el recibo todavía no, porque se escribe después \
                 de la transacción"
            );
        } else {
            assert!(
                operational_previous_version(&sandbox),
                "punto «{point}»: la versión anterior vuelve a estar operativa"
            );
            // Lo que puede quedar son los **directorios** que la colocación creó para
            // alojar archivos anidados y que la reversión vacía sin borrar, porque el
            // diario solo declara archivos. No es residuo que estorbe —la siguiente
            // colocación los reutiliza— y la versión anterior está operativa, que es lo
            // que la garantía exige. Se deja escrito para que no se lea como un olvido.
            let placed = listing::list(&sandbox.program_dir);
            let previous_version = [
                "LICENSE".to_string(),
                receipt::RECEIPT_NAME.to_string(),
                uninstall::executable_name_default(),
            ];
            for entry in &placed {
                assert!(
                    entry.ends_with('/') || previous_version.contains(entry),
                    "punto «{point}»: el directorio de programa queda con la \
                     versión anterior y, como mucho, con directorios vacíos, pero aparece \
                     {entry}: {placed:?}"
                );
            }
            let in_source: Vec<String> = listing::list(&sandbox.staging())
                .into_iter()
                .filter(|n| !n.ends_with('/'))
                .collect();
            assert!(
                !in_source.is_empty(),
                "punto «{point}»: lo colocado vuelve al origen, que es de donde \
                 la colocación lo movió: {in_source:?}"
            );
        }
        assert_eq!(
            residue(&sandbox),
            Vec::<String>::new(),
            "punto «{point}»: no queda residuo en las raíces exclusivas"
        );

        // ── Capa 2: la operación siguiente completa la recuperación ───────────────
        let s2 = Sandbox::new(&format!("operation-{slug}"));
        s2.seed_env();
        let _parked = interrupt_at(&s2, state);
        // El bootstrap extrae en un staging nuevo cada vez, así que el bundle se repone
        // antes de la operación. Es también lo que hace que la reversión encuentre el
        // origen ya completo y borre la copia en el programa en vez de devolver el
        // archivo: el ejecutable en ejecución es el caso en que se copió y no se movió.
        let exe = s2.write_bundle(&s2.staging());
        let installed = runtime
            .block_on(install::install(
                &s2.install_env(&exe),
                &Sandbox::install_options(),
                &Inert,
            ))
            .expect("la operación siguiente se completa");
        assert_eq!(
            installed.status, "installed",
            "punto «{point}»: la operación siguiente no tropieza con la \
             interrupción anterior"
        );
        assert_eq!(
            installed.receipt.version, "0.24.0",
            "punto «{point}»: e instala la versión que iba a instalar"
        );
        assert_eq!(
            installed.receipt.files.len(),
            avi_lifecycle::manifest::target_section(&installed.receipt.target)
                .expect("el target del host tiene sección")
                .required
                .len(),
            "punto «{point}»: con el bundle entero, que es lo que distingue una \
             instalación de un bundle a medias"
        );
        assert!(
            avi_lifecycle::transaction::read_journal(&s2.program_dir)
                .expect("se relee el diario")
                .is_none(),
            "punto «{point}»: la operación se llevó el diario"
        );
        assert_eq!(
            residue(&s2),
            Vec::<String>::new(),
            "punto «{point}»: y no deja aparcados ni temporales propios"
        );
    }
}

/// **La interrupción de verdad**, en cada punto de la transacción, con el punto de
/// inyección de fallos. Solo existe compilada con el feature `faults`.
///
/// Los cuatro primeros puntos caen dentro de la transacción, así que `self install` falla
/// con `rolled_back` y la versión anterior queda restaurada: es el mismo desenlace que una
/// interrupción real, sin tener que matar el proceso. Los dos últimos caen **después** de
/// confirmar la transacción, así que no hay reversión que hacer: la operación falla con el
/// fallo inyectado y el directorio de programa se queda con el bundle nuevo colocado. En
/// los seis casos la siguiente operación completa y no deja residuo, que es lo que el
/// garantía de recuperación promete.
///
/// El runtime es de un solo hilo y se conduce con `block_on` en vez de `#[tokio::test]`, y
/// el motivo es el candado: la prueba tiene que sostener `ENV_LOCK` mientras el motor lee
/// `AVI_CACHE_DIR` desde dentro de su propio `await`, y un `MutexGuard` sostenido a través
/// de un punto de espera es exactamente lo que un test asíncrono no debe hacer. Con
/// `block_on` el punto de espera ocurre dentro del runtime ajeno y el candado se sostiene
/// sin cruzarlo.
#[cfg(feature = "faults")]
#[test]
fn interrupted_install_recovers_next_run() {
    use avi_core::exit_codes::ExitCode;
    use avi_lifecycle::faults::{self, FaultPoint};

    let _guard = process_env::exclusively();
    let runtime = runtime::runtime();
    let points = [
        (FaultPoint::BeforePark, true),
        (FaultPoint::BeforePlace, true),
        (FaultPoint::BeforeFixPermissions, true),
        (FaultPoint::BeforeCommit, true),
        (FaultPoint::BeforePathIntegration, false),
        (FaultPoint::BeforeReceipt, false),
    ];

    for (point, inside_transaction) in points {
        let sandbox = Sandbox::new(&format!("injected-{}", point.as_str()));
        sandbox.seed_env();
        seed_previous_version(&sandbox);

        // ── Interrumpir ─────────────────────────────────────────────────────────
        let exe = sandbox.write_bundle(&sandbox.staging());
        let window = faults::armed(point);
        let error = runtime
            .block_on(install::install(
                &sandbox.install_env(&exe),
                &Sandbox::install_options(),
                &Inert,
            ))
            .expect_err("el punto inyectado hace fallar la operación");
        drop(window);

        assert!(
            error.to_string().contains(point.as_str()),
            "punto {}: el fallo es el inyectado y no otro: {error:#}",
            point.as_str()
        );
        if inside_transaction {
            let failure = error
                .downcast_ref::<avi_lifecycle::LifecycleError>()
                .expect("dentro de la transacción el fallo lleva `reason`");
            assert_eq!(failure.reason, "rolled_back");
            assert_eq!(
                ExitCode::from_reason(failure.reason).code(),
                13,
                "`RolledBack = 13` de la tabla única"
            );
            assert!(
                operational_previous_version(&sandbox),
                "punto {}: la versión anterior sigue operativa",
                point.as_str()
            );
        }

        // ── La siguiente operación completa ─────────────────────────────────────
        let exe = sandbox.write_bundle(&sandbox.staging());
        let installed = runtime
            .block_on(install::install(
                &sandbox.install_env(&exe),
                &Sandbox::install_options(),
                &Inert,
            ))
            .expect("la operación siguiente se completa");
        assert_eq!(installed.status, "installed");
        assert_eq!(installed.receipt.version, "0.24.0");
        assert_eq!(
            residue(&sandbox),
            Vec::<String>::new(),
            "punto {}: y no queda residuo en las raíces exclusivas",
            point.as_str()
        );
        assert!(
            avi_lifecycle::transaction::read_journal(&sandbox.program_dir)
                .expect("se relee el diario")
                .is_none(),
            "punto {}: el diario no sobrevive a la siguiente operación",
            point.as_str()
        );
    }
}

/// Sin el feature `faults` no hay punto de inyección, y eso es una garantía del producto y
/// no
/// una carencia: el binario distribuido no puede provocar un fallo a propósito.
///
/// La aserción también es la que impide que la capa 1 se apoye en la inyección sin
/// decirlo: si `armed` fuese efectivo sin el feature, esta prueba caería.
#[cfg(not(feature = "faults"))]
#[test]
fn fault_injection_is_inert_without_the_feature() {
    use avi_lifecycle::faults::{self, FaultPoint};

    let _guard = process_env::exclusively();
    let sandbox = Sandbox::new("inert-injection");
    sandbox.seed_env();
    let exe = sandbox.write_bundle(&sandbox.staging());

    let window = faults::armed(FaultPoint::BeforeCommit);
    let runtime = runtime::runtime();
    let outcome = runtime
        .block_on(install::install(
            &sandbox.install_env(&exe),
            &Sandbox::install_options(),
            &Inert,
        ))
        .expect("sin el feature, armar un punto no interrumpe nada");
    drop(window);

    assert_eq!(
        outcome.status, "installed",
        "sin `faults` la inyección es inerte, que es lo que el producto exige \
         del binario distribuido"
    );
    assert!(
        !faults::is_armed(FaultPoint::BeforeCommit),
        "y `is_armed` no miente: nada quedó armado"
    );
}

/// La regla dice que **toda** operación de ciclo de vida empieza por la recuperación, y esta
/// es
/// la comprobación de que la desinstalación no es la excepción: con una transacción
/// pendiente escrita, opera igualmente, sobre el directorio de programa registrado, y no
/// deja aparcados, stagings ni temporales.
#[test]
fn uninstall_recovers_before_it_plans() {
    let _guard = process_env::exclusively();
    let sandbox = Sandbox::new("recover-uninstall");
    sandbox.seed_env();
    let _parked = interrupt_at(&sandbox, JournalState::Placed);
    sandbox.seed_state();

    let runtime = runtime::runtime();
    let outcome = runtime
        .block_on(uninstall::run(
            &sandbox.env_uninstall(None, Channel::Unmanaged),
            &uninstall::Options {
                assume_yes: true,
                ..Default::default()
            },
            &Now,
            &Inert,
        ))
        .expect("la desinstalación se ejecuta");

    assert_eq!(outcome.status, "uninstalled");
    assert!(
        !inspect::exists(&sandbox.program_dir),
        "la desinstalación opera sobre el directorio de programa registrado"
    );
    assert_eq!(
        residue(&sandbox),
        Vec::<String>::new(),
        "y no deja aparcados ni temporales propios"
    );
    assert!(
        inspect::exists(&sandbox.staging()),
        "el staging lleva el token de este proceso, que sigue vivo, y no se barre"
    );
    assert!(
        !inspect::exists(&sandbox.data_dir) && !inspect::exists(&sandbox.models_dir),
        "el estado se borró con el alcance de `cleanup --all`"
    );
}

/// **El barrido decide por la identidad del propietario dentro de sus raíces fijas, y por
/// eso un sandbox ajeno al scratch nunca se toca.**
///
/// Se comprueba contra el **barrido real** y la identidad real de este proceso: el
/// scratch de un propietario muerto se borra, el de este proceso sobrevive, y lo que no
/// es de este modelo —los sandboxes de pruebas con cualquier nombre, directorios con la
/// forma de un token fuera del scratch y los nombres antiguos con PID— no se toca.
#[test]
fn sweep_never_touches_a_foreign_sandbox() {
    let _guard = process_env::exclusively();
    let sandbox = Sandbox::new("sandbox-sweep");
    sandbox.seed_env();

    // Un scratch de dueño muerto (PID que el sistema no puede asignar) y el de este
    // proceso, que sigue vivo.
    let dead = sandbox.scratch_root.join("4294967000-1");
    let own = sandbox.scratch_root.join(
        avi_process::current_owner_token()
            .expect("este proceso se puede observar")
            .format(),
    );
    support::write(&dead.join("clip.wav"), "scratch huérfano");
    support::write(&own.join("clip.wav"), "scratch de este proceso");
    // Sandboxes de pruebas junto al scratch y con la forma de un token fuera de él, y
    // nombres antiguos con PID dentro de él: nada de eso es de este modelo.
    let golden = sandbox.root.join("golden-sandbox_invariant_12345_0");
    let outside = sandbox.root.join("elsewhere").join("4294967000-1");
    let legacy = sandbox.scratch_root.join("avi-orphan-4294967000.tmp");
    let legacy_dir = sandbox.scratch_root.join("avi_clone_x_1");
    support::write(&golden.join("install").join("state"), "sandbox vivo");
    support::write(&outside.join("state"), "fuera del scratch");
    support::write(&legacy, "nombre antiguo");
    support::write(&legacy_dir.join("clip.wav"), "nombre antiguo");

    let outcome = recovery::recover(recovery::Roots {
        program_dir: &sandbox.program_dir,
        scratch_root: &sandbox.scratch_root,
        data_dir: &sandbox.data_dir,
        bin_dir: &sandbox.bin_dir,
    })
    .expect("el barrido se ejecuta");

    assert!(
        !inspect::exists(&dead),
        "el scratch de un dueño muerto se barre: {:?}",
        outcome.removed_scratch
    );
    assert!(inspect::exists(&own), "el de este proceso sobrevive");
    for survivor in [&golden, &outside, &legacy, &legacy_dir] {
        assert!(
            inspect::exists(survivor),
            "{} no es de este modelo y sobrevive al barrido",
            survivor.display()
        );
    }
    assert!(
        outcome.is_clean(),
        "nada queda pendiente: {:?}",
        outcome.kept
    );
}

/// Un PID reutilizado se barre aunque el proceso que lo tiene ahora siga vivo: el token
/// del temporal lleva otro inicio, luego su dueño es otro proceso y está muerto. El
/// temporal de este proceso, con su inicio real, se conserva sin informarse como pendiente.
#[test]
fn reused_pid_is_swept_while_the_live_owner_is_kept() {
    let _guard = process_env::exclusively();
    let sandbox = Sandbox::new("sweep-reused-pid");
    sandbox.seed_env();
    let own = avi_process::current_owner_token().expect("este proceso se puede observar");
    let reused = avi_shared::OwnerToken::new(own.pid(), "1").expect("token válido");
    let stale = sandbox
        .data_dir
        .join(avi_shared::staging_names::temp_file::name(
            "daemon.pid",
            &reused,
        ));
    let live = sandbox
        .data_dir
        .join(avi_shared::staging_names::temp_file::name(
            "daemon.ready",
            &own,
        ));
    support::write(&stale, "residuo de un proceso que tuvo este PID");
    support::write(&live, "temporal de este proceso, que sigue vivo");

    let outcome = recovery::recover(recovery::Roots {
        program_dir: &sandbox.program_dir,
        scratch_root: &sandbox.scratch_root,
        data_dir: &sandbox.data_dir,
        bin_dir: &sandbox.bin_dir,
    })
    .expect("el barrido se ejecuta");

    assert!(
        !inspect::exists(&stale),
        "el PID reutilizado se barre: {:?}",
        outcome.removed_temporaries
    );
    assert!(
        inspect::exists(&live),
        "el temporal de este proceso se conserva"
    );
    assert!(
        outcome.is_clean(),
        "y conservar lo vivo no es un pendiente: {:?}",
        outcome.kept
    );
}

/// Un diario que no se puede interpretar **detiene** la operación en vez de dejar que borre
/// a ciegas.
///
/// Es la postura de la recuperación aplicada al peor caso: si el estado del directorio de
/// programa no se puede leer, no se sabe qué operación lo dejó así, y borrar sin saberlo
/// sería peor que no borrar. La prueba afirma lo que el motor hace, no lo que el
/// enunciado no enumera: que la operación falla y que **no se borra nada**, que es la
/// propiedad de seguridad.
#[test]
fn unreadable_journal_stops_the_operation_without_deleting() {
    let _guard = process_env::exclusively();
    let sandbox = Sandbox::new("unreadable-journal");
    sandbox.seed_env();
    sandbox.seed_state();
    support::write(
        &avi_lifecycle::transaction::journal_path(&sandbox.program_dir),
        "{ esto no es un diario",
    );
    let receipt = sandbox.install_registered(PathIntegration::none());
    let before = state_without_lock(&sandbox);

    let runtime = runtime::runtime();
    runtime
        .block_on(uninstall::run(
            &sandbox.env_uninstall(Some(&receipt), Channel::Script),
            &uninstall::Options {
                assume_yes: true,
                ..Default::default()
            },
            &Now,
            &Inert,
        ))
        .expect_err("un diario ilegible detiene la operación");

    assert_eq!(
        state_without_lock(&sandbox),
        before,
        "y no borra nada, ni programa, ni estado, ni modelo"
    );
    assert!(
        inspect::exists(&sandbox.program_dir)
            && inspect::exists(&sandbox.data_dir.join("voices").join("mine"))
            && inspect::exists(&sandbox.models_dir),
        "las tres raíces siguen donde estaban"
    );
}
