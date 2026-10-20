// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! `self install` de punta a punta sobre raíces reubicadas a temporales.
//!
//! Todas las raíces se reubican por sus variables, que es lo que el aislamiento declara
//! que hace posibles las pruebas aisladas: en Windows las Known Folders ignoran
//! `LOCALAPPDATA`, así que el sandbox no puede apoyarse en ellas. El directorio de
//! programa, el directorio del enlace, la raíz de datos, la raíz de modelos, el
//! `HOME` y la clave de registro son todos del test.
//!
//! El bundle que se instala es **sintético y completo**: los cinco documentos, la
//! librería de runtime y el derivado del motor, con los nombres exactos que
//! `packaging/bundle-manifest.json` exige para el target del host. El ejecutable es
//! un fichero de texto: esta prueba afirma el flujo de la instalación, no que el binario
//! arranque, que es lo que hace el bootstrap en su último paso.
//!
//! `--no-setup` en todas las instalaciones: la provisión de modelos necesita red y es
//! la prueba de `setup`, no la de `self install`.

#![allow(clippy::disallowed_methods)]

// Las pruebas de este archivo no serializan por el entorno: cada una tiene su sandbox
// con directorio y clave de registro únicos, y un candado global añadiría un guard
// sostenido a través de un `await`, que es justo lo que un test asíncrono no debe hacer.
#[path = "support/bundle.rs"]
mod bundle;
#[path = "support/inert.rs"]
mod inert;
#[path = "support/install_env.rs"]
mod install_env;
#[path = "support/listing.rs"]
mod listing;
#[path = "support/net.rs"]
mod net;
mod support;

use avi_core::exit_codes::ExitCode;
use avi_lifecycle::install::{self, Mode};
use avi_lifecycle::receipt::{self, PathIntegration};
use bundle::place;
use inert::Inert;
use std::path::Path;
use support::Sandbox;

/// Idempotencia de punta a punta: instalar dos veces desde el mismo bundle deja el
/// mismo estado final, sin entradas de `PATH` ni bloques de perfil duplicados.
#[tokio::test]
async fn install_twice_is_idempotent() {
    let sandbox = Sandbox::new("double");
    let exe = sandbox.write_bundle(&sandbox.staging());
    let env = sandbox.install_env(&exe);
    let options = Sandbox::install_options();

    // ── Primera instalación ─────────────────────────────────────────────────────
    let first = install::install(&env, &options, &Inert)
        .await
        .expect("la primera instalación se completa");
    assert_eq!(first.mode, Mode::Install);
    assert_eq!(first.status, "installed");
    assert_eq!(first.models, avi_lifecycle::install::ModelsState::Skipped);

    // El bundle llegó al directorio de programa y el recibo está escrito.
    let section = avi_lifecycle::manifest::target_section(&env.target).unwrap();
    for relative in &section.required {
        assert!(
            place(&sandbox.program_dir, relative).is_file(),
            "el bundle colocado tiene {relative}"
        );
    }
    let receipt = receipt::read_from(&sandbox.program_dir)
        .expect("se lee el recibo")
        .expect("el recibo existe tras instalar");
    assert_eq!(receipt.version, "0.24.0");
    assert_eq!(receipt.app, "ai-voice-interconnector");
    assert_eq!(receipt.channel, avi_lifecycle::channel::Channel::Script);
    assert_eq!(receipt.roots.data_dir, sandbox.data_dir);
    assert_eq!(receipt.roots.cache_dir, sandbox.cache_dir);
    assert_eq!(
        receipt.files, section.required,
        "el recibo lista lo colocado"
    );
    assert_eq!(
        receipt.path_integration.modify_path,
        !cfg!(windows) || first.path_integration.modify_path,
        "el `PATH` se integra por defecto salvo `--no-modify-path`"
    );

    // La integración quedó registrada con lo que se hizo de verdad.
    let integration = first.path_integration.clone();
    if cfg!(unix) {
        assert!(integration.modify_path, "en Unix se modifica el `PATH`");
        let link = integration.symlink.as_ref().expect("el enlace se registra");
        assert_eq!(link, &sandbox.bin_dir.join("ai-voice-interconnector"));
        let blocks = integration
            .profile_blocks
            .as_ref()
            .expect("los bloques se registran");
        assert_eq!(blocks.len(), 1, "un solo archivo de arranque");
        let profile = sandbox.home.join(".profile");
        assert_eq!(blocks[0], profile);
        let text = std::fs::read_to_string(&profile).expect("el perfil existe");
        assert_eq!(
            text.matches(avi_lifecycle::path_unix::BLOCK_BEGIN).count(),
            1,
            "un solo bloque delimitado: {text}"
        );
    } else {
        assert!(
            integration.modify_path,
            "en Windows la entrada del registro queda registrada: {integration:?}"
        );
        assert!(integration.registry_entry.is_some());
        assert!(integration.symlink.is_none());
    }
    assert!(
        first.needs_new_terminal(),
        "el `PATH` se tocó, así que el resumen pide terminal nueva"
    );

    // ── Segunda instalación, desde el mismo bundle ──────────────────────────────
    // El bundle se repone porque la colocación **mueve** los archivos desde el
    // origen: sin reponerlo, la segunda pasada se instalaría un bundle
    // vacío. Es también lo que hace el bootstrap, que extrae en un staging
    // nuevo cada vez. El ejecutable vuelve a ser el del staging, que es el que la
    // primera pasada dejó de tener alrededor al mover el bundle.
    let _exe = sandbox.write_bundle(&sandbox.staging());
    let content_after_first = listing::list(&sandbox.program_dir);
    let second = install::install(&env, &options, &Inert)
        .await
        .expect("la segunda instalación se completa");

    // ── El estado final es el mismo ─────────────────────────────────────────────
    assert_eq!(
        second.mode,
        Mode::Install,
        "sigue siendo instalación, no reparación"
    );
    assert_eq!(second.status, "installed");
    assert_eq!(second.receipt.version, first.receipt.version);
    assert_eq!(
        second.path_integration, integration,
        "la integración es la misma: ni entradas ni bloques nuevos"
    );
    assert!(
        !second.path_changed(),
        "y la segunda pasada no reescribe el `PATH`: la entrada ya estaba"
    );
    assert!(
        second.path_integrated(),
        "pero sigue registrada en el recibo, que es lo que permite revertirla"
    );
    // El resumen **final** no es idéntico entre las dos pasadas, y no debería: la
    // segunda no reescribe el `PATH` y decirlo sería mentir. Lo que tiene que ser
    // igual es el estado, que es lo que la idempotencia afirma.
    //
    // La duración que acompaña al resumen tampoco es estado: es el tiempo que
    // tardó cada pasada, y las dos no tardan lo mismo. Se quitan las duraciones
    // del final de la línea para que la aserción afirme lo que afirma —el
    // estado— y no el reloj de dos ejecuciones.
    let state_lines = |summary: &[String]| -> Vec<String> {
        summary
            .iter()
            .filter(|l| !l.trim_start().starts_with("PATH:"))
            .map(|l| without_trailing_duration(l))
            .collect()
    };
    assert_eq!(
        state_lines(&second.summary),
        state_lines(&first.summary),
        "el estado final es el mismo"
    );
    assert!(
        first.path_changed() && !second.path_changed(),
        "pero la primera pasada sí reescribió el `PATH` y la segunda no"
    );
    assert!(
        second.receipt.path_integration == first.receipt.path_integration,
        "y el recibo de la segunda pasada conserva la integración de la primera: si \
         registrara solo el diff de esta pasada, `self uninstall` no podría revertir \
         la entrada que puso la primera instalación"
    );

    // El directorio de programa tiene el mismo contenido que después de la primera.
    assert_eq!(
        listing::list(&sandbox.program_dir),
        content_after_first,
        "el contenido del directorio de programa no cambió"
    );
    assert!(
        content_after_first.contains(&"install-receipt.json".to_string()),
        "y incluye el recibo, que es uno de los archivos que la operación escribe"
    );
    for relative in &section.required {
        assert!(
            content_after_first.contains(relative),
            "el contenido incluye {relative}, que es lo que el manifiesto exige"
        );
    }

    if cfg!(unix) {
        let text = std::fs::read_to_string(sandbox.home.join(".profile")).unwrap();
        assert_eq!(
            text.matches(avi_lifecycle::path_unix::BLOCK_BEGIN).count(),
            1,
            "el bloque de perfil no se duplicó: {text}"
        );
    }

    // Y el recibo se relee con la misma forma.
    let loaded = receipt::read_from(&sandbox.program_dir).unwrap().unwrap();
    assert_eq!(loaded.files, first.receipt.files);
    assert_eq!(
        loaded.path_integration, integration,
        "el recibo de disco coincide con el que devolvió la operación"
    );
}

/// Quita del final de una línea la duración que el resumen añade.
///
/// El resumen de una instalación termina con el tiempo que tardó, entre
/// paréntesis: «ai-voice-interconnector instalado: 0.34.0 (1 s)». Ese tiempo
/// es el reloj de una ejecución, no el estado que deja, y dos ejecuciones del
/// mismo estado no tardan lo mismo. La línea sin ese paréntesis es la que
/// afirma el estado.
///
/// El helper va anclado al formato de la duración, no a «cualquier paréntesis
/// final»: una línea del resumen que acabara en `)` por otro motivo no debe
/// mutilarse en silencio.
fn without_trailing_duration(line: &str) -> String {
    let trimmed = line.trim_end();
    // `format_duration` escribe «7 s», «1 min 59 s» o «1 h 02 min».
    let Some(pos) = trimmed.rfind(" (") else {
        return line.to_string();
    };
    let duration = &trimmed[pos + 2..trimmed.len() - 1];
    let looks_like_duration = duration.ends_with(" s")
        || duration.ends_with(" min")
        || duration.ends_with(" h")
        || duration
            .split_once(" min ")
            .is_some_and(|(_, rest)| rest.ends_with(" s"));
    if trimmed.ends_with(')') && looks_like_duration {
        trimmed[..pos].to_string()
    } else {
        line.to_string()
    }
}

/// Sondea el reloj hasta que `measured` tenga al menos `RESOLUTION` de antigüedad
/// (1 s es el peor caso conocido de resolución de `mtime`), con `deadline` como
/// límite duro: agotado el plazo, falla con `panic!` en vez de seguir y
/// comparar marcas que aún no se distinguen.
///
/// El hecho que espera es «la marca medida ya está fuera del tick del reloj», no
/// «ha pasado un segundo»: por eso sondea y no duerme, y por eso el diagnóstico
/// dice cuánto faltó en vez de callar.
fn wait_for_mtime_resolution(measured: std::time::SystemTime, deadline: std::time::Duration) {
    const RESOLUTION: std::time::Duration = std::time::Duration::from_secs(1);
    let start = std::time::Instant::now();
    loop {
        let elapsed = measured.elapsed().unwrap_or_default();
        if elapsed >= RESOLUTION {
            return;
        }
        assert!(
            start.elapsed() < deadline,
            "el reloj no avanzó lo necesario para separar la mtime (faltaban {:?} de {:?})",
            RESOLUTION - elapsed,
            RESOLUTION
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// La reparación desde dentro del directorio de programa no copia archivos: reaplica
/// integración de `PATH`, cuarentena y recibo.
///
/// La afirmación central es que **el ejecutable no se toca**, y se hace comparando el
/// `mtime` del bundle colocado: si la reparación hubiera copiado algo, el directorio
/// de programa se habría recreado y la marca de tiempo sería posterior.
#[tokio::test]
async fn repair_from_inside_program_dir_copies_nothing() {
    let sandbox = Sandbox::new("repair");
    let exe = sandbox.write_bundle(&sandbox.staging());
    let options = Sandbox::install_options();

    install::install(&sandbox.install_env(&exe), &options, &Inert)
        .await
        .expect("la instalación inicial se completa");

    // Se borra la integración: es exactamente lo que una reparación tiene que
    // reaplicar, y lo que un usuario ve cuando su perfil se sobrescribió.
    let receipt = receipt::read_from(&sandbox.program_dir).unwrap().unwrap();
    if cfg!(unix) {
        for block in receipt.path_integration.profile_blocks.iter().flatten() {
            std::fs::remove_file(block).expect("se borra el perfil");
        }
    }
    // Y se marca el bundle colocado para que un copiado se note.
    let placed = sandbox.program_dir.join(
        avi_lifecycle::manifest::target_section(avi_lifecycle::target::host_triple())
            .unwrap()
            .executable,
    );
    let before = std::fs::metadata(&placed)
        .and_then(|m| m.modified())
        .expect("el ejecutable colocado tiene fecha");
    // La separación de `mtime` se consigue por sondeo del reloj, no durmiendo a
    // ciegas: se espera, con plazo acotado y fallo ruidoso si no llega, a que la
    // marca vigente haya superado la resolución del sistema de ficheros (1 s es
    // el peor caso conocido). Ese margen es lo que hace concluyente la
    // comparación posterior: sin él, una copia hecha durante la reparación
    // podría caer en el mismo tick y conservar la marca.
    wait_for_mtime_resolution(before, std::time::Duration::from_secs(5));

    // Ahora se invoca **desde dentro** del directorio de programa.
    let env = sandbox.install_env(&placed);
    assert_eq!(
        install::detect_mode(&env.exe, &env.program_dir),
        Mode::Repair,
        "dentro del directorio de programa, el modo es reparación"
    );
    let repair = install::install(&env, &options, &Inert)
        .await
        .expect("la reparación se completa");

    assert_eq!(repair.mode, Mode::Repair);
    assert_eq!(repair.status, "repaired");
    assert_eq!(
        std::fs::metadata(&placed)
            .and_then(|m| m.modified())
            .expect("el ejecutable sigue ahí"),
        before,
        "la reparación no copió archivos: el ejecutable conserva su fecha"
    );
    assert!(
        repair.receipt.path_integration.modify_path,
        "la reparación reaplicó la integración del `PATH`"
    );
    if cfg!(unix) {
        let profile = sandbox.home.join(".profile");
        let text = std::fs::read_to_string(&profile).expect("el perfil se volvió a escribir");
        assert_eq!(
            text.matches(avi_lifecycle::path_unix::BLOCK_BEGIN).count(),
            1,
            "el bloque se reaplicó una sola vez"
        );
    }
    assert!(
        repair.summary.iter().any(|l| l.contains("reparado")),
        "el resumen final dice que fue una reparación: {:?}",
        repair.summary
    );
    assert!(
        repair
            .summary_before
            .iter()
            .any(|l| l.contains("no se copian archivos")),
        "y el resumen previo lo anuncia antes de confirmar: {:?}",
        repair.summary_before
    );
}

/// Un bundle fuera del volumen del programa (el temporal del bootstrap) se instala
/// desde una copia propia junto al programa: el original no se toca y, tras instalar,
/// no queda ningún staging hermano.
#[tokio::test]
async fn bundle_outside_program_parent_is_installed_from_own_staging_copy() {
    let sandbox = Sandbox::new("foreign-bundle");
    let foreign = sandbox.home.join("descarga-bootstrap");
    let exe = sandbox.write_bundle(&foreign);

    install::install(
        &sandbox.install_env(&exe),
        &Sandbox::install_options(),
        &Inert,
    )
    .await
    .expect("la instalación desde un bundle ajeno se completa");

    let section =
        avi_lifecycle::manifest::target_section(avi_lifecycle::target::host_triple()).unwrap();
    for relative in &section.required {
        assert!(
            place(&sandbox.program_dir, relative).is_file(),
            "{relative}"
        );
        assert!(
            place(&foreign, relative).is_file(),
            "el original no se mueve: {relative}"
        );
    }
    let parent = sandbox.program_dir.parent().unwrap();
    let leftovers: Vec<_> = std::fs::read_dir(parent)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.starts_with(".ai-voice-interconnector-staging-"))
        .collect();
    assert!(leftovers.is_empty(), "staging sin borrar: {leftovers:?}");
}

/// Sin bundle alrededor, `self install` termina con `bundle_invalid` y no modifica
/// nada, que es el tercer modo de la operación.
#[tokio::test]
async fn bundle_without_required_files_is_rejected() {
    let sandbox = Sandbox::new("incomplete");

    // Un "bundle" con el ejecutable y nada más: es el caso de `target/release`, que
    // el flujo de la instalación nombra explícitamente.
    let exe = sandbox.staging().join(
        avi_lifecycle::manifest::target_section(avi_lifecycle::target::host_triple())
            .unwrap()
            .executable,
    );
    std::fs::create_dir_all(sandbox.staging()).expect("se crea el staging");
    std::fs::write(&exe, "binario suelto\n").expect("se escribe el ejecutable");

    let before = state(&sandbox.root);
    let err = install::install(
        &sandbox.install_env(&exe),
        &Sandbox::install_options(),
        &Inert,
    )
    .await
    .expect_err("un bundle sin los archivos obligatorios no se instala");

    // `install` devuelve `anyhow::Error` porque hay fallos de E/S sin `reason` propio,
    // pero los que la tabla de reasons declara viajan dentro como `LifecycleError`, que
    // es lo que el envelope emite.
    let failure = err
        .downcast_ref::<avi_lifecycle::LifecycleError>()
        .unwrap_or_else(|| panic!("el fallo declara un `reason`: {err:#}"));
    assert_eq!(failure.reason, "bundle_invalid");
    assert_eq!(
        ExitCode::from_reason(failure.reason).code(),
        15,
        "`BundleInvalid = 15` de la tabla única"
    );
    assert!(
        failure.message.contains("faltan") && failure.message.contains("archivo(s)"),
        "el mensaje nombra los ausentes: {failure}"
    );
    assert_eq!(
        state(&sandbox.root),
        before,
        "nada modificado: la validación es del paso 2, antes de tocar el disco"
    );
    assert!(
        receipt::read_from(&sandbox.program_dir).unwrap().is_none(),
        "y no hay recibo"
    );
}

/// `--no-modify-path` no toca ni el perfil ni el registro, y el recibo lo dice para
/// que `self uninstall` no tenga que revertir nada (la otra mitad de la idempotencia).
#[tokio::test]
async fn no_modify_path_leaves_path_untouched() {
    let sandbox = Sandbox::new("sin-path");
    let exe = sandbox.write_bundle(&sandbox.staging());
    let options = install::Options {
        no_modify_path: true,
        ..Sandbox::install_options()
    };

    let outcome = install::install(&sandbox.install_env(&exe), &options, &Inert)
        .await
        .expect("la instalación se completa");
    assert_eq!(outcome.path_integration, PathIntegration::none());
    assert!(!outcome.path_changed(), "no se cambió el `PATH`");
    assert!(
        !outcome.needs_new_terminal(),
        "así que no se pide terminal nueva"
    );
    assert!(
        !sandbox.home.join(".profile").exists(),
        "no se creó ningún perfil"
    );
    assert!(
        receipt::read_from(&sandbox.program_dir)
            .unwrap()
            .unwrap()
            .path_integration
            == PathIntegration::none(),
        "el recibo dice que no se integró nada"
    );
}

/// Sobre una instalación ya integrada el plan del `PATH` es un no-op y el resumen
/// previo lo dice; antes de integrar, el mismo resumen anuncia el cambio.
#[tokio::test]
async fn integrated_install_announces_no_path_change() {
    let sandbox = Sandbox::new("integrated");
    let exe = sandbox.write_bundle(&sandbox.staging());
    let options = Sandbox::install_options();

    // Sin integrar: el plan anuncia el cambio.
    let before = sandbox.install_env(&exe);
    assert!(
        !install::plan_path(&before, &options).is_noop(),
        "antes de integrar hay algo que anunciar"
    );

    install::install(&before, &options, &Inert)
        .await
        .expect("la primera instalación se completa");

    // Integrada: el bundle se repone porque la colocación lo mueve, y el `PATH` de la
    // sesión incluye ya el directorio del enlace, como tras abrir una terminal nueva.
    let exe = sandbox.write_bundle(&sandbox.staging());
    let mut env = sandbox.install_env(&exe);
    env.path_env = format!("/usr/bin:{}", sandbox.bin_dir.display());
    assert!(
        install::plan_path(&env, &options).is_noop(),
        "con la integración hecha no hay nada que cambiar"
    );
    let second = install::install(&env, &options, &Inert)
        .await
        .expect("la segunda instalación se completa");
    let expected = if cfg!(windows) {
        format!(
            "  PATH:      {} ya está en el PATH del usuario; no se modifica",
            sandbox.bin_dir.display()
        )
    } else {
        format!(
            "  PATH:      {} ya está en el PATH; no se modifica",
            sandbox.bin_dir.display()
        )
    };
    assert!(
        second.summary_before.contains(&expected),
        "{:?}",
        second.summary_before
    );
}

/// Un `path_conflict` —algo que no es un enlace nuestro en la ruta del enlace— aborta
/// con su `reason` y su código, y **no integra el `PATH` ni registra nada**.
///
/// El nombre dice "no instala nada" y la afirmación es más estrecha a propósito: la
/// tabla de reasons
/// solo declara "nada modificado" para `daemon_stop_failed` y `bundle_invalid`, no para
/// `path_conflict`, y el conflicto se detecta en el **paso 8**, después de la colocación
/// transaccional del paso 6. El disco, por tanto, no queda idéntico —el bundle está en
/// el directorio de programa y el archivo de bloqueo existe— y afirmar lo contrario sería
/// afirmar más de lo que la especificación promete. Lo que sí tiene que ser cierto, y es
/// lo que se afirma, es que la ruta ajena no se toca, que el `PATH` no se integra y que
/// no se escribe recibo.
///
/// La prueba solo corre en Unix porque en Windows la análoga —una entrada ajena en el
/// registro— la comprueba `windows_user_path_type_and_vars_survive`; en Windows esta
/// prueba sale por el `return` de abajo.
#[tokio::test]
async fn foreign_path_is_conflict_and_installs_nothing() {
    if !cfg!(unix) {
        return;
    }
    let sandbox = Sandbox::new("conflict");
    let exe = sandbox.write_bundle(&sandbox.staging());
    let busy = sandbox.bin_dir.join("ai-voice-interconnector");
    std::fs::write(&busy, "no soy un enlace\n").expect("se ocupa la ruta del enlace");

    let err = install::install(
        &sandbox.install_env(&exe),
        &Sandbox::install_options(),
        &Inert,
    )
    .await
    .expect_err("una ruta ajena es conflicto");
    let failure = err
        .downcast_ref::<avi_lifecycle::LifecycleError>()
        .unwrap_or_else(|| panic!("el fallo declara un `reason`: {err:#}"));
    assert_eq!(failure.reason, "path_conflict");
    assert_eq!(
        ExitCode::from_reason(failure.reason).code(),
        14,
        "`PathConflict = 14` de la tabla única"
    );
    assert_eq!(
        std::fs::read_to_string(&busy).unwrap(),
        "no soy un enlace\n",
        "el fichero ajeno no se toca"
    );
    assert!(
        !sandbox.home.join(".profile").exists(),
        "y el `PATH` no se integra: el conflicto se detecta antes de escribir el bloque"
    );
    assert!(
        receipt::read_from(&sandbox.program_dir).unwrap().is_none(),
        "y no se escribe recibo: la integración es anterior a él"
    );
    // Lo que la tabla sí permite, y que conviene afirmar para que la diferencia con
    // `bundle_invalid` quede explícita: el bundle ya está colocado.
    assert!(
        sandbox
            .program_dir
            .join(avi_lifecycle::uninstall::executable_name_default())
            .is_file(),
        "el bundle quedó colocado en el directorio de programa: el conflicto es del \
         paso 8, no de la colocación"
    );
}

/// El estado del sandbox para comparar antes y después de una operación que se
/// supone que no modifica nada.
///
/// El **archivo de bloqueo queda fuera a propósito**: la recuperación lo crea al tomar el
/// bloqueo, que es el paso 1, y el «nada modificado» de `bundle_invalid` se refiere al
/// estado de la instalación —programa, datos, perfiles, registro y receipt—, no al
/// mecanismo que serializa las operaciones. Exigir que ni el bloqueo aparezca sería
/// exigir que el paso 1 no ocurriera, que es otra cosa.
fn state(root: &Path) -> Vec<String> {
    listing::list(root)
        .into_iter()
        .filter(|e| {
            ![
                avi_lifecycle::LIFECYCLE_LOCK_NAME,
                avi_lifecycle::LIFECYCLE_GATE_NAME,
            ]
            .contains(&e.rsplit('/').next().unwrap_or_default())
        })
        .collect()
}

/// Con un comando de uso en curso (bloqueo compartido tomado), `self install` termina
/// con `lifecycle_locked` sin tocar el disco, y al terminar una instalación el
/// bloqueo exclusivo queda libre.
#[tokio::test]
async fn install_honors_and_releases_the_lifecycle_lock() {
    let sandbox = Sandbox::new("lock");
    let exe = sandbox.write_bundle(&sandbox.staging());
    let env = sandbox.install_env(&exe);

    let usage = avi_lifecycle::lock::acquire_shared(&sandbox.program_dir)
        .expect("el comando de uso toma el bloqueo compartido");
    let before = state(&sandbox.root);
    let err = install::install(&env, &Sandbox::install_options(), &Inert)
        .await
        .expect_err("la instalación no corre con un uso en curso");
    let failure = err
        .downcast_ref::<avi_lifecycle::LifecycleError>()
        .unwrap_or_else(|| panic!("el fallo declara un `reason`: {err:#}"));
    assert_eq!(failure.reason, "lifecycle_locked");
    assert_eq!(state(&sandbox.root), before, "nada modificado");
    drop(usage);

    install::install(&env, &Sandbox::install_options(), &Inert)
        .await
        .expect("sin uso en curso la instalación se completa");
    drop(
        avi_lifecycle::lock::acquire_exclusive(&sandbox.program_dir)
            .expect("al terminar, el bloqueo exclusivo queda libre"),
    );
}
