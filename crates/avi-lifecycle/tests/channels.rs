//! Los cuatro canales de §8.2 en las operaciones de este ciclo.
//!
//! La tabla de §8.2 tiene una columna por operación y una fila por canal. Las celdas de
//! `self update` son del Ciclo 2 y este archivo las cubre en lo que le es propio —la
//! detección—, mientras que la negativa sin tocar nada es el criterio 15 en
//! `tests/update.rs`. Son cuatro cosas distintas que conviene no mezclar:
//!
//! 1. **Cómo se origina cada canal.** La precedencia de §8.2 —Homebrew sobre el recibo, el
//!    recibo sobre `unmanaged`— y el hecho de que `self install` escriba el canal en el
//!    recibo y de que `--channel dev` (§10.5) solo surta efecto cuando el recibo se crea por
//!    primera vez.
//! 2. **Qué puede hacer cada canal.** `homebrew` es `externally_managed` con el comando
//!    correcto y sin tocar nada; `script`, `dev` y `unmanaged` pueden desinstalar. Para
//!    `self update`, `homebrew` y `dev` son `externally_managed`: la detección es la misma
//!    de §8.2 y la afirma `update_detects_managed_channels`.
//! 3. **Sobre qué opera la desinstalación.** §8.2 dice que `self uninstall` actúa siempre
//!    sobre la **instalación registrada**, sea cual sea la copia del binario que ejecute el
//!    comando. Es la propiedad que hace que `self uninstall` funcione desde `target/`, y es
//!    la que ninguna otra prueba del ciclo cubre de punta a punta.
//! 4. **Qué ve `self update` en cada canal.** El brazo `Update` se niega antes de tocar la
//!    red o el disco cuando la detección dice `homebrew` o `dev`; `script` y `unmanaged`
//!    siguen adelante. Es la misma detección de los puntos anteriores, ejercitada con las
//!    entradas que ese brazo distingue.

#![allow(clippy::disallowed_methods)]

mod support;

use avi_lifecycle::channel::{self, Channel};
use avi_lifecycle::cleanup;
use avi_lifecycle::install::{self, Options as InstallOptions};
use avi_lifecycle::receipt::{self, PathIntegration};
use avi_lifecycle::uninstall;
use support::{Inert, Now, Sandbox};

/// Opciones de instalación desatendida sin provisión de modelos y con `--channel dev`.
fn dev_options() -> InstallOptions {
    InstallOptions {
        channel: Some(Channel::Dev),
        ..Sandbox::install_options()
    }
}

/// Los cuatro canales de §8.2 se comportan como la tabla dice, en las operaciones de este
/// ciclo.
#[test]
fn channels_behave_per_spec() {
    let _guard = support::exclusively();
    let runtime = support::runtime();

    // ── 1. Cómo se origina cada canal ──────────────────────────────────────────
    // La precedencia de §8.2: Homebrew sobre el recibo, el recibo sobre `unmanaged`.
    // Homebrew gana porque el Cask no deja recibo —lo gestiona otra herramienta— y porque
    // una copia del Cask ejecutándose dentro de otra instalación sigue siendo de Homebrew:
    // es el prefijo el que manda, no el papel del directorio.
    let sandbox = Sandbox::new("canales");
    sandbox.seed_env();
    let caskroom = sandbox.root.join("opt").join("homebrew").join("Caskroom");
    let cask_exe = caskroom.join("ai-voice-interconnector");

    let script = sandbox.install_registered(PathIntegration::none());
    assert_eq!(
        script.channel,
        Channel::Script,
        "§8.2: por defecto, `script`"
    );
    assert_eq!(
        channel::detect(
            &sandbox.program_dir.join("ai-voice-interconnector"),
            Some(&script)
        ),
        Channel::Script,
        "§8.2: sin Cask, manda el recibo"
    );
    assert_eq!(
        channel::detect(&cask_exe, Some(&script)),
        Channel::Homebrew,
        "§8.2: y el prefijo de Homebrew gana sobre el recibo"
    );
    assert_eq!(
        channel::detect(&sandbox.staging.join("ai-voice-interconnector"), None),
        Channel::Unmanaged,
        "§8.2: sin recibo y sin Cask, `unmanaged`"
    );
    // `unmanaged` es el canal de un binario ejecutado fuera de una instalación, y por eso
    // `self install` funciona desde `target/`: instala si el bundle es válido.
    let from_staging = sandbox.write_bundle(&sandbox.staging);
    let installed = runtime
        .block_on(install::install(
            &sandbox.install_env(&from_staging),
            &InstallOptions {
                channel: Some(Channel::Dev),
                ..Sandbox::install_options()
            },
            &Inert,
        ))
        .expect("§8.2: `self install` desde fuera de una instalación instala");
    assert_eq!(
        installed.receipt.channel,
        Channel::Script,
        "§8.2: el valor por defecto es `script`"
    );

    // ── `--channel dev` solo cuando el recibo se crea por primera vez ──────────
    // Es la opción oculta de §10.5, y su regla es lo que hace que una instalación de
    // desarrollo siga siendo `dev` después de una reparación.
    let dev = Sandbox::new("canal-dev");
    dev.seed_env();
    let exe = dev.write_bundle(&dev.staging);
    let first = runtime
        .block_on(install::install(
            &dev.install_env(&exe),
            &dev_options(),
            &Inert,
        ))
        .expect("§8.2: `self install --channel dev` se completa");
    assert_eq!(
        first.receipt.channel,
        Channel::Dev,
        "§8.2: `--channel dev` origina el canal `dev`"
    );
    assert_eq!(first.receipt.channel.as_str(), "dev");

    let exe = dev.write_bundle(&dev.staging);
    let second = runtime
        .block_on(install::install(
            &dev.install_env(&exe),
            &dev_options(),
            &Inert,
        ))
        .expect("§8.2: la segunda pasada se completa");
    assert_eq!(
        second.receipt.channel,
        Channel::Dev,
        "§8.2: y una reparación conserva el canal, que es lo que hace que `self update` siga \
         tratando la instalación como `dev`"
    );
    let on_disk = receipt::read_from(&dev.program_dir)
        .expect("§8.2: se lee el recibo")
        .expect("§8.2: el recibo existe");
    assert_eq!(
        on_disk.channel,
        Channel::Dev,
        "§8.2: y el recibo de disco lo dice"
    );

    // ── 2. `homebrew` es `externally_managed` y no toca nada ────────────────────
    // Las dos operaciones de §8.2 que declaran el canal `homebrew` con comando propio.
    let hb = Sandbox::new("canal-homebrew");
    hb.seed_env();
    hb.seed_state();
    let hb_receipt = hb.install_registered(PathIntegration::none());
    let before = hb.snapshot();

    let error = runtime
        .block_on(uninstall::run(
            &hb.env_uninstall(Some(&hb_receipt), Channel::Homebrew),
            &uninstall::Options {
                assume_yes: true,
                ..Default::default()
            },
            &Now,
            &Inert,
        ))
        .expect_err("§8.2: `homebrew` no se desinstala desde aquí");
    let failure = error
        .downcast_ref::<avi_lifecycle::LifecycleError>()
        .expect("§8.2: el fallo declara un `reason`");
    assert_eq!(failure.reason, "externally_managed");
    assert_eq!(
        failure.exit_code, 12,
        "`ExternallyManaged = 12` de la tabla cerrada"
    );
    assert!(
        failure.message.contains(uninstall::HOMEBREW_UNINSTALL),
        "§8.2: el mensaje lleva el comando de Homebrew exacto: {}",
        failure.message
    );
    assert!(
        failure.message.contains("cleanup --all"),
        "§8.2: y sugiere `cleanup --all` para el estado de usuario"
    );
    assert_eq!(
        hb.snapshot(),
        before,
        "§8.2: y nada se toca, ni programa, ni estado, ni modelos"
    );

    // `cleanup` sí opera en los cuatro canales: no depende del canal, y el estado de
    // usuario es del usuario.
    for channel in [
        Channel::Script,
        Channel::Dev,
        Channel::Homebrew,
        Channel::Unmanaged,
    ] {
        let s = Sandbox::new(&format!("cleanup-{channel}"));
        s.seed_env();
        s.seed_state();
        s.install_registered(PathIntegration::none());
        assert_eq!(
            s.env_uninstall(None, channel).channel,
            channel,
            "§8.2: {channel}"
        );
        let outcome = runtime
            .block_on(cleanup::run(
                &s.roots(),
                &cleanup::Options {
                    all: true,
                    assume_yes: true,
                    ..Default::default()
                },
                &Inert,
            ))
            .expect("§8.2: `cleanup` se ejecuta en cualquier canal");
        assert_eq!(outcome.status, "cleanup_complete", "§8.2: {channel}");
        assert!(
            !support::exists(&s.models_dir),
            "§8.2: {channel}: borra los modelos"
        );
        assert!(
            support::exists(&s.program_dir),
            "§8.2: {channel}: y nunca borra el programa"
        );
    }

    // ── 3. `self uninstall` opera sobre la instalación registrada ──────────────
    // La fila de §8.2 dice, para los cuatro canales, que `self uninstall` actúa sobre la
    // instalación registrada. La prueba lo monta al revés de lo habitual: el recibo apunta
    // a un directorio y la operación se invoca con `program_dir` de otro sitio, que es lo
    // que pasa cuando el comando corre desde `target/` o desde el bundle extraído a mano.
    let registered = Sandbox::new("registrado");
    registered.seed_env();
    registered.seed_state();
    let registered_receipt = registered.install_registered(PathIntegration::none());

    let bare = Sandbox::new("al-pie");
    bare.seed_env();
    bare.seed_state();
    bare.install_registered(PathIntegration::none());

    let env = env_over_registered(&registered, &registered_receipt);
    let outcome = runtime
        .block_on(uninstall::run(
            &env,
            &uninstall::Options {
                assume_yes: true,
                ..Default::default()
            },
            &Now,
            &Inert,
        ))
        .expect("§8.2: la desinstalación se ejecuta");
    assert_eq!(outcome.status, "uninstalled");
    assert!(
        !support::exists(&registered.program_dir),
        "§8.2: se borra la instalación **registrada**, que es la del recibo"
    );
    assert!(
        support::exists(&bare.program_dir),
        "§8.2: y la copia de la que se invoca el comando no se toca, porque no es la \
         instalación registrada"
    );
    assert!(
        support::exists(&bare.data_dir) && support::exists(&bare.models_dir),
        "§8.2: tampoco sus raíces, porque el recibo manda sobre dónde se resuelven ahora: es \
         lo que permite que la actualización y la desinstalación operen sobre las mismas \
         ubicaciones aunque la variable ya no esté definida"
    );
    assert!(
        registered.program_dir != bare.program_dir,
        "§8.2: los dos directorios son distintos, que es lo que hace la prueba significativa"
    );

    // Y el directorio de programa sobre el que se opera sale del recibo, no de la
    // resolución por convención: con `AVI_INSTALL_DIR` apuntando al otro sitio, la
    // operación sigue intentando el registrado.
    let resolved = channel::registered_install_dir(Some(&registered_receipt));
    assert_eq!(
        resolved, registered.program_dir,
        "§8.2: `registered_install_dir` devuelve el del recibo, no el de `AVI_INSTALL_DIR`"
    );
    assert_ne!(
        resolved, bare.program_dir,
        "§8.2: y son distintos, que es justo lo que la operación tiene que ignorar"
    );
    assert_eq!(
        std::env::var("AVI_INSTALL_DIR").ok().as_deref(),
        Some(bare.program_dir.to_string_lossy().as_ref()),
        "§8.2: la reubicación por variable apunta al otro sitio, que es la trampa"
    );
}

/// `Env` de desinstalación tal como lo compone el binario en `handle_self`: el directorio
/// de programa y las raíces de datos y modelos salen del recibo, y solo el directorio de
/// temporales es el del sandbox.
///
/// El `temp_root` es la única diferencia con `cleanup::Roots::from_receipt`, y es
/// deliberada: el directorio de temporales del sistema es un parámetro para que las pruebas
/// no barren `%TEMP%` de la máquina que las ejecuta, que es el mismo motivo por el que §13
/// exige raíces reubicadas.
fn env_over_registered<'a>(
    registered: &'a Sandbox,
    receipt: &'a receipt::InstallReceipt,
) -> uninstall::Env<'a> {
    uninstall::Env {
        roots: cleanup::Roots {
            program_dir: receipt.install_dir.clone(),
            data_dir: receipt.roots.data_dir.clone(),
            models_dir: receipt.roots.cache_dir.clone(),
            temp_root: registered.temp_root.clone(),
            home: registered.home.clone(),
            models_shared: registered.shared_hub.is_some(),
        },
        program_dir: channel::registered_install_dir(Some(receipt)),
        receipt: Some(receipt),
        channel: Channel::Unmanaged,
        daemon_addr: "127.0.0.1:0".to_string(),
        home: registered.home.clone(),
    }
}

/// Lo que `self update` ve en cada canal (§8.2, Ciclo 2): `homebrew` y `dev` son los dos
/// que el brazo `Update` declara `externally_managed`, y `script` y `unmanaged` los que
/// siguen adelante. Es la misma detección de la prueba grande, ejercitada con las
/// entradas que ese brazo distingue; la negativa sin tocar nada es el criterio 15 en
/// `tests/update.rs`.
#[test]
fn update_detects_managed_channels() {
    let _guard = support::exclusively();

    let sandbox = Sandbox::new("canal-update");
    sandbox.seed_env();
    let script = sandbox.install_registered(PathIntegration::none());
    let cask_exe = sandbox
        .root
        .join("opt")
        .join("homebrew")
        .join("Caskroom")
        .join("ai-voice-interconnector");
    assert_eq!(
        channel::detect(&cask_exe, Some(&script)),
        Channel::Homebrew,
        "§8.2: `self update` ve `homebrew` bajo el prefijo, aunque haya recibo de `script`"
    );

    let dev_receipt = receipt::InstallReceipt::new(
        "0.23.1",
        avi_lifecycle::target::host_triple(),
        Channel::Dev,
        &sandbox.program_dir,
        vec![uninstall::executable_name_default()],
        PathIntegration::none(),
        receipt::Roots {
            data_dir: sandbox.data_dir.clone(),
            cache_dir: sandbox.models_dir.clone(),
        },
        None,
    );
    assert_eq!(
        channel::detect(
            &sandbox.program_dir.join("ai-voice-interconnector"),
            Some(&dev_receipt)
        ),
        Channel::Dev,
        "§8.2: `self update` ve `dev` cuando el recibo lo declara"
    );
    assert_eq!(
        channel::detect(
            &sandbox.program_dir.join("ai-voice-interconnector"),
            Some(&script)
        ),
        Channel::Script,
        "§8.2: y `script` sigue adelante"
    );
    assert_eq!(
        channel::detect(&sandbox.staging.join("ai-voice-interconnector"), None),
        Channel::Unmanaged,
        "§8.2: como `unmanaged` sin recibo"
    );
}
