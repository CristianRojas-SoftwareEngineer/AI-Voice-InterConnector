//! Los cuatro canales de §8.2 en las operaciones de este ciclo.
//!
//! La tabla de §8.2 tiene una columna por operación y una fila por canal, y casi todas las
//! celdas son de otros ciclos: `self update` es del Ciclo 2. Lo que este archivo demuestra
//! son las que **sí** son de C1, y son tres cosas distintas que conviene no mezclar:
//!
//! 1. **Cómo se origina cada canal.** La precedencia de §8.2 —Homebrew sobre el recibo, el
//!    recibo sobre `unmanaged`— y el hecho de que `self install` escriba el canal en el
//!    recibo y de que `--channel dev` (§10.5) solo surte efecto cuando el recibo se crea por
//!    primera vez.
//! 2. **Qué puede hacer cada canal.** `homebrew` es `externally_managed` con el comando
//!    correcto y sin tocar nada; `script`, `dev` y `unmanaged` pueden desinstalar.
//! 3. **Sobre qué opera la desinstalación.** §8.2 dice que `self uninstall` actúa siempre
//!    sobre la **instalación registrada**, sea cual sea la copia del binario que ejecute el
//!    comando. Es la propiedad que hace que `self uninstall` funcione desde `target/`, y es
//!    la que ninguna otra prueba del ciclo cubre de punta a punta.

#![allow(clippy::disallowed_methods)]

mod support;

use avi_lifecycle::channel::{self, Channel};
use avi_lifecycle::cleanup;
use avi_lifecycle::install::{self, Options as InstallOptions};
use avi_lifecycle::receipt::{self, PathIntegration};
use avi_lifecycle::uninstall;
use support::{Ahora, Inerte, Sandbox};

/// Opciones de instalación desatendida sin provisión de modelos y con `--channel dev`.
fn opciones_dev() -> InstallOptions {
    InstallOptions {
        channel: Some(Channel::Dev),
        ..Sandbox::opciones_instalacion()
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
    let sandbox = Sandbox::nuevo("canales");
    sandbox.sembrar_entorno();
    let caskroom = sandbox.raiz.join("opt").join("homebrew").join("Caskroom");
    let exe_del_cask = caskroom.join("ai-voice-interconnector");

    let script = sandbox.instalar_registrada(PathIntegration::none());
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
        channel::detect(&exe_del_cask, Some(&script)),
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
    let desde_staging = sandbox.escribir_bundle(&sandbox.staging);
    let instalada = runtime
        .block_on(install::install(
            &sandbox.env_instalacion(&desde_staging),
            &InstallOptions {
                channel: Some(Channel::Dev),
                ..Sandbox::opciones_instalacion()
            },
            &Inerte,
        ))
        .expect("§8.2: `self install` desde fuera de una instalación instala");
    assert_eq!(
        instalada.receipt.channel,
        Channel::Script,
        "§8.2: el valor por defecto es `script`"
    );

    // ── `--channel dev` solo cuando el recibo se crea por primera vez ──────────
    // Es la opción oculta de §10.5, y su regla es lo que hace que una instalación de
    // desarrollo siga siendo `dev` después de una reparación.
    let dev = Sandbox::nuevo("canal-dev");
    dev.sembrar_entorno();
    let exe = dev.escribir_bundle(&dev.staging);
    let primera = runtime
        .block_on(install::install(
            &dev.env_instalacion(&exe),
            &opciones_dev(),
            &Inerte,
        ))
        .expect("§8.2: `self install --channel dev` se completa");
    assert_eq!(
        primera.receipt.channel,
        Channel::Dev,
        "§8.2: `--channel dev` origina el canal `dev`"
    );
    assert_eq!(primera.receipt.channel.as_str(), "dev");

    let exe = dev.escribir_bundle(&dev.staging);
    let segunda = runtime
        .block_on(install::install(
            &dev.env_instalacion(&exe),
            &opciones_dev(),
            &Inerte,
        ))
        .expect("§8.2: la segunda pasada se completa");
    assert_eq!(
        segunda.receipt.channel,
        Channel::Dev,
        "§8.2: y una reparación conserva el canal, que es lo que hace que `self update` siga \
         tratando la instalación como `dev`"
    );
    let en_disco = receipt::read_from(&dev.program_dir)
        .expect("§8.2: se lee el recibo")
        .expect("§8.2: el recibo existe");
    assert_eq!(
        en_disco.channel,
        Channel::Dev,
        "§8.2: y el recibo de disco lo dice"
    );

    // ── 2. `homebrew` es `externally_managed` y no toca nada ────────────────────
    // Las dos operaciones de §8.2 que declaran el canal `homebrew` con comando propio.
    let hb = Sandbox::nuevo("canal-homebrew");
    hb.sembrar_entorno();
    hb.plantar_estado();
    let recibo_hb = hb.instalar_registrada(PathIntegration::none());
    let antes = hb.snapshot();

    let error = runtime
        .block_on(uninstall::run(
            &hb.env_uninstall(Some(&recibo_hb), Channel::Homebrew),
            &uninstall::Options {
                assume_yes: true,
                ..Default::default()
            },
            &Ahora,
            &Inerte,
        ))
        .expect_err("§8.2: `homebrew` no se desinstala desde aquí");
    let le = error
        .downcast_ref::<avi_lifecycle::LifecycleError>()
        .expect("§8.2: el fallo declara un `reason`");
    assert_eq!(le.reason, "externally_managed");
    assert_eq!(
        le.exit_code, 12,
        "`ExternallyManaged = 12` de la tabla cerrada"
    );
    assert!(
        le.message.contains(uninstall::HOMEBREW_UNINSTALL),
        "§8.2: el mensaje lleva el comando de Homebrew exacto: {}",
        le.message
    );
    assert!(
        le.message.contains("cleanup --all"),
        "§8.2: y sugiere `cleanup --all` para el estado de usuario"
    );
    assert_eq!(
        hb.snapshot(),
        antes,
        "§8.2: y nada se toca, ni programa, ni estado, ni modelos"
    );

    // `cleanup` sí opera en los cuatro canales: no depende del canal, y el estado de
    // usuario es del usuario.
    for canal in [
        Channel::Script,
        Channel::Dev,
        Channel::Homebrew,
        Channel::Unmanaged,
    ] {
        let s = Sandbox::nuevo(&format!("cleanup-{canal}"));
        s.sembrar_entorno();
        s.plantar_estado();
        s.instalar_registrada(PathIntegration::none());
        assert_eq!(s.env_uninstall(None, canal).channel, canal, "§8.2: {canal}");
        let outcome = runtime
            .block_on(cleanup::run(
                &s.roots(),
                &cleanup::Options {
                    all: true,
                    assume_yes: true,
                    ..Default::default()
                },
                &Inerte,
            ))
            .expect("§8.2: `cleanup` se ejecuta en cualquier canal");
        assert_eq!(outcome.status, "cleanup_complete", "§8.2: {canal}");
        assert!(
            !support::existe(&s.models_dir),
            "§8.2: {canal}: borra los modelos"
        );
        assert!(
            support::existe(&s.program_dir),
            "§8.2: {canal}: y nunca borra el programa"
        );
    }

    // ── 3. `self uninstall` opera sobre la instalación registrada ──────────────
    // La fila de §8.2 dice, para los cuatro canales, que `self uninstall` actúa sobre la
    // instalación registrada. La prueba lo monta al revés de lo habitual: el recibo apunta
    // a un directorio y la operación se invoca con `program_dir` de otro sitio, que es lo
    // que pasa cuando el comando corre desde `target/` o desde el bundle extraído a mano.
    let registrado = Sandbox::nuevo("registrado");
    registrado.sembrar_entorno();
    registrado.plantar_estado();
    let recibo_registrado = registrado.instalar_registrada(PathIntegration::none());

    let al_pie = Sandbox::nuevo("al-pie");
    al_pie.sembrar_entorno();
    al_pie.plantar_estado();
    al_pie.instalar_registrada(PathIntegration::none());

    let env = entorno_sobre_el_registrado(&registrado, &recibo_registrado);
    let outcome = runtime
        .block_on(uninstall::run(
            &env,
            &uninstall::Options {
                assume_yes: true,
                ..Default::default()
            },
            &Ahora,
            &Inerte,
        ))
        .expect("§8.2: la desinstalación se ejecuta");
    assert_eq!(outcome.status, "uninstalled");
    assert!(
        !support::existe(&registrado.program_dir),
        "§8.2: se borra la instalación **registrada**, que es la del recibo"
    );
    assert!(
        support::existe(&al_pie.program_dir),
        "§8.2: y la copia de la que se invoca el comando no se toca, porque no es la \
         instalación registrada"
    );
    assert!(
        support::existe(&al_pie.data_dir) && support::existe(&al_pie.models_dir),
        "§8.2: tampoco sus raíces, porque el recibo manda sobre dónde se resuelven ahora: es \
         lo que permite que la actualización y la desinstalación operen sobre las mismas \
         ubicaciones aunque la variable ya no esté definida"
    );
    assert!(
        registrado.program_dir != al_pie.program_dir,
        "§8.2: los dos directorios son distintos, que es lo que hace la prueba significativa"
    );

    // Y el directorio de programa sobre el que se opera sale del recibo, no de la
    // resolución por convención: con `AVI_INSTALL_DIR` apuntando al otro sitio, la
    // operación sigue intentando el registrado.
    let resuelta = channel::registered_install_dir(Some(&recibo_registrado));
    assert_eq!(
        resuelta, registrado.program_dir,
        "§8.2: `registered_install_dir` devuelve el del recibo, no el de `AVI_INSTALL_DIR`"
    );
    assert_ne!(
        resuelta, al_pie.program_dir,
        "§8.2: y son distintos, que es justo lo que la operación tiene que ignorar"
    );
    assert_eq!(
        std::env::var("AVI_INSTALL_DIR").ok().as_deref(),
        Some(al_pie.program_dir.to_string_lossy().as_ref()),
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
fn entorno_sobre_el_registrado<'a>(
    registrado: &'a Sandbox,
    recibo: &'a receipt::InstallReceipt,
) -> uninstall::Env<'a> {
    uninstall::Env {
        roots: cleanup::Roots {
            program_dir: recibo.install_dir.clone(),
            data_dir: recibo.roots.data_dir.clone(),
            models_dir: recibo.roots.cache_dir.clone(),
            temp_root: registrado.temp_root.clone(),
            home: registrado.home.clone(),
            models_shared: registrado.shared_hub.is_some(),
        },
        program_dir: channel::registered_install_dir(Some(recibo)),
        receipt: Some(recibo),
        channel: Channel::Unmanaged,
        daemon_addr: "127.0.0.1:0".to_string(),
        home: registrado.home.clone(),
    }
}
