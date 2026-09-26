//! Arnés común de las pruebas de aceptación de §15.
//!
//! §13 exige que la integración corra «con las raíces reubicadas a temporales», y §7
//! declara las cuatro variables de reubicación que lo hacen posible. En Windows las
//! Known Folders ignoran `LOCALAPPDATA`, así que un sandbox que se apoyara en ellas no
//! estaría probando el mecanismo de §7 sino el de la convención del sistema. Este
//! módulo hace las dos cosas a la vez, que es lo que hace falta para que la prueba
//! valga:
//!
//! 1. **Declara las cuatro variables** (`AVI_INSTALL_DIR`, `AVI_BIN_DIR`, `AVI_DATA_DIR`
//!    y `AVI_CACHE_DIR`) apuntando al sandbox, para que lo que el motor resuelve por el
//!    entorno —`ModelStore::new()` en los pasos 4 y 11 de §9.3, `ct2_model_dir()`—
//!    caiga dentro del sandbox y no en la máquina que ejecuta la puerta.
//! 2. **Pasa las mismas rutas como dato** en `install::Env`, `cleanup::Roots` y
//!    `uninstall::Env`, que es como las reciben en producción. Si las dos mitudes
//!    divergieran, la prueba estaría afirmando el sandbox y no el motor.
//!
//! La raíz de modelos tiene dos modos, y la diferencia no es cosmética: `AVI_CACHE_DIR`
//! **tiene precedencia** sobre `HF_HUB_CACHE` y `HF_HOME` en `avi-store`, así que el
//! modo compartido se declara **quitando** `AVI_CACHE_DIR` y aponiendo `HF_HUB_CACHE`.
//! Un sandbox que pusiera las dos cosas estaría probando la raíz exclusiva y declararía
//! `models_shared: true` sobre una ruta que R3 nunca protege.
//!
//! El prefijo del directorio del sandbox es deliberadamente neutro: los barridos de
//! §9.1 solo tocan entradas con los prefijos propios (`avi-`, `avi_`), y un sandbox
//! llamado `avi-…` se confundiría con un temporal del producto.

#![allow(dead_code)]
#![allow(clippy::disallowed_methods)]

use avi_lifecycle::channel::Channel;
use avi_lifecycle::cleanup;
use avi_lifecycle::daemon_stop::ProcessControl;
use avi_lifecycle::install;
use avi_lifecycle::receipt::{self, InstallReceipt, PathIntegration};
use avi_lifecycle::uninstall;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, MutexGuard};

/// Serializa las pruebas que tocan el entorno del proceso.
///
/// El entorno es global, así que dos pruebas que lo muten a la vez se contaminan. Es el
/// mismo argumento que el candado de `avi-store` y el `test_support` del crate, y
/// aplica también a las escrituras que hace el propio motor: `ModelStore::new()` fija
/// `HF_XET_CACHE` cuando la raíz de modelos es exclusiva, que es una escritura al
/// entorno aunque la prueba no la ordene.
pub static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Toma [`ENV_LOCK`], sin envenenarse si otra prueba lo dejó tomado al caer en pánico.
pub fn exclusively() -> MutexGuard<'static, ()> {
    ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Variables de entorno de §7 que el sandbox declara o borra según el modo.
const ROOT_VARS: [&str; 6] = [
    "AVI_INSTALL_DIR",
    "AVI_BIN_DIR",
    "AVI_DATA_DIR",
    "AVI_CACHE_DIR",
    "HF_HUB_CACHE",
    "HF_HOME",
];

/// Modo de la raíz de modelos: exclusiva de la aplicación (D3) o caché HF compartida.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Models {
    /// Exclusiva: `AVI_CACHE_DIR`, y `xet` y el `.locks` completo son nuestros.
    Exclusive,
    /// Compartida: `HF_HUB_CACHE`, y R3 limita el borrado a lo atribuible.
    Shared,
}

/// Sandbox con las siete raíces de §7 dentro de un directorio propio.
pub struct Sandbox {
    /// Raíz del sandbox, para borrarla entera al terminar.
    pub root: PathBuf,
    /// Etiqueta de la prueba. Va en el nombre del staging, así que el hijo sin
    /// terminal del criterio 19 necesita conocerla para reconstruir el sandbox.
    pub tag: String,
    /// Directorio de programa de §7.
    pub program_dir: PathBuf,
    /// Staging hermano, con el prefijo hermano de §7.
    pub staging: PathBuf,
    /// Directorio del enlace (`~/.local/bin`).
    pub bin_dir: PathBuf,
    pub home: PathBuf,
    pub data_dir: PathBuf,
    /// Raíz de modelos. En modo compartido es la caché HF que eligió el usuario.
    pub models_dir: PathBuf,
    /// `Some` cuando la raíz de modelos es la caché HF compartida del usuario.
    pub shared_hub: Option<PathBuf>,
    /// Directorio de temporales del sistema, en el papel que tendría en producción.
    pub temp_root: PathBuf,
    /// Subclave de registro propia del test, en Windows.
    pub registry_subkey: String,
    /// `false` cuando el sandbox no es suyo y no debe borrarse al soltarlo. Es lo que
    /// necesita el proceso hijo del criterio 19, que reconstruye el sandbox del padre
    /// para poder negarse a borrarlo sin montarlo de nuevo.
    pub remove_on_drop: bool,
}

impl Sandbox {
    /// Levanta un sandbox nuevo con la raíz de modelos en modo exclusivo.
    pub fn new(tag: &str) -> Self {
        Self::new_with(tag, Models::Exclusive)
    }

    /// Levanta un sandbox nuevo eligiendo el modo de la raíz de modelos.
    pub fn new_with(tag: &str, mode: Models) -> Self {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or_default();
        let root = std::env::temp_dir().join(format!("criterio-{tag}-{}-{n}", std::process::id()));
        let sandbox = Self::built(tag, root, mode);
        sandbox.create_directories();
        sandbox
    }

    /// Reconstruye un sandbox a partir de su raíz y su etiqueta, que es lo que
    /// necesita el proceso hijo del criterio 19: el hijo no puede sortear otro
    /// directorio, porque el padre tiene que comprobar **ese** disco.
    pub fn from_root(tag: &str, root: PathBuf, mode: Models) -> Self {
        Self::built(tag, root, mode)
    }

    /// Deriva todas las rutas de la etiqueta y de la raíz. Todo lo que depende del
    /// instante de creación vive en `root`, así que reconstruir es determinista.
    fn built(tag: &str, root: PathBuf, mode: Models) -> Self {
        let opt = root.join("opt");
        let home = root.join("home").join("ana");
        let hub = root.join("hub");
        let (models_dir, shared_hub) = match mode {
            Models::Exclusive => (root.join("models"), None),
            Models::Shared => (hub.clone(), Some(hub)),
        };
        Self {
            staging: opt.join(format!("{}test-{tag}", avi_lifecycle::STAGING_DIR_PREFIX)),
            program_dir: opt.join(avi_lifecycle::APP_NAME),
            bin_dir: home.join(".local").join("bin"),
            data_dir: root.join("data"),
            models_dir,
            shared_hub,
            temp_root: root.join("tmp"),
            home,
            registry_subkey: format!(
                r"Software\AI-Voice-InterConnector\criterion-{}-{tag}",
                std::process::id()
            ),
            root,
            tag: tag.to_string(),
            remove_on_drop: true,
        }
    }

    /// Devuelve el sandbox sin derecho a borrar su árbol. Lo usa el proceso hijo del
    /// criterio 19, que opera sobre el disco del padre y tiene que dejarlo intacto para
    /// que el padre pueda compararlo.
    pub fn borrowed(mut self) -> Self {
        self.remove_on_drop = false;
        self
    }

    /// Crea **solo** los directorios que existen en una máquina donde la aplicación no
    /// está instalada: el padre del programa, `$HOME`, el directorio del enlace y el de
    /// temporales.
    ///
    /// El directorio de programa, la raíz de datos y la raíz de modelos **no** se crean,
    /// y es deliberado: `uninstall::run` decide entre `not_installed` y una
    /// desinstalación vacía mirando si el estado existe, y un sandbox que llegara con esas
    /// raíces vacías afirmaría un caso que en producción no existe. Las pruebas que las
    /// necesitan las crean al plantar el estado o al instalar.
    pub fn create_directories(&self) {
        for dir in [
            self.program_dir
                .parent()
                .expect("el directorio de programa tiene padre"),
            &self.home,
            &self.bin_dir,
            &self.temp_root,
        ] {
            std::fs::create_dir_all(dir).expect("se crea el sandbox");
        }
        #[cfg(windows)]
        avi_lifecycle::path_windows::create_key(&self.registry_subkey)
            .expect("se crea la clave de registro de prueba");
    }

    /// Declara —o borra— en el entorno del proceso las variables de §7 que este sandbox
    /// representa. Sin esto, lo que el motor resuelve por entorno saldría de la máquina
    /// que ejecuta la puerta.
    ///
    /// Requiere [`ENV_LOCK`]. En modo compartido **borra** `AVI_CACHE_DIR`, porque en
    /// `avi-store` tiene precedencia sobre `HF_HUB_CACHE` y su presencia volvería
    /// exclusiva una raíz que el test declara compartida.
    pub fn seed_env(&self) {
        let value = |p: &Path| p.display().to_string();
        match &self.shared_hub {
            Some(hub) => {
                std::env::remove_var("AVI_CACHE_DIR");
                std::env::set_var("HF_HUB_CACHE", value(hub));
                std::env::remove_var("HF_HOME");
            }
            None => {
                std::env::set_var("AVI_CACHE_DIR", value(&self.models_dir));
                std::env::remove_var("HF_HUB_CACHE");
                std::env::remove_var("HF_HOME");
            }
        }
        std::env::set_var("AVI_INSTALL_DIR", value(&self.program_dir));
        std::env::set_var("AVI_BIN_DIR", value(&self.bin_dir));
        std::env::set_var("AVI_DATA_DIR", value(&self.data_dir));
    }

    /// Borra las seis variables de §7. Lo llama [`Self::limpiar`].
    pub fn clear_env() {
        for variable in ROOT_VARS {
            std::env::remove_var(variable);
        }
    }

    /// Escribe un bundle sintético **completo** en `destino`, con los nombres exactos
    /// que `packaging/bundle-manifest.json` exige para el target del host, y devuelve
    /// el ejecutable. La lista no se escribe a mano: si el manifiesto cambiara, la
    /// seguiría la validación del paso 2 de §9.3.
    pub fn write_bundle(&self, dest: &Path) -> PathBuf {
        let section = avi_lifecycle::manifest::target_section(avi_lifecycle::target::host_triple())
            .expect("el target del host tiene sección en el manifiesto");
        for relative in &section.required {
            let complete = place(dest, relative);
            write(&complete, &format!("contenido de {relative}\n"));
        }
        dest.join(section.executable_path())
    }

    /// `Env` de instalación con las raíces del sandbox. `exe` decide el modo, así que
    /// es el parámetro que distingue instalar de reparar.
    pub fn install_env(&self, exe: &Path) -> install::Env {
        install::Env {
            exe: exe.to_path_buf(),
            version: "0.24.0".to_string(),
            target: avi_lifecycle::target::host_triple().to_string(),
            program_dir: self.program_dir.clone(),
            bin_dir: self.bin_dir.clone(),
            data_dir: self.data_dir.clone(),
            models_dir: self.models_dir.clone(),
            temp_root: self.temp_root.clone(),
            home: self.home.clone(),
            // Sin el directorio del enlace en el `PATH`: así el bloque de perfil sí se
            // escribe, que es la mitad de D2.
            path_env: "/usr/bin:/bin".to_string(),
            shell: avi_lifecycle::path_unix::Shell::Bash,
            zdotdir: None,
            registry_subkey: self.registry_subkey.clone(),
            // Puerto donde no hay nada: la parada del daemon es un no-op.
            daemon_addr: dead_port(),
            source: None,
        }
    }

    /// Opciones de una instalación desatendida sin provisión de modelos.
    pub fn install_options() -> install::Options {
        install::Options {
            assume_yes: true,
            no_setup: true,
            no_modify_path: false,
            force: false,
            channel: None,
            with_voice_cloning: false,
        }
    }

    /// `Roots` de `cleanup` y de `self uninstall`.
    pub fn roots(&self) -> cleanup::Roots {
        cleanup::Roots {
            program_dir: self.program_dir.clone(),
            data_dir: self.data_dir.clone(),
            models_dir: self.models_dir.clone(),
            temp_root: self.temp_root.clone(),
            home: self.home.clone(),
            models_shared: self.shared_hub.is_some(),
        }
    }

    /// `Env` de desinstalación. El directorio de programa lo da el recibo si lo hay,
    /// que es §8.2: la operación actúa sobre la instalación registrada, no sobre la
    /// posición del ejecutable.
    pub fn env_uninstall<'a>(
        &self,
        receipt: Option<&'a InstallReceipt>,
        channel: Channel,
    ) -> uninstall::Env<'a> {
        uninstall::Env {
            roots: self.roots(),
            program_dir: receipt
                .map(|r| r.install_dir.clone())
                .unwrap_or_else(|| self.program_dir.clone()),
            receipt,
            channel,
            daemon_addr: "127.0.0.1:0".to_string(),
            home: self.home.clone(),
        }
    }

    /// Planta el estado de §9.6 completo y, además, los recursos **compartidos** que el
    /// criterio 23 nombra: un repo de otra herramienta dentro de la caché de modelos,
    /// `~/.cargo` y el directorio de `sccache`.
    ///
    /// Los locks se plantan dos veces a propósito: el del repo propio, que R3 declara
    /// atribuible a la aplicación, y el de otro, que no. La distinción es el contenido
    /// del criterio, y sin los dos directorios no se puede ver.
    pub fn seed_state(&self) {
        let hub = &self.models_dir;
        for (name, repo, rev) in avi_store::MODEL_REVISIONS {
            if *name == avi_lifecycle::setup::CLONING_MODEL {
                continue;
            }
            // Snapshot propio, con el layout que `avi-store` resuelve.
            write(
                &hub.join(repo_dir(repo))
                    .join("snapshots")
                    .join(rev)
                    .join("pesos.bin"),
                "pesos",
            );
            // Y su lock, que R3 declara atribuible a la aplicación.
            write(&hub.join(".locks").join(repo_dir(repo)).join("lock"), "");
        }
        // El derivado CT2, que es atribuible aunque no cuelgue de un repo.
        write(
            &hub.join("ct2").join("marian-es-en").join("model.bin"),
            "ct2",
        );
        // `xet` y el `.locks` completo: globales de la caché, nunca nuestros.
        write(&hub.join("xet").join("shard"), "xet");
        write(
            &hub.join(".locks")
                .join("models--otra--herramienta")
                .join("lock"),
            "",
        );
        // Y el repo de otra herramienta, con su lock propio.
        write(
            &hub.join("models--otra--herramienta")
                .join("otro.safetensors"),
            "ajeno",
        );
        // Estado de usuario: voces de fábrica y de usuario, habla, configuración, logs
        // y pidfile.
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
        write(&self.data_dir.join("config.json"), "{}");
        write(&self.data_dir.join("logs").join("daemon.log"), "log");
        write(&self.data_dir.join("daemon.pid"), "{}");
        // Recursos compartidos del entorno, que ninguna operación puede borrar
        // (criterio 23). `sccache` se planta en dos sitios porque son los dos que
        // existen en producción: la caché del usuario y el temporal del sistema.
        write(
            &self.home.join(".cargo").join("registry").join("indice"),
            "carga",
        );
        write(
            &self.home.join(".cache").join("sccache").join("objeto"),
            "sccache",
        );
        write(&self.temp_root.join("sccache").join("objeto"), "sccache");
    }

    /// Un temporal propio huérfano, que §9.6 obliga a barrer, plantado junto a los
    /// compartidos del párrafo anterior: es el contraste que demuestra que el barrido es
    /// selectivo por prefijo y no por directorio.
    pub fn seed_own_temp(&self) -> PathBuf {
        let temp = self.temp_root.join("avi-huerfano.tmp");
        write(&temp, "temporal propio");
        temp
    }

    /// Recibo de una instalación registrada, con las raíces del sandbox.
    pub fn receipt(&self, integration: PathIntegration) -> InstallReceipt {
        InstallReceipt::new(
            "0.24.0",
            avi_lifecycle::target::host_triple(),
            Channel::Script,
            &self.program_dir,
            vec![uninstall::executable_name_default()],
            integration,
            receipt::Roots {
                data_dir: self.data_dir.clone(),
                cache_dir: self.models_dir.clone(),
            },
            None,
        )
    }

    /// Instala sin pasar por `self install`: escribe el ejecutable y el recibo de §8.1.
    /// Es lo que necesitan las pruebas de desinstalación y limpieza, cuyo objeto es la
    /// desinstalación y no la colocación.
    pub fn install_registered(&self, integration: PathIntegration) -> InstallReceipt {
        let receipt = self.receipt(integration);
        write(
            &self.program_dir.join(uninstall::executable_name_default()),
            "binario",
        );
        receipt::write_to(&receipt, &self.program_dir).expect("se escribe el recibo");
        receipt
    }

    /// Estado del sandbox como `(ruta relativa, tamaño)`, para afirmar que una
    /// operación no ha modificado el disco.
    pub fn snapshot(&self) -> Vec<(String, u64)> {
        let mut out = Vec::new();
        let mut stack = vec![self.root.clone()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let meta = match entry.metadata() {
                    Ok(meta) => meta,
                    Err(_) => continue,
                };
                let relative = path
                    .strip_prefix(&self.root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/");
                if meta.is_dir() {
                    out.push((relative, 0));
                    stack.push(path);
                } else {
                    out.push((relative, meta.len()));
                }
            }
        }
        out.sort();
        out
    }

    /// Contenido del sandbox como rutas relativas, incluidos los directorios. Es lo que
    /// se usa para afirmar el **residuo cero** del criterio 17, donde importa que un
    /// directorio vacío también se ha ido.
    pub fn content(&self) -> Vec<String> {
        list(&self.root)
    }

    /// Entradas del directorio de programa, ordenadas, para el residuo cero.
    pub fn program_content(&self) -> Vec<String> {
        list(&self.program_dir)
    }

    /// Borra la clave de registro de prueba, restaura el entorno y elimina el árbol del
    /// sandbox. Es idempotente, para que se pueda llamar a mano y también desde `Drop`.
    ///
    /// Un sandbox en préstamo no borra su árbol: es el proceso padre quien lo creó y
    /// quien tiene que encontrarlo después.
    pub fn clear(&self) {
        #[cfg(windows)]
        {
            let _ = avi_lifecycle::path_windows::delete_key(&self.registry_subkey);
        }
        if self.remove_on_drop {
            Self::clear_env();
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        self.clear();
    }
}

/// Nombre del directorio de repo en la caché de HuggingFace, tal y como lo construye
/// `avi-store`.
pub fn repo_dir(repo: &str) -> String {
    format!("models--{}", repo.replace('/', "--"))
}

/// Control de procesos inerte: no hay daemon en el sandbox, así que la parada es un
/// no-op. Es el mismo `ProcessControl` que `daemon_stop` espera y que el binario
/// alimenta con `avi-daemon` y `avi-tts`.
pub struct Inert;

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

/// Removedor que borra ya. El caso diferido de Windows es el de §9.5, paso 8, y no es
/// lo que se ejercita aquí.
pub struct Now;

impl uninstall::ProgramDirRemover for Now {
    fn exe_lives_inside(&self, _program_dir: &Path) -> bool {
        false
    }
    fn remove_now(&self, program_dir: &Path) -> anyhow::Result<()> {
        std::fs::remove_dir_all(program_dir)?;
        Ok(())
    }
    fn schedule(&self, _program_dir: &Path, _pid: u32) -> anyhow::Result<bool> {
        Ok(false)
    }
}

/// Runtime de un solo hilo: las operaciones del motor son asíncronas pero no
/// necesitan concurrencia, y un runtime por prueba evita el coste y el
/// `block_on` anidado de un runtime global.
pub fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime de la prueba")
}

/// Escribe un fichero, creando los directorios intermedios.
pub fn write(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("se crea el directorio padre");
    }
    std::fs::write(path, content).expect("se escribe el fichero");
}

/// Une un fragmento del manifiesto con la raíz del bundle.
pub fn place(dest: &Path, relative: &str) -> PathBuf {
    let mut path = dest.to_path_buf();
    for part in relative.split('/') {
        path.push(part);
    }
    path
}

/// `true` si la ruta existe, incluido como enlace roto.
pub fn exists(path: &Path) -> bool {
    path.exists() || path.symlink_metadata().is_ok()
}

/// Contenido de un directorio, con `/` al final en los subdirectorios.
pub fn list(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                out.push(format!("{relative}/"));
                stack.push(path);
            } else {
                out.push(relative);
            }
        }
    }
    out.sort();
    out
}

/// Puerto efímero que se enlaza y se suelta: garantiza que no hay nada escuchando,
/// sin depender de que el puerto por defecto esté libre en la máquina que ejecuta.
pub fn dead_port() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("se enlaza un puerto");
    let addr = listener.local_addr().expect("dirección local");
    drop(listener);
    addr.to_string()
}

/// Rutas del plan de borrado, como cadenas, que es la forma en que el sobre las
/// publica.
pub fn paths(plan: &cleanup::DeletionPlan) -> Vec<String> {
    plan.targets
        .iter()
        .map(|t| t.path.display().to_string())
        .collect()
}

/// Rutas de las entradas de un plan de desinstalación, con su tamaño.
pub fn entries(plan: &[avi_lifecycle::confirm::PlanEntry]) -> Vec<String> {
    plan.iter().map(|e| e.path.display().to_string()).collect()
}

// ─── El criterio 19 y la ausencia de terminal ──────────────────────────────────────
//
// `uninstall::run` y `cleanup::run` leen `stdin_is_terminal` del **stdin real** del
// proceso, con `std::io::IsTerminal`. No es un parámetro, así que no hay forma de
// recorrer la celda «sin terminal» de la tabla de §9.1 sin cambiar el entorno de la
// puerta: si el proceso se lanza desde una consola interactiva, `stdin` es una terminal
// y la operación pediría confirmación en vez de negarse. Afirmar lo contrario en un
// proceso con terminal sería pasar la prueba por el motivo equivocado.
//
// La salida es un proceso hijo: el mismo binario de prueba reejecutado con la entrada
// redirigida a la null. No es una prueba aparte con nombre propio —la marca se lee de
// una variable, así que la ejecución directa es la del padre— y el padre exige que el
// hijo termine en éxito, de modo que el trabajo nunca se omite.

/// Variable que marca este proceso como el hijo sin terminal.
pub const MARK_WITHOUT_TERMINAL: &str = "AVI_LIFECYCLE_HIJO_SIN_TERMINAL";
/// Variable con la raíz del sandbox que el hijo debe reconstruir.
pub const VAR_ROOT: &str = "AVI_LIFECYCLE_RAIZ";
/// Variable con la etiqueta del sandbox que el hijo debe reconstruir.
pub const VAR_TAG: &str = "AVI_LIFECYCLE_TAG";
/// Prefijo de las líneas con las que el hijo informa al padre.
pub const REPORT_PREFIX: &str = "AVI_LIFECYCLE_INFORME: ";

/// `true` si este proceso es el hijo sin terminal del criterio 19.
pub fn is_child_without_terminal() -> bool {
    std::env::var_os(MARK_WITHOUT_TERMINAL).is_some()
}

/// Reejecuta `nombre_test` en un proceso hijo con `stdin` redirigido a la null.
///
/// El hijo hereda el entorno, incluida la marca, y recibe la raíz y la etiqueta del
/// sandbox del padre. `--nocapture` es lo que deja la salida del hijo en su `stdout`
/// real, que es por donde el padre lee el informe.
pub fn run_without_terminal(test_name: &str, sandbox: &Sandbox) -> std::process::Output {
    Command::new(std::env::current_exe().expect("la ruta del binario de prueba"))
        .args(["--exact", test_name, "--nocapture", "--test-threads=1"])
        .env(MARK_WITHOUT_TERMINAL, "1")
        .env(VAR_ROOT, &sandbox.root)
        .env(VAR_TAG, &sandbox.tag)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("se reejecuta la prueba sin terminal")
}

/// Línea de informe del hijo.
pub fn report_line(line: &str) {
    println!("{REPORT_PREFIX}{line}");
}

/// Líneas de informe del hijo que el padre lee de su salida.
///
/// `libtest` imprime `test <nombre> ... ` **sin salto de línea** antes de la salida de la
/// prueba, así que la primera línea de informe viene pegada a ese prefijo. Por eso se
/// busca el marcador en cualquier punto de la línea y se toma lo que viene detrás, en vez
/// de exigir que la línea empiece por él.
pub fn read_report_lines(stdout: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(stdout)
        .lines()
        .filter_map(|line| line.split_once(REPORT_PREFIX))
        .map(|(_, rest)| rest.trim().to_string())
        .collect()
}
