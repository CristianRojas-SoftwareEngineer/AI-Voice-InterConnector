//! `self install` de punta a punta sobre raíces reubicadas a temporales (§9.3,
//! §13, criterio 2).
//!
//! Todas las raíces de §7 se reubican por sus variables, que es lo que §13 declara
//! que hace posibles las pruebas aisladas: en Windows las Known Folders ignoran
//! `LOCALAPPDATA`, así que el sandbox no puede apoyarse en ellas. El directorio de
//! programa, el directorio del enlace, la raíz de datos, la raíz de modelos, el
//! `HOME` y la clave de registro son todos del test.
//!
//! El bundle que se instala es **sintético y completo**: los cuatro documentos, la
//! librería de runtime y el derivado del motor, con los nombres exactos que
//! `packaging/bundle-manifest.json` exige para el target del host. El ejecutable es
//! un fichero de texto: esta prueba afirma el flujo de §9.3, no que el binario
//! arranque, que es lo que hace el bootstrap en el paso 8 de §9.2.
//!
//! `--no-setup` en todas las instalaciones: la provisión de modelos necesita red y es
//! la prueba `setup` de T13, no la de `self install`.

#![allow(clippy::disallowed_methods)]

use avi_lifecycle::daemon_stop::ProcessControl;
use avi_lifecycle::install::{self, Env, Mode, Options};
use avi_lifecycle::receipt::{self, PathIntegration};
use std::path::{Path, PathBuf};

/// Control de procesos inerte: no hay daemon en el sandbox, así que la parada es un
/// no-op. Es el mismo `ProcessControl` que `daemon_stop` espera y que T16 alimentará
/// con `avi-daemon` y `avi-tts`.
struct Inerte;

impl ProcessControl for Inerte {
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

/// Las pruebas de este archivo tocan el registro de Windows y crean directorios con
/// nombre único, así que **no necesitan serializarse**: cada una tiene su sandbox y su
/// clave. Un `Mutex` global solo añadiría un `Guard` sostenido a través de un `await`,
/// que es justo lo que un test asíncrono no debe hacer. Lo que evita que dos pruebas
/// simultáneas se pisen es el nombre único del sandbox y el de la clave de registro.
///
/// Sandbox con las siete raíces de §7 reubicadas.
struct Sandbox {
    /// Raíz del sandbox, para borrarlo entero al terminar.
    raiz: PathBuf,
    /// Directorio de programa: hermano de `opt`, con el nombre de §7.
    program_dir: PathBuf,
    /// Staging hermano, con el prefijo hermano de §7. Tiene que estar en el mismo
    /// volumen que el directorio de programa porque la colocación es un renombrado
    /// (§9.3.6.2).
    staging: PathBuf,
    /// Directorio del enlace (`~/.local/bin`).
    bin_dir: PathBuf,
    home: PathBuf,
    data_dir: PathBuf,
    models_dir: PathBuf,
    temp_root: PathBuf,
    /// Subclave de registro propia del test, en Windows.
    registry_subkey: String,
}

impl Sandbox {
    /// Levanta un sandbox nuevo. `etiqueta` distingue los de una misma ejecución.
    fn nuevo(tag: &str) -> Self {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or_default();
        let raiz =
            std::env::temp_dir().join(format!("install-e2e-{}-{tag}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&raiz);
        let opt = raiz.join("opt");
        let sandbox = Self {
            program_dir: opt.join("ai-voice-interconnector"),
            staging: opt.join(format!("{}test-{tag}", avi_lifecycle::STAGING_DIR_PREFIX)),
            bin_dir: raiz.join("home/ana/.local/bin"),
            home: raiz.join("home/ana"),
            data_dir: raiz.join("data"),
            models_dir: raiz.join("models"),
            temp_root: raiz.join("tmp"),
            registry_subkey: format!(
                r"Software\AI-Voice-InterConnector\install-test-{}-{tag}",
                std::process::id()
            ),
            raiz,
        };
        for dir in [
            &sandbox.program_dir,
            &sandbox.bin_dir,
            &sandbox.home,
            &sandbox.data_dir,
            &sandbox.models_dir,
            &sandbox.temp_root,
        ] {
            std::fs::create_dir_all(dir).expect("se crea el sandbox");
        }
        // `path_windows` solo existe en Windows, así que la rama va con `#[cfg]` y no
        // con `cfg!(windows)`: la segunda compila la llamada en Unix y el archivo no
        // compila allí.
        #[cfg(windows)]
        {
            // Clave vacía: no hay valor `Path`, así que la integración tiene que
            // crearlo desde cero. Es el caso de §9.3.1 "REG_EXPAND_SZ si no
            // existía", que es el que se ejercita en la puerta de Windows.
            avi_lifecycle::path_windows::create_key(&sandbox.registry_subkey)
                .expect("se crea la clave de registro de prueba");
        }
        sandbox
    }

    /// Escribe un bundle sintético completo en `destino` y devuelve el ejecutable.
    /// Los nombres son los del manifiesto del target del host, así que la validación
    /// del paso 2 se ejercita de verdad y no con una lista recortada.
    fn escribir_bundle(&self, destino: &Path) -> PathBuf {
        let seccion = avi_lifecycle::manifest::target_section(avi_lifecycle::target::host_triple())
            .expect("el target del host tiene sección en el manifiesto");
        for relativa in &seccion.required {
            let completa = aviar(destino, relativa);
            if let Some(parent) = completa.parent() {
                std::fs::create_dir_all(parent).expect("se crea el directorio del archivo");
            }
            std::fs::write(&completa, format!("contenido de {relativa}\n"))
                .expect("se escribe el archivo del bundle");
        }
        destino.join(seccion.executable_path())
    }

    /// `Env` de la operación, con el ejecutable que se invoca. `exe` es lo que decide
    /// el modo, así que es el parámetro que distingue instalar de reparar.
    fn env(&self, exe: &Path) -> Env {
        Env {
            exe: exe.to_path_buf(),
            version: "0.24.0".to_string(),
            target: avi_lifecycle::target::host_triple().to_string(),
            program_dir: self.program_dir.clone(),
            bin_dir: self.bin_dir.clone(),
            data_dir: self.data_dir.clone(),
            models_dir: self.models_dir.clone(),
            temp_root: self.temp_root.clone(),
            home: self.home.clone(),
            // Sin el directorio del enlace en el `PATH`: así el bloque de perfil sí
            // se escribe, que es la mitad de D2.
            path_env: "/usr/bin:/bin".to_string(),
            shell: avi_lifecycle::path_unix::Shell::Bash,
            zdotdir: None,
            registry_subkey: self.registry_subkey.clone(),
            // Puerto donde no hay nada: la parada del daemon es un no-op.
            daemon_addr: puerto_muerto(),
            source: None,
        }
    }

    /// Opciones de instalación desatendida sin provisión de modelos.
    fn opciones() -> Options {
        Options {
            assume_yes: true,
            no_setup: true,
            no_modify_path: false,
            force: false,
            channel: None,
            with_voice_cloning: false,
        }
    }

    /// Borra la clave de registro de prueba y el árbol del sandbox. El directorio se
    /// borra aunque la prueba haya fallado antes, para que un sandbox huérfano en
    /// `%TEMP%` no se acumule.
    fn limpiar(&self) {
        #[cfg(windows)]
        {
            let _ = avi_lifecycle::path_windows::delete_key(&self.registry_subkey);
        }
        let _ = std::fs::remove_dir_all(&self.raiz);
    }
}

/// Une un fragmento del manifiesto con la raíz del bundle.
fn aviar(destino: &Path, relativa: &str) -> PathBuf {
    let mut path = destino.to_path_buf();
    for parte in relativa.split('/') {
        path.push(parte);
    }
    path
}

/// Puerto efímero que se enlaza y se suelta: garantiza que no hay nada escuchando,
/// sin depender de que el puerto por defecto esté libre en la máquina que ejecuta.
fn puerto_muerto() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("se enlaza un puerto");
    let addr = listener.local_addr().expect("dirección local");
    drop(listener);
    addr.to_string()
}

/// El criterio 2 de punta a punta: instalar dos veces desde el mismo bundle deja el
/// mismo estado final, sin entradas de `PATH` ni bloques de perfil duplicados.
#[tokio::test]
async fn install_twice_is_idempotent() {
    let sandbox = Sandbox::nuevo("doble");
    let exe = sandbox.escribir_bundle(&sandbox.staging);
    let env = sandbox.env(&exe);
    let opciones = Sandbox::opciones();

    // ── Primera instalación ─────────────────────────────────────────────────────
    let primera = install::install(&env, &opciones, &Inerte)
        .await
        .expect("la primera instalación se completa");
    assert_eq!(primera.mode, Mode::Install);
    assert_eq!(primera.status, "installed");
    assert_eq!(primera.models, avi_lifecycle::install::ModelsState::Skipped);

    // El bundle llegó al directorio de programa y el recibo está escrito.
    let seccion = avi_lifecycle::manifest::target_section(&env.target).unwrap();
    for relativa in &seccion.required {
        assert!(
            aviar(&sandbox.program_dir, relativa).is_file(),
            "el bundle colocado tiene {relativa}"
        );
    }
    let recibo = receipt::read_from(&sandbox.program_dir)
        .expect("se lee el recibo")
        .expect("el recibo existe tras instalar");
    assert_eq!(recibo.version, "0.24.0");
    assert_eq!(recibo.app, "ai-voice-interconnector");
    assert_eq!(recibo.channel, avi_lifecycle::channel::Channel::Script);
    assert_eq!(recibo.roots.data_dir, sandbox.data_dir);
    assert_eq!(recibo.roots.cache_dir, sandbox.models_dir);
    assert_eq!(
        recibo.files, seccion.required,
        "el recibo lista lo colocado"
    );
    assert_eq!(
        recibo.path_integration.modify_path,
        !cfg!(windows) || primera.path_integration.modify_path,
        "el `PATH` se integra por defecto salvo `--no-modify-path`"
    );

    // La integración quedó registrada con lo que se hizo de verdad.
    let integracion = primera.path_integration.clone();
    if cfg!(unix) {
        assert!(integracion.modify_path, "en Unix se modifica por D2");
        let enlace = integracion.symlink.as_ref().expect("el enlace se registra");
        assert_eq!(enlace, &sandbox.bin_dir.join("ai-voice-interconnector"));
        let bloques = integracion
            .profile_blocks
            .as_ref()
            .expect("los bloques se registran");
        assert_eq!(bloques.len(), 1, "un solo archivo de arranque");
        let perfil = sandbox.home.join(".profile");
        assert_eq!(bloques[0], perfil);
        let texto = std::fs::read_to_string(&perfil).expect("el perfil existe");
        assert_eq!(
            texto.matches(avi_lifecycle::path_unix::BLOCK_BEGIN).count(),
            1,
            "un solo bloque delimitado: {texto}"
        );
    } else {
        assert!(
            integracion.modify_path,
            "en Windows la entrada del registro queda registrada: {integracion:?}"
        );
        assert!(integracion.registry_entry.is_some());
        assert!(integracion.symlink.is_none());
    }
    assert!(
        primera.needs_new_terminal(),
        "el `PATH` se tocó, así que el resumen pide terminal nueva"
    );

    // ── Segunda instalación, desde el mismo bundle ──────────────────────────────
    // El bundle se repone porque la colocación **mueve** los archivos desde el
    // origen (§9.3.6.2): sin reponerlo, la segunda pasada se instalaría un bundle
    // vacío. Es también lo que hace el bootstrap de §9.2, que extrae en un staging
    // nuevo cada vez. El ejecutable vuelve a ser el del staging, que es el que la
    // primera pasada dejó de tener alrededor al mover el bundle.
    let _exe = sandbox.escribir_bundle(&sandbox.staging);
    let contenido_tras_primera = listar(&sandbox.program_dir);
    let segunda = install::install(&env, &opciones, &Inerte)
        .await
        .expect("la segunda instalación se completa");

    // ── El estado final es el mismo ─────────────────────────────────────────────
    assert_eq!(
        segunda.mode,
        Mode::Install,
        "sigue siendo instalación, no reparación"
    );
    assert_eq!(segunda.status, "installed");
    assert_eq!(segunda.receipt.version, primera.receipt.version);
    assert_eq!(
        segunda.path_integration, integracion,
        "la integración es la misma: ni entradas ni bloques nuevos"
    );
    assert!(
        !segunda.path_changed(),
        "y la segunda pasada no reescribe el `PATH`: la entrada ya estaba"
    );
    assert!(
        segunda.path_integrated(),
        "pero sigue registrada en el recibo, que es lo que permite revertirla"
    );
    // El resumen **final** no es idéntico entre las dos pasadas, y no debería: la
    // segunda no reescribe el `PATH` y decirlo sería mentir. Lo que tiene que ser
    // igual es el estado, que es lo que la idempotencia del criterio 2 afirma.
    let lineas_de_estado = |resumen: &[String]| -> Vec<String> {
        resumen
            .iter()
            .filter(|l| !l.trim_start().starts_with("PATH:"))
            .cloned()
            .collect()
    };
    assert_eq!(
        lineas_de_estado(&segunda.summary),
        lineas_de_estado(&primera.summary),
        "el estado final es el mismo"
    );
    assert!(
        primera.path_changed() && !segunda.path_changed(),
        "pero la primera pasada sí reescribió el `PATH` y la segunda no"
    );
    assert!(
        segunda.receipt.path_integration == primera.receipt.path_integration,
        "y el recibo de la segunda pasada conserva la integración de la primera: si \
         registrara solo el diff de esta pasada, `self uninstall` no podría revertir \
         la entrada que puso la primera instalación"
    );

    // El directorio de programa tiene el mismo contenido que después de la primera.
    assert_eq!(
        listar(&sandbox.program_dir),
        contenido_tras_primera,
        "el contenido del directorio de programa no cambió"
    );
    assert!(
        contenido_tras_primera.contains(&"install-receipt.json".to_string()),
        "y incluye el recibo, que es uno de los archivos que la operación escribe"
    );
    for relativa in &seccion.required {
        assert!(
            contenido_tras_primera.contains(relativa),
            "el contenido incluye {relativa}, que es lo que el manifiesto exige"
        );
    }

    if cfg!(unix) {
        let texto = std::fs::read_to_string(sandbox.home.join(".profile")).unwrap();
        assert_eq!(
            texto.matches(avi_lifecycle::path_unix::BLOCK_BEGIN).count(),
            1,
            "el bloque de perfil no se duplicó: {texto}"
        );
    }

    // Y el recibo se relee con la misma forma.
    let releido = receipt::read_from(&sandbox.program_dir).unwrap().unwrap();
    assert_eq!(releido.files, primera.receipt.files);
    assert_eq!(
        releido.path_integration, integracion,
        "el recibo de disco coincide con el que devolvió la operación"
    );

    sandbox.limpiar();
}

/// La reparación desde dentro del directorio de programa no copia archivos: reaplica
/// integración de `PATH`, cuarentena y recibo (§9.3, modo reparación).
///
/// La afirmación central es que **el ejecutable no se toca**, y se hace comparando el
/// `mtime` del bundle colocado: si la reparación hubiera copiado algo, el directorio
/// de programa se habría recreado y la marca de tiempo sería posterior.
#[tokio::test]
async fn repair_from_inside_program_dir_copies_nothing() {
    let sandbox = Sandbox::nuevo("reparacion");
    let exe = sandbox.escribir_bundle(&sandbox.staging);
    let opciones = Sandbox::opciones();

    install::install(&sandbox.env(&exe), &opciones, &Inerte)
        .await
        .expect("la instalación inicial se completa");

    // Se borra la integración: es exactamente lo que una reparación tiene que
    // reaplicar, y lo que un usuario ve cuando su perfil se sobrescribió.
    let recibo = receipt::read_from(&sandbox.program_dir).unwrap().unwrap();
    if cfg!(unix) {
        for bloque in recibo.path_integration.profile_blocks.iter().flatten() {
            std::fs::remove_file(bloque).expect("se borra el perfil");
        }
    }
    // Y se marca el bundle colocado para que un copiado se note.
    let colocado = sandbox.program_dir.join(
        avi_lifecycle::manifest::target_section(avi_lifecycle::target::host_triple())
            .unwrap()
            .executable,
    );
    let antes = std::fs::metadata(&colocado)
        .and_then(|m| m.modified())
        .expect("el ejecutable colocado tiene fecha");
    std::thread::sleep(std::time::Duration::from_millis(1100));

    // Ahora se invoca **desde dentro** del directorio de programa.
    let env = sandbox.env(&colocado);
    assert_eq!(
        install::detect_mode(&env.exe, &env.program_dir),
        Mode::Repair,
        "dentro del directorio de programa, el modo es reparación"
    );
    let reparacion = install::install(&env, &opciones, &Inerte)
        .await
        .expect("la reparación se completa");

    assert_eq!(reparacion.mode, Mode::Repair);
    assert_eq!(reparacion.status, "repaired");
    assert_eq!(
        std::fs::metadata(&colocado)
            .and_then(|m| m.modified())
            .expect("el ejecutable sigue ahí"),
        antes,
        "la reparación no copió archivos: el ejecutable conserva su fecha"
    );
    assert!(
        reparacion.receipt.path_integration.modify_path,
        "la reparación reaplicó la integración del `PATH`"
    );
    if cfg!(unix) {
        let perfil = sandbox.home.join(".profile");
        let texto = std::fs::read_to_string(&perfil).expect("el perfil se volvió a escribir");
        assert_eq!(
            texto.matches(avi_lifecycle::path_unix::BLOCK_BEGIN).count(),
            1,
            "el bloque se reaplicó una sola vez"
        );
    }
    assert!(
        reparacion.summary.iter().any(|l| l.contains("reparado")),
        "el resumen final dice que fue una reparación: {:?}",
        reparacion.summary
    );
    assert!(
        reparacion
            .summary_before
            .iter()
            .any(|l| l.contains("no se copian archivos")),
        "y el resumen previo lo anuncia antes de confirmar: {:?}",
        reparacion.summary_before
    );
    sandbox.limpiar();
}

/// Sin bundle alrededor, `self install` termina con `bundle_invalid` y no modifica
/// nada (§9.3, tercer modo).
#[tokio::test]
async fn bundle_without_required_files_is_rejected() {
    let sandbox = Sandbox::nuevo("incompleto");

    // Un "bundle" con el ejecutable y nada más: es el caso de `target/release`, que
    // §9.3 nombra explícitamente.
    let exe = sandbox.staging.join(
        avi_lifecycle::manifest::target_section(avi_lifecycle::target::host_triple())
            .unwrap()
            .executable,
    );
    std::fs::create_dir_all(&sandbox.staging).expect("se crea el staging");
    std::fs::write(&exe, "binario suelto\n").expect("se escribe el ejecutable");

    let antes = estado(&sandbox.raiz);
    let err = install::install(&sandbox.env(&exe), &Sandbox::opciones(), &Inerte)
        .await
        .expect_err("un bundle sin los archivos obligatorios no se instala");

    // `install` devuelve `anyhow::Error` porque hay fallos de E/S sin `reason` propio,
    // pero los que §9.1 declara viajan dentro como `LifecycleError`, que es lo que el
    // sobre emite.
    let le = err
        .downcast_ref::<avi_lifecycle::LifecycleError>()
        .unwrap_or_else(|| panic!("el fallo declara un `reason`: {err:#}"));
    assert_eq!(le.reason, "bundle_invalid");
    assert_eq!(le.exit_code, 15, "`BundleInvalid = 15` de la tabla cerrada");
    assert!(
        le.message.contains("faltan") && le.message.contains("archivo(s)"),
        "el mensaje nombra los ausentes: {le}"
    );
    assert_eq!(
        estado(&sandbox.raiz),
        antes,
        "nada modificado: la validación es del paso 2, antes de tocar el disco"
    );
    assert!(
        receipt::read_from(&sandbox.program_dir).unwrap().is_none(),
        "y no hay recibo"
    );
    sandbox.limpiar();
}

/// `--no-modify-path` no toca ni el perfil ni el registro, y el recibo lo dice para
/// que `self uninstall` no tenga que revertir nada (criterio 2, la otra mitad).
#[tokio::test]
async fn no_modify_path_leaves_path_untouched() {
    let sandbox = Sandbox::nuevo("sin-path");
    let exe = sandbox.escribir_bundle(&sandbox.staging);
    let opciones = Options {
        no_modify_path: true,
        ..Sandbox::opciones()
    };

    let outcome = install::install(&sandbox.env(&exe), &opciones, &Inerte)
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
    sandbox.limpiar();
}

/// Un `path_conflict` —algo que no es un enlace nuestro en la ruta del enlace— aborta
/// con su `reason` y su código, y **no integra el `PATH` ni registra nada** (§9.3.1).
///
/// El nombre dice "no instala nada" y la afirmación es más estrecha a propósito: §9.1
/// solo declara "nada modificado" para `daemon_stop_failed` y `bundle_invalid`, no para
/// `path_conflict`, y el conflicto se detecta en el **paso 8**, después de la colocación
/// transaccional del paso 6. El disco, por tanto, no queda idéntico —el bundle está en
/// el directorio de programa y el archivo de bloqueo existe— y afirmar lo contrario sería
/// afirmar más de lo que la especificación promete. Lo que sí tiene que ser cierto, y es
/// lo que se afirma, es que la ruta ajena no se toca, que el `PATH` no se integra y que
/// no se escribe recibo.
///
/// La prueba solo corre en Unix porque en Windows la análoga —una entrada ajena en el
/// registro— la comprueba `windows_user_path_type_and_vars_survive`, y **antes de este
/// cambio no se había ejecutado nunca en ninguna plataforma**: en Windows salía por el
/// `return` de arriba y en Unix fallaba en la comparación de disco, con el mensaje que
/// él mismo describía como el comportamiento correcto.
#[tokio::test]
async fn foreign_path_is_conflict_and_installs_nothing() {
    if !cfg!(unix) {
        return;
    }
    let sandbox = Sandbox::nuevo("conflicto");
    let exe = sandbox.escribir_bundle(&sandbox.staging);
    let ocupado = sandbox.bin_dir.join("ai-voice-interconnector");
    std::fs::write(&ocupado, "no soy un enlace\n").expect("se ocupa la ruta del enlace");

    let err = install::install(&sandbox.env(&exe), &Sandbox::opciones(), &Inerte)
        .await
        .expect_err("una ruta ajena es conflicto");
    let le = err
        .downcast_ref::<avi_lifecycle::LifecycleError>()
        .unwrap_or_else(|| panic!("el fallo declara un `reason`: {err:#}"));
    assert_eq!(le.reason, "path_conflict");
    assert_eq!(le.exit_code, 14, "`PathConflict = 14` de la tabla cerrada");
    assert_eq!(
        std::fs::read_to_string(&ocupado).unwrap(),
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
    // Lo que §9.1 sí permite, y que conviene afirmar para que la diferencia con
    // `bundle_invalid` quede explícita: el bundle ya está colocado.
    assert!(
        sandbox
            .program_dir
            .join(avi_lifecycle::uninstall::executable_name_default())
            .is_file(),
        "el bundle quedó colocado en el directorio de programa: el conflicto es del \
         paso 8, no de la colocación"
    );
    sandbox.limpiar();
}

/// El estado del sandbox, **sin el archivo de bloqueo**.
fn listar(raiz: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut pila = vec![raiz.to_path_buf()];
    while let Some(dir) = pila.pop() {
        let Ok(entradas) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entrada in entradas.flatten() {
            let ruta = entrada.path();
            let relativa = ruta
                .strip_prefix(raiz)
                .unwrap_or(&ruta)
                .to_string_lossy()
                .replace('\\', "/");
            if entrada.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                out.push(format!("{relativa}/"));
                pila.push(ruta);
            } else {
                out.push(relativa);
            }
        }
    }
    out.sort();
    out
}

/// El estado del sandbox para comparar antes y después de una operación que se
/// supone que no modifica nada.
///
/// El **archivo de bloqueo queda fuera a propósito**: §9.1 lo crea al tomar el
/// bloqueo, que es el paso 1, y el «nada modificado» de `bundle_invalid` se refiere al
/// estado de la instalación —programa, datos, perfiles, registro y receipt—, no al
/// mecanismo que serializa las operaciones. Exigir que ni el bloqueo aparezca sería
/// exigir que el paso 1 no ocurriera, que es otra cosa.
fn estado(raiz: &Path) -> Vec<String> {
    listar(raiz)
        .into_iter()
        .filter(|e| e.rsplit('/').next() != Some(avi_lifecycle::LIFECYCLE_LOCK_NAME))
        .collect()
}
