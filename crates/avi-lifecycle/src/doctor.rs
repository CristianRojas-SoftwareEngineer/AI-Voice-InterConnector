//! Sección de ciclo de vida de `doctor`, como datos y no como texto.
//!
//! `doctor` es la única operación que el usuario puede ejecutar sin riesgo, y por eso
//! es donde el estado del ciclo de vida tiene que ser legible: versión y target, canal,
//! instalación y recibo, `PATH` con duplicados y precedencia, pendientes de transacción
//! y modelos.
//!
//! **Por qué este módulo no imprime nada.** El contrato de la CLI exige que cada
//! invocación emita **exactamente un objeto JSON**, y que la salida por veredicto de
//! `doctor` —código ≠ 0 con el reporte ya emitido y **sin** objeto `error`— no lleve un
//! segundo objeto detrás. Eso solo es posible si el veredicto es un **dato** que el
//! binario compone, y no una impresión con un `exit` detrás. Aquí no hay `println!` ni
//! `exit`: hay una función que devuelve el reporte, serializable, con su veredicto
//! dentro.
//!
//! **Las claves que se retiran.** El contrato niega cuatro claves de primer nivel del
//! reporte —la ruta de la caché, la del directorio de datos, el estado del modelo opt-in
//! de clonado y la lista de problemas— y el sobre las cubre con `install`, `path` y
//! `models`. Aquí no se emiten, **ni siquiera como nombre**: la prueba de este módulo
//! afirma el **conjunto exacto** de claves del sobre en vez de una lista de
//! prohibidas, que es una afirmación más fuerte y no necesita nombrarlas. La
//! información no se pierde, cambia de sitio: la raíz de datos es el campo `data_dir` de
//! `install`, el estado del modelo opt-in de clonado es el campo `base` de `models` con
//! sus mismos dos valores, y las comprobaciones que eran la lista de problemas son
//! `checks` y `failed`.
//!
//! **Recuperación en modo informe.** La regla dice que `doctor` ejecuta la recuperación
//! al
//! empezar "en modo informe". Eso significa que **calcula** lo que la recuperación
//! haría —con [`crate::recovery::preview`], la misma decisión que usa el barrido real— y
//! lo publica en `pending`, **sin tocar nada**: sin tomar el bloqueo y sin modificar el
//! programa. Barrer de verdad desde un diagnóstico convertiría el comando más inocuo del
//! producto en uno que borra temporales de la máquina que lo invoca.

use crate::channel::{self, Channel};
use crate::cleanup;
use crate::receipt::{self, InstallReceipt, PathIntegration};
use crate::recovery::{self, Roots as RecoveryRoots};
use serde::Serialize;
use std::path::{Path, PathBuf};

/// Entorno que informa `doctor`: las raíces del producto y lo que hace falta para juzgar
/// la
/// integración de `PATH`.
///
/// Reutiliza [`cleanup::Roots`] en vez de declarar otro juego de raíces: la regla exige
/// una
/// sola fuente, y duplicar la estructura es el primer paso de duplicar la resolución.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Env {
    pub roots: cleanup::Roots,
    /// `PATH` de la sesión, tal como lo ve el proceso.
    pub path_env: String,
    /// Subclave de registro donde se integra el `PATH` en Windows. Vacía en Unix.
    pub registry_subkey: String,
}

impl Env {
    /// Entorno con las raíces ya resueltas, que es lo que usa el binario.
    pub fn resolve() -> Self {
        Self {
            roots: cleanup::Roots::resolve(),
            path_env: std::env::var("PATH").unwrap_or_default(),
            registry_subkey: registry_subkey_default(),
        }
    }

    /// Entorno de la instalación registrada, con el recibo como fuente de verdad de
    /// las raíces, del recibo si lo hay.
    pub fn from_receipt(receipt: Option<&InstallReceipt>) -> Self {
        Self {
            roots: cleanup::Roots::from_receipt(receipt),
            ..Self::resolve()
        }
    }

    fn recovery_roots(&self) -> RecoveryRoots<'_> {
        RecoveryRoots {
            program_dir: &self.roots.program_dir,
            temp_root: &self.roots.temp_root,
            in_use: None,
        }
    }
}

#[cfg(windows)]
fn registry_subkey_default() -> String {
    crate::path_windows::ENV_SUBKEY.to_string()
}

#[cfg(not(windows))]
fn registry_subkey_default() -> String {
    String::new()
}

/// Fila `install` del reporte: directorio de programa, raíz de datos efectiva y estado
/// del
/// recibo.
///
/// La raíz de datos vive **aquí** y no como clave de primer nivel: es el sitio donde el
/// contrato la coloca y donde espera que esté tras retirar `data_dir`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Install {
    /// Directorio de programa, que es el del recibo si lo hay.
    pub dir: PathBuf,
    /// Raíz de datos efectiva, del recibo si lo hay.
    pub data_dir: PathBuf,
    /// `valid` o `absent`, que es como el sobre nombra el estado del recibo.
    pub receipt: &'static str,
    /// Versión instalada, si el recibo la declara.
    pub version: Option<String>,
}

/// Una instalación coexistente con la registrada, y cuál tiene precedencia en el
/// `PATH`: si conviven dos instalaciones, `doctor` lo informa junto con cuál
/// tiene precedencia en el PATH.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Coexistence {
    pub path: PathBuf,
    pub channel: Channel,
    /// `true` si esta instalación es la que resuelve el comando.
    pub takes_precedence: bool,
}

/// Fila `path` del reporte.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PathState {
    /// `true` si el comando resuelve a la instalación registrada.
    pub resolves_to_this_install: bool,
    /// Entradas del `PATH` que apuntan a una instalación de la aplicación; más de una
    /// es el duplicado que hay que detectar.
    pub duplicate_entries: Vec<String>,
    /// `present`, `absent` o `not_modified` (`--no-modify-path`).
    pub integration: &'static str,
    /// Instalaciones que compiten con la registrada, con su precedencia.
    pub coexisting: Vec<Coexistence>,
}

///
/// Fila `pending` del reporte: diario de transacción, aparcados, stagings huérfanos y
/// temporales propios huérfanos.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Pending {
    pub transaction_journal: bool,
    pub parked: Vec<String>,
    pub stagings: Vec<String>,
    pub temporaries: Vec<String>,
    /// Temporales de un proceso vivo: no se pueden decidir y no son un residuo.
    pub temporaries_kept: Vec<String>,
}

impl Pending {
    /// `true` si no queda nada pendiente. Es lo que un `doctor` sano afirma.
    pub fn is_clean(&self) -> bool {
        !self.transaction_journal
            && self.parked.is_empty()
            && self.stagings.is_empty()
            && self.temporaries.is_empty()
    }
}

/// Fila `models` del reporte: provisionados, faltantes y con tamaños.
///
/// El estado del modelo opt-in de clonado es el campo `base` de esta fila y no una clave
/// del sobre: es un dato de modelos, no un veredicto aparte. Se llama así, y no como la
/// clave plana que el contrato retira, para que el sobre no vuelva a exponer un nombre
/// que el contrato niega.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Models {
    pub root: PathBuf,
    /// `true` si la raíz es la caché HF compartida que eligió el usuario, lo que hace
    /// que R3 limite el alcance de cualquier borrado.
    pub shared_root: bool,
    pub provisioned: Vec<String>,
    pub missing: Vec<String>,
    /// Estado del modelo Base de clonado: `ready` o `missing_opt_in`.
    pub base: &'static str,
    /// Pares de traducción cuyo derivado CT2 no pasa el gate.
    pub ct2_incomplete: Vec<String>,
    pub size_bytes: u64,
}

/// Una comprobación con su veredicto. Es la clave `checks` del contrato.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Check {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

/// Reporte de `doctor`: las siete claves del sobre más `checks` y `failed`.
///
/// Es serializable y **no** lleva `status`: el veredicto son `checks` y `failed`, y el
/// código de salida lo decide el binario conservando el 1 del contrato, sin adjuntar un
/// objeto `error` detrás.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Report {
    pub version: String,
    pub target: String,
    pub channel: Channel,
    pub install: Install,
    pub path: PathState,
    pub pending: Pending,
    pub models: Models,
    pub checks: Vec<Check>,
    /// Comprobaciones que fallan, que es lo que el contrato llama `failed`.
    pub failed: Vec<String>,
}

impl Report {
    /// Veredicto del contrato: `true` si alguna comprobación falló.
    pub fn is_failure(&self) -> bool {
        !self.failed.is_empty()
    }
}

/// Informa del ciclo de vida y devuelve el veredicto como dato.
///
/// `exe` es el ejecutable que se está ejecutando: lo necesitan la detección de canal y
/// la de precedencia en el `PATH`, y por eso es un parámetro y no una lectura de
/// `current_exe` — las pruebas tienen que aislar el entorno.
///
/// `product_version` es la versión del producto que declara el binario en ejecución:
/// la cabecera `version` del reporte la refleja, mientras `install.version` refleja
/// la del recibo leído en disco.
pub fn report(env: &Env, exe: &Path, product_version: &str) -> Report {
    let roots = &env.roots;
    // El recibo se lee de la instalación registrada, que es donde vive.
    let receipt = receipt::read_from(&roots.program_dir).ok().flatten();
    let channel = channel::detect(exe, receipt.as_ref());
    let pending = pending(env);
    let path = path_state(env, receipt.as_ref());
    let models = models(roots);

    let install = Install {
        // El directorio de programa es el del **entorno**, que ya es el de la
        // instalación registrada cuando hay recibo: `Env::from_receipt` lo toma de ahí.
        // Volver a llamar a `registered_install_dir` aquí volvería a la convención de
        // rutas cuando no hay recibo, que es un sitio que el usuario no está mirando.
        dir: roots.program_dir.clone(),
        data_dir: roots.data_dir.clone(),
        receipt: if receipt.is_some() { "valid" } else { "absent" },
        version: receipt.as_ref().map(|r| r.version.clone()),
    };

    let mut checks: Vec<Check> = Vec::new();
    let mut failed: Vec<String> = Vec::new();
    let mut check = |name: &str, ok: bool, detail: String| {
        checks.push(Check {
            name: name.to_string(),
            ok,
            detail: detail.clone(),
        });
        if !ok {
            failed.push(name.to_string());
        }
    };

    check(
        "install_receipt",
        install.receipt == "valid",
        match &receipt {
            Some(r) => format!("recibo válido en {}", r.install_dir.display()),
            None => "no hay recibo de instalación".to_string(),
        },
    );
    check(
        "path_resolves",
        path.resolves_to_this_install,
        if path.resolves_to_this_install {
            "el comando resuelve a la instalación registrada".to_string()
        } else {
            "el comando no resuelve a la instalación registrada".to_string()
        },
    );
    check(
        "path_duplicates",
        path.duplicate_entries.is_empty(),
        if path.duplicate_entries.is_empty() {
            "sin entradas duplicadas en el PATH".to_string()
        } else {
            format!(
                "{} entradas apuntan a una instalación: {}",
                path.duplicate_entries.len(),
                path.duplicate_entries.join(", ")
            )
        },
    );
    check(
        "pending_artifacts",
        pending.is_clean(),
        if pending.is_clean() {
            "sin residuos de operaciones anteriores".to_string()
        } else {
            format!(
                "{} aparcado(s), {} staging(s) y {} temporal(es) por recoger",
                pending.parked.len(),
                pending.stagings.len(),
                pending.temporaries.len()
            )
        },
    );
    check(
        "models_provisioned",
        models.missing.is_empty(),
        if models.missing.is_empty() {
            "todos los modelos fijados están provisionados".to_string()
        } else {
            format!("faltan: {}", models.missing.join(", "))
        },
    );
    check(
        "models_ct2",
        models.ct2_incomplete.is_empty(),
        if models.ct2_incomplete.is_empty() {
            "los derivados CT2 pasan el gate".to_string()
        } else {
            format!(
                "derivado CT2 incompleto: {}",
                models.ct2_incomplete.join(", ")
            )
        },
    );

    Report {
        version: product_version.to_string(),
        target: crate::target::host_triple().to_string(),
        channel,
        install,
        path,
        pending,
        models,
        checks,
        failed,
    }
}

/// Fila `pending` del reporte, en modo informe: lo que la recuperación **haría**, sin
/// hacerlo.
fn pending(env: &Env) -> Pending {
    let preview = recovery::preview(env.recovery_roots());
    Pending {
        transaction_journal: crate::transaction::journal_path(&env.roots.program_dir).is_file(),
        parked: display_all(&preview.parked),
        stagings: display_all(&preview.stagings),
        temporaries: display_all(&preview.temporaries),
        temporaries_kept: display_all(&preview.temporaries_kept),
    }
}

/// Fila `path` del reporte.
///
/// Un solo recorrido del `PATH` alimenta las tres cosas que la fila declara: qué
/// entradas apuntan a una instalación, cuál es la que resuelve el comando y cuáles son
/// las instalaciones coexistentes. Recorrerlo tres veces era la forma de que una de
/// las tres viera algo que las otras no.
fn path_state(env: &Env, receipt: Option<&InstallReceipt>) -> PathState {
    let integration = receipt.map(|r| &r.path_integration);
    let registered = installs_in_path(env, integration, receipt);

    // Entradas del `PATH` de la sesión que apuntan a una instalación de la aplicación.
    let entries: Vec<String> = env
        .path_env
        .split(path_separator())
        .filter(|e| !e.trim().is_empty())
        .filter(|e| {
            registered
                .iter()
                .any(|(dir, _, _)| crate::canonical_path_entry_matches(Path::new(e), dir))
        })
        .map(|e| e.to_string())
        .collect();
    // El comando resuelve a la instalación registrada si la **primera** coincidencia
    // del `PATH` es la de la instalación registrada, por el orden de precedencia.
    let resolves_to_this_install = entries.first().is_some_and(|first| {
        registered
            .iter()
            .any(|(dir, _, own)| *own && crate::canonical_path_entry_matches(Path::new(first), dir))
    });
    let duplicate_entries = if entries.len() > 1 {
        entries
    } else {
        Vec::new()
    };

    let mut coexisting: Vec<Coexistence> = registered
        .iter()
        .enumerate()
        .filter(|(_, (_, _, own))| !*own)
        .map(|(index, (path, channel, _))| Coexistence {
            path: path.clone(),
            channel: *channel,
            takes_precedence: index == 0,
        })
        .collect();
    // El directorio de programa registrado puede no estar en el `PATH` de la sesión —
    // se invoca por su ruta completa— y aun así ser la instalación a la que se opera.
    // Se informa igualmente, y sin precedencia porque no está en el `PATH`.
    if !registered.iter().any(|(_, _, own)| *own) {
        coexisting.push(Coexistence {
            path: env.roots.program_dir.clone(),
            channel: channel::detect(&env.roots.program_dir, receipt),
            takes_precedence: false,
        });
    }

    PathState {
        resolves_to_this_install,
        duplicate_entries,
        integration: integration_state(env, integration),
        coexisting,
    }
}

/// Separador de entradas del `PATH` de la plataforma.
fn path_separator() -> char {
    if cfg!(windows) {
        ';'
    } else {
        ':'
    }
}

/// Estado del enlace o de la entrada de registro.
///
/// Windows pregunta al **registro**, que es donde vive la integración, y no al `PATH`
/// del proceso: el registro es lo que sobrevive a la sesión. Unix pregunta por el
/// enlace, y `path_unix` no necesita el `PATH` porque la integración es el archivo de
/// arranque.
fn integration_state(env: &Env, integration: Option<&PathIntegration>) -> &'static str {
    if !integration.is_some_and(|i| i.modify_path) {
        return "not_modified";
    }
    #[cfg(windows)]
    {
        if env.registry_subkey.is_empty() {
            return "absent";
        }
        // Se pregunta al registro por la entrada del **directorio de programa**, que es
        // lo que la integración del `PATH` añade: el directorio del enlace es el mismo en
        // Windows.
        let program = env.roots.program_dir.display().to_string();
        match crate::path_windows::read_path(&env.registry_subkey) {
            Ok(Some(raw)) if any_entry_is_from(&raw.value, &[program]) => "present",
            _ => "absent",
        }
    }
    #[cfg(not(windows))]
    {
        let _ = env;
        match integration.and_then(|i| i.symlink.as_ref()) {
            Some(symlink) if symlink.symlink_metadata().is_ok() => "present",
            _ => "absent",
        }
    }
}

#[cfg(windows)]
fn any_entry_is_from(value: &str, candidates: &[String]) -> bool {
    value.split(';').filter(|e| !e.trim().is_empty()).any(|e| {
        candidates
            .iter()
            .any(|c| crate::canonical_path_entry_matches(Path::new(e), Path::new(c)))
    })
}

/// Instalaciones visibles en el `PATH`, en el orden del `PATH`, con la bandera de
/// cuál es la registrada.
///
/// El criterio de "esto es una instalación" es objetivo: el directorio contiene el
/// ejecutable de la aplicación o su recibo. El Cask de Homebrew no deja recibo, así que
/// se reconoce por el ejecutable bajo el prefijo que `channel::is_homebrew_path`
/// define, y el canal sale de [`channel::detect`] con esa misma precedencia.
///
/// El orden es el del `PATH` porque es el que decide la precedencia, que es lo que hay
/// que informar cuando conviven dos instalaciones.
fn installs_in_path(
    env: &Env,
    integration: Option<&PathIntegration>,
    receipt: Option<&InstallReceipt>,
) -> Vec<(PathBuf, Channel, bool)> {
    let exe_name = crate::uninstall::executable_name_default();
    let registered_key = receipt
        .map(|r| crate::canonical_path_key(&r.install_dir))
        .unwrap_or_else(|| crate::canonical_path_key(&env.roots.program_dir));
    // En Unix el bloque delimitado exporta el directorio del enlace, que es la ruta que
    // el usuario ve en el `PATH`; la instalación de ahí también se cuenta.
    let link_directory = integration
        .and_then(|i| i.symlink.clone())
        .and_then(|s| s.parent().map(Path::to_path_buf))
        .unwrap_or_else(crate::bin_dir);

    let mut views: Vec<(PathBuf, Channel, bool)> = Vec::new();
    for entry in env
        .path_env
        .split(path_separator())
        .filter(|e| !e.trim().is_empty())
    {
        let dir = PathBuf::from(entry);
        let looks_like_installation =
            dir.join(&exe_name).is_file() || receipt::receipt_path(&dir).is_file();
        if !looks_like_installation {
            continue;
        }
        let key = crate::canonical_path_key(&dir);
        if views
            .iter()
            .any(|(v, _, _)| crate::canonical_path_key(v) == key)
        {
            continue;
        }
        let channel = channel::detect(&dir.join(&exe_name), None);
        views.push((dir, channel, key == registered_key));
    }

    // El directorio del enlace sin el ejecutable al lado —una instalación enlazada cuyo
    // programa se movió— sigue siendo una instalación visible, y sin ella el
    // diagnóstico no podría decir que el comando resuelve a ella.
    if cfg!(unix) {
        let key = crate::canonical_path_key(&link_directory);
        let already = views
            .iter()
            .any(|(v, _, _)| crate::canonical_path_key(v) == key);
        if !already && link_directory.join(&exe_name).exists() {
            views.insert(
                0,
                (
                    link_directory.clone(),
                    channel::detect(&link_directory.join(&exe_name), None),
                    key == registered_key,
                ),
            );
        }
    }

    views
}

/// Fila `models` del reporte.
fn models(roots: &cleanup::Roots) -> Models {
    // El almacén se ancla en la raíz que describe el reporte, no en la del
    // usuario: la fila `root` y los indicadores de provisión tienen que hablar
    // del mismo árbol. Con la raíz global, un sandbox vacío informaría del
    // estado de la caché real y el reporte mentiría sobre lo que está
    // mirando.
    let store = avi_store::ModelStore::at(roots.models_dir.clone());
    let mut provisioned = Vec::new();
    let mut missing = Vec::new();
    for (name, _, _) in avi_store::MODEL_REVISIONS {
        if store.is_provisioned(name) {
            provisioned.push((*name).to_string());
        } else {
            missing.push((*name).to_string());
        }
    }
    let base_ready = store.is_provisioned(crate::setup::CLONING_MODEL);
    let mut ct2_incomplete = Vec::new();
    for pair in crate::setup::CT2_PAIRS {
        if store.is_provisioned(&format!("marian-{pair}"))
            && !avi_store::is_ct2_provisioned_at(&roots.models_dir, pair)
        {
            ct2_incomplete.push(pair.to_string());
        }
    }
    Models {
        root: roots.models_dir.clone(),
        shared_root: roots.models_shared,
        provisioned,
        missing,
        base: if base_ready {
            "ready"
        } else {
            "missing_opt_in"
        },
        ct2_incomplete,
        size_bytes: cleanup::path_size(&roots.models_dir),
    }
}

fn display_all(paths: &[PathBuf]) -> Vec<String> {
    paths.iter().map(|p| p.display().to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::write_file;

    fn env(tag: &str) -> Env {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or_default();
        let root = std::env::temp_dir().join(format!("doctor-{}-{tag}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let roots = cleanup::Roots {
            program_dir: root.join("opt/ai-voice-interconnector"),
            data_dir: root.join("data"),
            models_dir: root.join("models"),
            temp_root: root.join("tmp"),
            home: root.join("home/ana"),
            models_shared: false,
        };
        for dir in [
            &roots.program_dir,
            &roots.data_dir,
            &roots.models_dir,
            &roots.temp_root,
        ] {
            std::fs::create_dir_all(dir).expect("se crea el sandbox");
        }
        Env {
            roots,
            path_env: String::new(),
            registry_subkey: String::new(),
        }
    }

    fn keys(report: &Report) -> Vec<String> {
        let value = serde_json::to_value(report).expect("el reporte es JSON");
        value
            .as_object()
            .expect("el reporte es un objeto")
            .keys()
            .cloned()
            .collect()
    }

    /// Ejecutable de la plataforma, para plantar una instalación coexistente.
    fn exe_name() -> String {
        crate::uninstall::executable_name_default()
    }

    /// Las nueve claves del sobre —las siete del contrato más `checks` y `failed`— están, y
    /// ninguna de las cuatro que el contrato retira está. La raíz de datos y el estado
    /// del modelo Base sobreviven **dentro** de `install` y de `models`, que es donde el
    /// contrato los coloca.
    #[test]
    fn doctor_reports_every_lifecycle_key() {
        let env = env("keys");
        let exe = env.roots.program_dir.join(exe_name());
        let report = report(&env, &exe, "0.24.0");

        let expected = [
            "version", "target", "channel", "install", "path", "pending", "models", "checks",
            "failed",
        ];
        let keys = keys(&report);
        for expected in expected {
            assert!(
                keys.iter().any(|k| k == expected),
                "falta la clave `{expected}`: {keys:?}"
            );
        }
        assert_eq!(keys.len(), expected.len(), "y no hay más: {keys:?}");

        // La información que las claves retiradas tenían no se pierde: cambia de sitio.
        // La cabecera es la versión del producto que recibe el reporte, no la del
        // crate de librería donde se compila.
        assert_eq!(report.version, "0.24.0");
        assert_eq!(report.target, crate::target::host_triple());
        assert_eq!(report.channel, Channel::Unmanaged);
        assert_eq!(report.install.receipt, "absent");
        assert_eq!(report.install.dir, env.roots.program_dir);
        assert_eq!(report.install.data_dir, env.roots.data_dir);
        assert_eq!(report.path.integration, "not_modified");
        assert!(report.pending.is_clean());
        assert_eq!(report.models.root, env.roots.models_dir);
        assert!(!report.models.shared_root);
        assert_eq!(report.models.base, "missing_opt_in");
        assert_eq!(report.checks.len(), 6, "seis comprobaciones");
        assert!(
            report.is_failure(),
            "sin nada instalado, el veredicto es negativo"
        );

        let _ = std::fs::remove_dir_all(env.roots.program_dir.parent().unwrap().parent().unwrap());
    }

    /// La fila de modelos describe la raíz que recibe el reporte y no la del
    /// usuario. La prueba materializa en el sandbox el snapshot pinneado del
    /// modelo de clonación y espera que el reporte lo declare listo: si la fila
    /// volviera a mirar la caché global, el veredicto dependería de qué tenga
    /// provisionado la máquina que ejecuta la suite.
    #[test]
    fn doctor_reports_models_of_the_root_it_describes() {
        let env = env("raiz-modelos");
        // El layout es el de la caché HF: `models--<org>--<repo>/snapshots/<rev>`.
        // La revisión pinneada se toma de `MODEL_REVISIONS` en vez de fijarla a
        // mano, para que un bump de pin no pueda volver obsoleta la prueba.
        let (repo, revision) = avi_store::MODEL_REVISIONS
            .iter()
            .find(|(name, _, _)| *name == crate::setup::CLONING_MODEL)
            .map(|(_, repo, revision)| (*repo, *revision))
            .expect("el modelo de clonación está pinneado");
        let snapshot = env
            .roots
            .models_dir
            .join(format!("models--{}", repo.replace('/', "--")))
            .join("snapshots")
            .join(revision);
        std::fs::create_dir_all(&snapshot).expect("se crea el snapshot");
        std::fs::write(snapshot.join("model.safetensors"), b"pesos")
            .expect("se escribe un peso no vacío");

        let report = report(&env, &env.roots.program_dir.join(exe_name()), "0.24.0");

        assert_eq!(
            report.models.base, "ready",
            "el snapshot del sandbox hace que el modelo de clonación esté listo"
        );
        assert!(
            report
                .models
                .provisioned
                .contains(&crate::setup::CLONING_MODEL.to_string()),
            "el modelo de clonación figura provisionado en la raíz que describe el reporte"
        );

        let _ = std::fs::remove_dir_all(env.roots.program_dir.parent().unwrap().parent().unwrap());
    }

    /// Sin nada en la raíz que describe el reporte, la fila de modelos dice que
    /// no está. La comprobación contraria no valdría: mirando la caché global,
    /// un sandbox vacío también informaría "listo" en una máquina que tenga el
    /// modelo, que es justo lo que hacía fallar la suite.
    #[test]
    fn doctor_reports_absent_models_for_an_empty_root() {
        let env = env("raiz-vacia");
        let report = report(&env, &env.roots.program_dir.join(exe_name()), "0.24.0");
        assert_eq!(
            report.models.base, "missing_opt_in",
            "una raíz vacía no puede declarar provisionado nada"
        );
        assert_eq!(report.models.provisioned.len(), 0);
        let _ = std::fs::remove_dir_all(env.roots.program_dir.parent().unwrap().parent().unwrap());
    }

    /// Dos instalaciones simultáneas, una de ellas del Cask, y cuál tiene precedencia
    /// en el `PATH`. La registrada no se lista a sí misma.
    #[test]
    fn doctor_reports_coexisting_installations() {
        let mut env = env("coexistence");
        let registered = env.roots.program_dir.clone();
        write_file(&registered.join(exe_name()), "binario");
        let second = env.roots.temp_root.join("segunda-instalacion");
        write_file(&second.join(exe_name()), "binario");
        // La del Cask va la primera, y no deja recibo: se reconoce por el prefijo.
        let cask = env
            .roots
            .temp_root
            .join("Caskroom/ai-voice-interconnector/0.24.0");
        write_file(&cask.join(exe_name()), "binario");
        // El separador es el de la plataforma: `separador_path()` lo decide también en
        // producción, y con el equivocado el `PATH` entero se lee como una sola entrada y
        // la prueba mide otra cosa.
        let sep = if cfg!(windows) { ";" } else { ":" };
        env.path_env = [cask.clone(), registered.clone(), second]
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<String>>()
            .join(sep);

        let report = report(&env, &registered.join(exe_name()), "0.24.0");
        let coexisting = &report.path.coexisting;
        assert_eq!(coexisting.len(), 2, "solo las ajenas: {coexisting:?}");
        assert!(
            coexisting[0].takes_precedence,
            "la primera del PATH tiene precedencia"
        );
        assert!(!coexisting[1].takes_precedence, "la segunda no");
        assert_eq!(
            coexisting[0].channel,
            Channel::Homebrew,
            "el Cask se reconoce por el prefijo, no por el recibo"
        );
        assert_eq!(coexisting[0].path, cask, "y la que precede es la del Cask");
        assert!(report.path.duplicate_entries.len() > 1, "y hay duplicados");
        assert!(report.is_failure());

        let _ = std::fs::remove_dir_all(env.roots.program_dir.parent().unwrap().parent().unwrap());
    }

    /// El informe de pendientes con un `.old-*` y un staging huérfano plantados, y
    /// `doctor` **no toca nada**: es un modo informe.
    #[test]
    fn doctor_reports_pending_artifacts() {
        let env = env("pending");
        let parked = env
            .roots
            .program_dir
            .join(format!("{}1234", crate::PARKED_DIR_PREFIX));
        write_file(&parked.join("anterior"), "v1");
        let staging = env
            .roots
            .program_dir
            .parent()
            .unwrap()
            .join(format!("{}9999", crate::STAGING_DIR_PREFIX));
        write_file(&staging.join("descargado"), "bundle");
        let temp = env.roots.temp_root.join("avi-huerfano.tmp");
        write_file(&temp, "x");

        let findings = report(&env, &env.roots.program_dir.join(exe_name()), "0.24.0");
        let pending = &findings.pending;

        assert!(
            pending
                .parked
                .iter()
                .any(|p| p == &parked.display().to_string()),
            "el aparcado se informa: {:?}",
            pending.parked
        );
        assert!(
            pending
                .stagings
                .iter()
                .any(|p| p == &staging.display().to_string()),
            "el staging huérfano se informa: {:?}",
            pending.stagings
        );
        assert!(
            pending
                .temporaries
                .iter()
                .any(|p| p == &temp.display().to_string()),
            "el temporal huérfano se informa: {:?}",
            pending.temporaries
        );
        assert!(!pending.is_clean());
        assert!(findings.is_failure());

        // Y el diario de transacción se informa cuando existe.
        write_file(
            &crate::transaction::journal_path(&env.roots.program_dir),
            "{}",
        );
        let with_journal = report(&env, &env.roots.program_dir.join(exe_name()), "0.24.0");
        assert!(with_journal.pending.transaction_journal);

        let _ = std::fs::remove_dir_all(env.roots.program_dir.parent().unwrap().parent().unwrap());
    }

    /// La cabecera del reporte es la versión del producto recibida, no la del
    /// crate de librería: con una versión ficticia se distingue el origen.
    #[test]
    fn doctor_reports_product_version_not_crate_version() {
        let env = env("product-version");
        let exe = env.roots.program_dir.join(exe_name());
        let report = report(&env, &exe, "9.9.9");
        assert_eq!(report.version, "9.9.9");
        assert_ne!(report.version, env!("CARGO_PKG_VERSION"));

        let _ = std::fs::remove_dir_all(env.roots.program_dir.parent().unwrap().parent().unwrap());
    }
}
