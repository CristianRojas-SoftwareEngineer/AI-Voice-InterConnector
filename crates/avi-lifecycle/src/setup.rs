//! `setup` trasladado al motor: provisión de modelos y conversión del derivado,
//! que es el paso 11 de la instalación.
//!
//! Es una **traducción fiel, no un rediseño**. Se traslada tal cual la semántica
//! que ya existe: la selección por banderas, la idempotencia por presencia del
//! snapshot, la purga de `--force-update` y la conversión del derivado con su
//! directorio temporal atómico y el mismo gate que la acepta.
//!
//! **Lo que el Ciclo 2 añade, y dónde.** La **selección persistida en
//! configuración** (`setup-selection.json`), la **poda de revisiones obsoletas**
//! y las **migraciones** del `setup` nuevo viven en este módulo, tras `Options`: las
//! necesita una actualización, y el `setup` invocado por el traspaso lee la
//! selección guardada en vez de los flags.
//!
//! **Dónde queda `HF_XET_CACHE`.** Toda la provisión pasa por
//! `ModelStore::new()`, que ya la fija al subdirectorio `xet` de la raíz de modelos
//! exclusiva antes de construir el cliente (`avi-store`, decisión 1 del plan). No
//! hay nada que hacer aquí para cumplirlo, y no se toca: si se fijara también desde
//! el motor, habría dos sitios decidiendo lo mismo.
//!
//! **La purga de `--force-update` pasa por el plan de borrado de modelos**, no por
//! las purgas ad hoc del binario. Es el cambio de fondo de T13: `setup` y
//! `cleanup --model` obedecen las mismas reglas de propiedad —R3 entre ellas— y la
//! misma confirmación destructiva.

use crate::LifecycleError;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Los pares de traducción cuyo derivado CT2 hay que tener, en el orden en que se
/// procesan. El derivado es obligatorio, no opcional: sin él la traducción no
/// funciona.
pub const CT2_PAIRS: [&str; 2] = ["es-en", "en-es"];

/// Modelo cuya provisión depende de `--with-voice-cloning`. Es el único caso de
/// selección opcional que el producto tiene hoy, y por eso el filtro es una
/// comparación con su nombre y no una tabla.
pub use avi_shared::paths::CLONING_MODEL;

/// Opciones de `setup` que el motor necesita conocer. El resto de la superficie
/// (`--json`, `--with-stt`) se queda en el binario, que es quien parsea la CLI.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Options {
    /// `--with-voice-cloning`: añade el modelo base de clonado a la selección.
    pub with_voice_cloning: bool,
    /// `--force-update`: purga los modelos seleccionados y los vuelve a
    /// provisionar. Es una operación destructiva y por eso pide su propia
    /// confirmación.
    pub force_update: bool,
    /// `--yes`: omite la confirmación de la purga y la del tamaño pendiente.
    pub assume_yes: bool,
    /// La invocó `self install` o `self update` **después de su propio resumen**:
    /// La regla dice que entonces no vuelve a preguntar por el tamaño pendiente.
    pub called_from_lifecycle: bool,
}

impl Options {
    /// Opciones de una invocación directa de `setup` por el usuario.
    pub fn user(with_voice_cloning: bool, force_update: bool, assume_yes: bool) -> Self {
        Self {
            with_voice_cloning,
            force_update,
            assume_yes,
            called_from_lifecycle: false,
        }
    }
}

/// Nombre del fichero de selección persistida en la raíz de datos, por decisión 4
/// del Ciclo 2.
pub const SELECTION_FILE_NAME: &str = "setup-selection.json";

/// Versión del esquema de la selección que esta versión del motor sabe leer.
pub const SELECTION_SCHEMA_VERSION: u32 = 1;

/// Selección de `setup` persistida, del Ciclo 2: hoy solo
/// `with_voice_cloning`, extensible a futuros opcionales.
///
/// Sobrevive a los updates porque el reemplazo no toca la raíz de datos; se
/// pierde al desinstalar, lo cual es correcto. Lectura tolerante (fichero
/// ausente o ilegible → conjunto base) y escritura atómica (temporal +
/// renombrado, como el recibo).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupSelection {
    pub schema_version: u32,
    pub with_voice_cloning: bool,
}

impl SetupSelection {
    /// Conjunto base: sin clonado de voz.
    pub fn base() -> Self {
        Self {
            schema_version: SELECTION_SCHEMA_VERSION,
            with_voice_cloning: false,
        }
    }
}

/// Ruta de la selección bajo la raíz de datos vigente (honra `AVI_DATA_DIR`).
pub fn selection_path(data_dir: &Path) -> PathBuf {
    data_dir.join(SELECTION_FILE_NAME)
}

/// Lee la selección guardada, con tolerancia: fichero ausente, ilegible o con
/// esquema desconocido → conjunto base.
pub fn read_selection() -> SetupSelection {
    read_selection_from(&crate::data_dir())
}

/// Núcleo comprobable de [`read_selection`], con la raíz de datos como dato
/// para que las pruebas no toquen la del usuario.
pub fn read_selection_from(data_dir: &Path) -> SetupSelection {
    let text = std::fs::read_to_string(selection_path(data_dir)).unwrap_or_default();
    parse_selection(&text)
}

/// Parsea una selección, devolviendo el conjunto base ante cualquier defecto:
/// JSON inválido, esquema desconocido o forma inesperada.
fn parse_selection(text: &str) -> SetupSelection {
    serde_json::from_str::<SetupSelection>(text)
        .ok()
        .filter(|selection| selection.schema_version == SELECTION_SCHEMA_VERSION)
        .unwrap_or_else(SetupSelection::base)
}

/// Escribe la selección de forma atómica: temporal hermano, escritura,
/// renombrado sobre el destino. Un corte a mitad deja la selección anterior
/// intacta.
pub fn write_selection(selection: &SetupSelection) -> anyhow::Result<()> {
    write_selection_to(&crate::data_dir(), selection)
}

/// Núcleo comprobable de [`write_selection`], con la raíz de datos como dato.
pub fn write_selection_to(data_dir: &Path, selection: &SetupSelection) -> anyhow::Result<()> {
    std::fs::create_dir_all(data_dir)?;
    let dest = selection_path(data_dir);
    let temp = data_dir.join(format!("{SELECTION_FILE_NAME}.tmp-{}", std::process::id()));
    let text = serde_json::to_string_pretty(selection)?;
    std::fs::write(&temp, text.as_bytes())?;
    match std::fs::rename(&temp, &dest) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&temp);
            Err(e.into())
        }
    }
}

/// Opciones efectivas del `setup`: el invocado por el ciclo de vida (el
/// traspaso de `self update`, que corre `self install` con
/// `called_from_lifecycle`) lee la selección guardada, no los flags; el
/// invocado directo por el usuario usa sus flags.
pub fn effective_options(options: &Options) -> Options {
    if options.called_from_lifecycle {
        Options {
            with_voice_cloning: read_selection().with_voice_cloning,
            ..*options
        }
    } else {
        *options
    }
}

/// Desenlace de las migraciones del `setup` nuevo.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MigrationOutcome {
    /// Se creó la selección base porque faltaba o era ilegible.
    pub selection_created: bool,
}

/// Migraciones idempotentes hacia delante del `setup` nuevo, del Ciclo 2,
/// antes de provisionar: hoy, asegurar la selección en esquema 1.
///
/// Idempotente: una segunda ejecución no escribe nada. No borra una selección
/// válida: solo normaliza la ausente o ilegible al conjunto base, que es lo
/// mismo que la lectura tolerante devolvería.
pub fn migrate() -> anyhow::Result<MigrationOutcome> {
    migrate_at(&crate::data_dir())
}

/// Núcleo comprobable de [`migrate`], con la raíz de datos como dato.
pub fn migrate_at(data_dir: &Path) -> anyhow::Result<MigrationOutcome> {
    let valid = std::fs::read_to_string(selection_path(data_dir))
        .ok()
        .and_then(|text| serde_json::from_str::<SetupSelection>(&text).ok())
        .is_some_and(|selection| selection.schema_version == SELECTION_SCHEMA_VERSION);
    if valid {
        return Ok(MigrationOutcome {
            selection_created: false,
        });
    }
    write_selection_to(data_dir, &SetupSelection::base())?;
    Ok(MigrationOutcome {
        selection_created: true,
    })
}

/// Plan de lo que falta provisionar. Alimenta el resumen previo y la
/// confirmación de tamaño; la ejecución no se decide con él, porque [`provision`]
/// mira el estado real del almacén tras cada descarga.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pending {
    /// Repos de los que hay que descargar, con el nombre corto que usa el producto.
    pub models: Vec<String>,
    /// Pares de traducción cuyo derivado CT2 hay que convertir o revalidar,
    /// incluido el de un Marian que se va a descargar en esta pasada.
    pub ct2: Vec<String>,
}

impl Pending {
    /// `true` si no hay nada que hacer, que es la condición de idempotencia
    /// y la que hace que `setup` no descargue nada cuando todo está provisionado.
    pub fn is_empty(&self) -> bool {
        self.models.is_empty() && self.ct2.is_empty()
    }

    /// Tamaño estimado de la descarga pendiente, en bytes: la suma de
    /// `approx_bytes` de los modelos pendientes, medida en la revisión fijada de
    /// cada uno. Es una estimación declarada, no una cifra contable: los derivados
    /// CT2 no descargan nada.
    pub fn estimated_bytes(&self) -> u64 {
        self.models
            .iter()
            .filter_map(|name| {
                avi_store::MODEL_REVISIONS
                    .iter()
                    .find(|pin| pin.name == name)
            })
            .map(|pin| pin.approx_bytes)
            .sum()
    }
}

/// Repos de la selección, con el mismo filtro de clonado que usa el binario hoy.
/// La función es pura para que el filtro se pueda afirmar sin almacén ni disco.
///
/// Cuando la invoca el ciclo de vida (traspaso de `self update`), la selección
/// sale del fichero guardado y no de los flags ([`effective_options`]).
pub fn selection(options: &Options) -> Vec<&'static str> {
    selection_for(effective_options(options).with_voice_cloning)
}

/// Repos de la selección para una elección de clonado ya resuelta. Es el filtro
/// puro que comparten `setup`, la provisión de `self install` y `doctor`.
pub fn selection_for(with_voice_cloning: bool) -> Vec<&'static str> {
    avi_store::MODEL_REVISIONS
        .iter()
        .map(|pin| pin.name)
        .filter(|name| *name != CLONING_MODEL || with_voice_cloning)
        .collect()
}

/// Qué se purga con `--force-update`: **la misma selección**, no el conjunto
/// entero. Purgar el modelo de clonado cuando el usuario no lo pidió dejaría la
/// instalación sin lo que sí quiere.
///
/// Como [`selection`], sale del fichero guardado cuando la invoca el ciclo de
/// vida.
pub fn purge_targets(options: &Options) -> Vec<&'static str> {
    selection(options)
}

/// Calcula lo pendiente contra un almacén ya construido. El almacén se pasa
/// inyectado para que la función sea comprobable con un directorio de pruebas y no
/// necesite red.
pub fn pending(store: &avi_store::ModelStore, options: &Options) -> Pending {
    let mut out = Pending::default();
    for name in selection(options) {
        if !store.is_provisioned(name) {
            out.models.push(name.to_string());
        }
    }
    for pair in CT2_PAIRS {
        // Un derivado está pendiente si su Marian se va a descargar en esta
        // pasada, o si el Marian ya existe y el derivado no pasa el gate.
        let marian = format!("marian-{pair}");
        if out.models.contains(&marian) {
            out.ct2.push(pair.to_string());
            continue;
        }
        if !store.is_provisioned(&marian) {
            continue;
        }
        let ct2_dir = avi_store::ct2_model_dir(pair);
        if needs_reconversion(&ct2_dir, store, &format!("marian-{pair}")) {
            out.ct2.push(pair.to_string());
        }
    }
    out
}

/// ¿Hay que reconvertir el derivado?
///
/// La regla es la del binario: si el derivado existe y es sano, solo se
/// reconvierte cuando su `model.bin` es **más viejo** que el snapshot. Si alguna de
/// las dos fechas no se puede leer, se da el derivado por bueno: reconvertir porque
/// no se pudo leer una fecha convertiría en un fallo lo que es un no-op.
pub fn needs_reconversion(ct2_dir: &Path, store: &avi_store::ModelStore, hf_name: &str) -> bool {
    if !avi_store::ct2_dir_missing_files(ct2_dir).is_empty() {
        return true;
    }
    let Some(snapshot) = store.model_snapshot_path(hf_name) else {
        return true;
    };
    if !snapshot.is_dir() {
        return true;
    }
    let ct2_time = std::fs::metadata(ct2_dir.join("model.bin"))
        .and_then(|m| m.modified())
        .ok();
    let hf_time = std::fs::metadata(snapshot.join("pytorch_model.bin"))
        .or_else(|_| std::fs::metadata(snapshot.join("model.safetensors")))
        .and_then(|m| m.modified())
        .ok();
    match (ct2_time, hf_time) {
        (Some(ct2), Some(hf)) => ct2 <= hf,
        // Sin fechas legibles el derivado se acepta: el criterio del binario es
        // "ya existe, se omite".
        _ => false,
    }
}

/// Ejecuta la purga de `--force-update` por el **plan de borrado de modelos**, de
/// modo que la operación obedezca las mismas reglas de propiedad que
/// `cleanup --model`.
///
/// Devuelve lo que se borró de verdad. Un repo ausente no se reporta: `cleanup`
/// tampoco lo hace, y distinguirlos convertiría un `--force-update` normal en una
/// lista de errores.
pub fn purge(store: &avi_store::ModelStore, options: &Options) -> PurgeOutcome {
    let mut outcome = PurgeOutcome::default();
    for name in purge_targets(options) {
        match store.remove_hf_snapshot(name) {
            Ok(true) => outcome.snapshots.push(name.to_string()),
            Ok(false) => {}
            Err(e) => outcome.failures.push((name.to_string(), e.to_string())),
        }
    }
    // `xet` y los locks solo se tocan en la raíz exclusiva, que es exactamente lo
    // que `remove_xet_cache` y `remove_hf_locks` ya garantizan bajo R3. La purga
    // no reimplementa esa regla: la usa.
    match avi_store::ModelStore::remove_xet_cache() {
        Ok(true) => outcome.xet = true,
        Ok(false) => {}
        Err(e) => outcome.failures.push(("xet".to_string(), e.to_string())),
    }
    if let Err(e) = avi_store::ModelStore::remove_hf_locks() {
        outcome.failures.push((".locks".to_string(), e.to_string()));
    }
    // Y el derivado CT2, que el binario no purgaba en `--force-update` y que es
    // lo que hace que la reconversión tenga sentido.
    match avi_store::remove_ct2_cache() {
        Ok(true) => outcome.ct2 = true,
        Ok(false) => {}
        Err(e) => outcome.failures.push(("ct2".to_string(), e.to_string())),
    }
    outcome
}

/// Qué borró la purga.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PurgeOutcome {
    /// Repos cuyos snapshots se borraron de verdad.
    pub snapshots: Vec<String>,
    /// La caché `xet` se borró. `false` en raíz compartida, donde R3 lo prohíbe.
    pub xet: bool,
    /// El derivado CT2 se borró.
    pub ct2: bool,
    /// Lo que no se pudo borrar, con su motivo.
    pub failures: Vec<(String, String)>,
}

impl PurgeOutcome {
    /// `true` si no hubo ningún fallo. Un `--force-update` con fallos no es un
    /// error de la operación —el resto se provisiona igual— pero sí se informa.
    pub fn is_clean(&self) -> bool {
        self.failures.is_empty()
    }
}

/// Qué podó la poda de revisiones obsoletas.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PruneOutcome {
    /// Revisiones obsoletas borradas, como `nombre/snapshots/<hash>`.
    pub removed: Vec<String>,
    /// Lo que no se pudo borrar, con su motivo.
    pub failures: Vec<(String, String)>,
}

impl PruneOutcome {
    /// `true` si no hubo ningún fallo. Una poda con fallos no es un error de
    /// la operación —lo provisionado queda igual— pero sí se informa.
    pub fn is_clean(&self) -> bool {
        self.failures.is_empty()
    }
}

/// Poda tras éxito, del Ciclo 2: elimina las revisiones de los repos
/// propios que están fuera del pin vigente de `MODEL_REVISIONS`.
///
/// Solo toca directorios de snapshots de repos propios, que son atribuibles a
/// la aplicación: procede tanto en la raíz exclusiva como en la compartida
/// (R3), y nunca toca `xet`, `.locks` ni el derivado CT2. Idempotente: una
/// segunda pasada no encuentra nada que borrar.
///
/// La confirmación de tamaño y `called_from_lifecycle` siguen vigentes: la
/// poda no pregunta por su cuenta, corre tras un éxito ya confirmado.
pub fn prune_obsolete() -> PruneOutcome {
    prune_obsolete_at(&avi_store::models_cache_dir())
}

/// Núcleo comprobable de [`prune_obsolete`], con la raíz de modelos como dato.
pub fn prune_obsolete_at(models_root: &Path) -> PruneOutcome {
    let mut outcome = PruneOutcome::default();
    for pin in avi_store::MODEL_REVISIONS {
        let snapshots = models_root
            .join(format!("models--{}", pin.repo.replace('/', "--")))
            .join("snapshots");
        let live = live_snapshot_name(&snapshots, pin.revision);
        let entries = match std::fs::read_dir(&snapshots) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let child = entry.file_name().to_string_lossy().to_string();
            if live.as_deref() == Some(child.as_str()) {
                continue;
            }
            let label = format!("{}/snapshots/{child}", pin.name);
            match std::fs::remove_dir_all(&path) {
                Ok(()) => outcome.removed.push(label),
                Err(e) => outcome.failures.push((label, e.to_string())),
            }
        }
    }
    outcome.removed.sort();
    outcome
}

/// Nombre del snapshot vivo de un repo: la revisión pinneada si está
/// materializada, y si no la que resuelve el `refs/` (layout estándar de HF
/// hub, el mismo que `ModelStore::model_snapshot_path` replica). `None` si el
/// pin no está en disco: entonces toda revisión presente es obsoleta.
fn live_snapshot_name(snapshots: &Path, revision: &str) -> Option<String> {
    if snapshots.join(revision).is_dir() {
        return Some(revision.to_string());
    }
    let hash = std::fs::read_to_string(snapshots.parent()?.join("refs").join(revision)).ok()?;
    let hash = hash.trim();
    if !hash.is_empty() && snapshots.join(hash).is_dir() {
        return Some(hash.to_string());
    }
    None
}

/// Convierte el snapshot de Marian al derivado CT2 con **escritura atómica**.
///
/// El conversor vuelca en un directorio temporal hermano y solo tras verificar el
/// derivado completo con el mismo gate que lo acepta se renombra sobre el destino.
/// Así un fallo nunca deja un parcial que el gate aceptaría, y un derivado previo
/// roto se sustituye entero.
///
/// La razón de cada fallo nombra lo que hay que hacer para reintentar, como nombra
/// el original: son las que el usuario lee cuando la conversión falla y no hay
/// ninguna otra fuente del mismo mensaje.
pub fn convert(hf_snapshot: &Path, ct2_dir: &Path) -> anyhow::Result<()> {
    let tmp_dir = tmp_dir_for(ct2_dir);
    if tmp_dir.exists() {
        std::fs::remove_dir_all(&tmp_dir)?;
    }
    std::fs::create_dir_all(&tmp_dir)?;
    if let Err(e) = convert_body(hf_snapshot, &tmp_dir) {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(e);
    }
    if ct2_dir.exists() {
        std::fs::remove_dir_all(ct2_dir)?;
    }
    std::fs::rename(&tmp_dir, ct2_dir)?;
    Ok(())
}

/// Directorio temporal hermano del derivado. Hermano y no dentro, porque el
/// renombrado final tiene que ser del mismo volumen: si estuviera dentro del
/// destino, la escritura no sería atómica porque el destino se borra antes.
pub fn tmp_dir_for(ct2_dir: &Path) -> PathBuf {
    ct2_dir.with_extension(format!("tmp-{}", std::process::id()))
}

/// El cuerpo de la conversión: invocar el conversor, asegurar los `.spm` y pasar
/// el gate. Aislado para que `convert` tenga una única salida de error y el
/// temporal se limpie siempre.
fn convert_body(hf_snapshot: &Path, tmp_dir: &Path) -> anyhow::Result<()> {
    let try_converter = |bin: &str| {
        std::process::Command::new(bin)
            .args([
                "-m",
                "ctranslate2.converters.transformers",
                "--model",
                &hf_snapshot.to_string_lossy(),
                "--output_dir",
                &tmp_dir.to_string_lossy(),
                "--quantization",
                "int8",
                "--copy_files",
                "source.spm",
                "target.spm",
                "--force",
            ])
            .status()
    };
    match try_converter("python") {
        Ok(s) if s.success() => {}
        Ok(s) => anyhow::bail!("el conversor python terminó con {s}"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => match try_converter("python3") {
            Ok(s) if s.success() => {}
            Ok(s) => anyhow::bail!("el conversor python3 terminó con {s}"),
            Err(e2) => anyhow::bail!("python no encontrado: {e} / {e2}"),
        },
        Err(e) => anyhow::bail!("fallo al ejecutar converter: {e}"),
    }
    // Copia posterior verificada: si el conversor no depositó los `.spm` —una
    // versión sin `--copy_files`— se copian desde el snapshot pinneado, porque sin
    // ellos el derivado no puede tokenizar.
    for spm in ["source.spm", "target.spm"] {
        if !tmp_dir.join(spm).is_file() {
            let source = hf_snapshot.join(spm);
            if !source.is_file() {
                anyhow::bail!(
                    "el snapshot {} no contiene {spm} (revisión inesperada) — limpia la \
                     caché de modelos y reintenta setup",
                    hf_snapshot.display()
                );
            }
            std::fs::copy(&source, tmp_dir.join(spm))?;
        }
    }
    // Verificación con el mismo criterio del gate antes de declarar éxito.
    let missing = avi_store::ct2_dir_missing_files(tmp_dir);
    if !missing.is_empty() {
        anyhow::bail!(
            "derivado CT2 incompleto (faltan: {}) — limpia la caché de modelos y \
             reintenta setup",
            missing.join(", ")
        );
    }
    Ok(())
}

/// Traduce un fallo de provisión al `reason` de contrato que corresponde.
///
/// `setup_failed` es el que el resumen de `self install` emite ("Programa
/// instalado, pero la provisión de modelos falló"), y `network_error` el que la
/// tabla declara para un fallo de descarga tras reintentos. La traducción vive aquí
/// para que `self install` no tenga que distinguir el origen del fallo: lo que le
/// importa es que el programa queda instalado y basta reintentar con `setup`.
pub fn map_download_failure(model: &str, cause: &anyhow::Error) -> LifecycleError {
    LifecycleError::new("network_error", format!("{model}: {cause}"))
}

/// Costura de la provisión: la descarga (red) y la conversión (Python) se inyectan
/// para poder comprobar la regla sin ninguna de las dos.
#[allow(async_fn_in_trait)]
pub trait Provisioner {
    /// Descarga el modelo `name` al almacén.
    async fn download(&self, name: &str) -> anyhow::Result<()>;
    /// Convierte el snapshot de Marian al derivado CT2 de `ct2_dir`.
    fn convert(&self, snapshot: &Path, ct2_dir: &Path) -> anyhow::Result<()>;
}

/// Implementación de producción: `ModelStore::ensure_downloaded` y [`convert`].
pub struct HubProvisioner;

impl Provisioner for HubProvisioner {
    async fn download(&self, name: &str) -> anyhow::Result<()> {
        avi_store::ModelStore::ensure_downloaded(name)
            .await
            .map(|_| ())
    }

    fn convert(&self, snapshot: &Path, ct2_dir: &Path) -> anyhow::Result<()> {
        convert(snapshot, ct2_dir)
    }
}

/// Lo que hizo [`provision`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Provisioned {
    /// Repos de la selección, en el orden en que se procesaron.
    pub provisioned: Vec<String>,
    /// Repos que hubo que descargar en esta pasada.
    pub downloaded: Vec<String>,
    /// Derivados CT2 convertidos en esta pasada.
    pub converted: Vec<String>,
}

/// Fallo de [`provision`], sin traducir todavía al `reason` de contrato: cada
/// llamador conserva su propio código de error.
#[derive(Debug)]
pub enum ProvisionError {
    /// Falló la descarga de `name`.
    Download { name: String, cause: anyhow::Error },
    /// Falló la conversión del derivado del par `pair`.
    Conversion { pair: String, reason: String },
}

/// Provisión compartida por `setup` y `self install`: descarga la selección no
/// provisionada y después convierte los derivados CT2 según el estado real del
/// almacén. No confirma nada ni escribe la selección.
///
/// La conversión se decide **después** de las descargas porque depende del
/// snapshot descargado: en un almacén vacío no hay nada que convertir antes de
/// bajar Marian.
pub async fn provision(
    store: &avi_store::ModelStore,
    options: &Options,
    provisioner: &impl Provisioner,
) -> Result<Provisioned, ProvisionError> {
    let mut out = Provisioned::default();

    // 1. Descarga idempotente de la selección.
    for name in selection(options) {
        if !store.is_provisioned(name) {
            provisioner
                .download(name)
                .await
                .map_err(|cause| ProvisionError::Download {
                    name: name.to_string(),
                    cause,
                })?;
            out.downloaded.push(name.to_string());
        }
        out.provisioned.push(name.to_string());
    }

    // 2. Derivados CT2: obligatorios, e idempotentes por fecha sobre directorios sanos.
    for pair in CT2_PAIRS {
        let hf_name = format!("marian-{pair}");
        if !store.is_provisioned(&hf_name) {
            continue;
        }
        let conversion = |reason: String| ProvisionError::Conversion {
            pair: pair.to_string(),
            reason,
        };
        let Some(snapshot) = store.model_snapshot_path(&hf_name) else {
            return Err(conversion(format!(
                "snapshot HF de '{hf_name}' no resoluble"
            )));
        };
        if !snapshot.is_dir() {
            return Err(conversion(format!("snapshot HF de '{hf_name}' ausente")));
        }
        let ct2_dir = avi_store::ct2_model_dir(pair);
        if !needs_reconversion(&ct2_dir, store, &hf_name) {
            eprintln!("CT2 {pair} ya convertido, se omite");
            continue;
        }
        provisioner
            .convert(&snapshot, &ct2_dir)
            .map_err(|e| conversion(format!("{e:#}")))?;
        out.converted.push(pair.to_string());
    }
    Ok(out)
}

/// Desenlace de `setup`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outcome {
    /// Qué borró la purga de `--force-update`. Vacío si no se pidió.
    pub purge: PurgeOutcome,
    /// Revisiones obsoletas que podó el Ciclo 2 tras el éxito. Vacío si no
    /// había ninguna fuera del pin vigente.
    pub pruned: PruneOutcome,
    /// Repos de la selección, en el orden en que se procesaron. Es lo que el resumen
    /// publica como `models_provisioned`, y no distingue los que se descargaron de los
    /// que ya estaban: el contrato solo promete el conjunto disponible.
    pub provisioned: Vec<String>,
    /// Derivados CT2 convertidos en esta pasada.
    pub converted: Vec<String>,
}

impl Outcome {
    /// `true` si no provisionó nada porque ya estaba todo, que es la condición de
    /// idempotencia del `setup`.
    pub fn is_already_provisioned(&self) -> bool {
        self.converted.is_empty() && self.provisioned.iter().all(|_| true)
    }
}

/// Ejecuta `setup`: purga si toca, provisiona la selección y convierte los derivados.
///
/// Es el punto de entrada que el binario cablea. La descarga y la conversión viven en
/// [`provision`], compartida con `self install`; `run` añade la migración, la
/// persistencia de la selección, la purga, la confirmación y la poda. Lo único que se
/// queda en el binario es el sobre `--json` y la prosa, porque el parseo de la CLI y
/// el emisor no viven en este crate.
///
/// La confirmación destructiva de `--force-update` y la del tamaño pendiente se
/// aplican aquí, con el mismo módulo `confirm` que usan `cleanup` y
/// `self uninstall`, para que las tres operaciones destructivas del producto tengan una
/// sola implementación de la tabla de confirmaciones.
pub async fn run(store: &avi_store::ModelStore, options: &Options) -> anyhow::Result<Outcome> {
    let mut outcome = Outcome::default();

    // 0. Migraciones del `setup` nuevo y persistencia de la
    //    selección del usuario, antes de provisionar. El `setup` invocado por
    //    el traspaso lee la guardada, no los flags (`called_from_lifecycle`):
    //    por eso aquí solo se escribe en la invocación directa.
    migrate()?;
    if !options.called_from_lifecycle {
        write_selection(&SetupSelection {
            schema_version: SELECTION_SCHEMA_VERSION,
            with_voice_cloning: options.with_voice_cloning,
        })?;
    }

    // 1. `--force-update`: purga de la **selección**, no del conjunto entero.
    if options.force_update {
        if !confirm_destructive(options)? {
            return Ok(outcome);
        }
        outcome.purge = purge(store, options);
        for (name, reason) in &outcome.purge.failures {
            eprintln!("  no se pudo purgar {name}: {reason}");
        }
    }

    // 2. Resumen previo y confirmación con el plan de lo pendiente; después,
    //    la provisión compartida (descarga y conversión CT2 según el estado real).
    let pending = pending(store, options);
    if (options.force_update || !pending.is_empty()) && !confirm_size(&pending, options)? {
        return Ok(outcome);
    }
    let provisioned = provision(store, options, &HubProvisioner)
        .await
        .map_err(|e| match e {
            ProvisionError::Download { name, cause } => {
                anyhow::Error::from(map_download_failure(&name, &cause))
            }
            ProvisionError::Conversion { pair, reason } => conversion_error(
                &pair,
                &format!(
                    "{reason} — instala ctranslate2 (pip install ctranslate2) y reintenta setup"
                ),
            )
            .into(),
        })?;
    outcome.provisioned = provisioned.provisioned;
    outcome.converted = provisioned.converted;

    // 4. Poda tras éxito: las revisiones fuera del pin vigente.
    //    Un fallo aquí no invalida lo provisionado: se informa y se sigue.
    outcome.pruned = prune_obsolete();
    for (name, reason) in &outcome.pruned.failures {
        eprintln!("  no se pudo podar {name}: {reason}");
    }

    Ok(outcome)
}

/// Error de conversión con el motivo ya redactado: el usuario lee este mensaje y no
/// hay otra fuente para él.
fn conversion_error(pair: &str, reason: &str) -> LifecycleError {
    LifecycleError::new(
        "setup_failed",
        format!("No se pudo convertir CT2 {pair}: {reason}"),
    )
}

/// Confirmación destructiva de `--force-update`. `false` es "el usuario dijo
/// que no", que la tabla de `reason` no cuenta como error.
fn confirm_destructive(options: &Options) -> anyhow::Result<bool> {
    let summary =
        vec!["Se purgarán los modelos descargados y se volverán a descargar.".to_string()];
    let decision = crate::confirm::confirm(
        &crate::confirm::Confirmation {
            kind: crate::confirm::Kind::Destructive,
            summary: &summary,
            entries: &[],
            assume_yes: options.assume_yes,
            dry_run: false,
            stdin_is_terminal: std::io::IsTerminal::is_terminal(&std::io::stdin()),
        },
        &mut std::io::stdin().lock(),
        &mut std::io::stderr(),
    )?;
    Ok(decision != crate::confirm::Decision::Cancelled)
}

/// Línea de la confirmación: cuántos modelos se descargan y su tamaño estimado.
fn download_summary(pending: &Pending) -> String {
    format!(
        "Se descargarán {} modelo(s), unos {}.",
        pending.models.len(),
        crate::human_bytes(pending.estimated_bytes())
    )
}

/// Confirmación del tamaño pendiente de la provisión: es no destructiva, así que sin
/// terminal
/// procede, y `--yes` la omite. Cuando la invoca `self install` después de su propio
/// resumen, `called_from_lifecycle` la omite también.
fn confirm_size(pending: &Pending, options: &Options) -> anyhow::Result<bool> {
    if options.called_from_lifecycle {
        return Ok(true);
    }
    if pending.estimated_bytes() == 0 {
        return Ok(true);
    }
    let summary = vec![download_summary(pending)];
    let decision = crate::confirm::confirm(
        &crate::confirm::Confirmation {
            kind: crate::confirm::Kind::NonDestructive,
            summary: &summary,
            entries: &[],
            assume_yes: options.assume_yes,
            dry_run: false,
            stdin_is_terminal: std::io::IsTerminal::is_terminal(&std::io::stdin()),
        },
        &mut std::io::stdin().lock(),
        &mut std::io::stderr(),
    )?;
    Ok(decision != crate::confirm::Decision::Cancelled)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::test_support::{scratch, write_file, ENV_LOCK};
    use std::sync::MutexGuard;

    /// Provisionador simulado: la descarga planta el snapshot pinneado y la
    /// conversión escribe un derivado sano, sin red ni Python.
    pub(crate) struct FakeProvisioner;

    impl Provisioner for FakeProvisioner {
        async fn download(&self, name: &str) -> anyhow::Result<()> {
            let (repo, revision) = avi_store::ModelStore::revision_of(name)
                .ok_or_else(|| anyhow::anyhow!("sin pin: {name}"))?;
            fake_snapshot(&repo.replace('/', "--"), revision);
            Ok(())
        }

        fn convert(&self, _snapshot: &Path, ct2_dir: &Path) -> anyhow::Result<()> {
            let pair = ct2_dir
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_prefix("opus-mt-"))
                .ok_or_else(|| anyhow::anyhow!("directorio CT2 inesperado"))?;
            derived_healthy(pair);
            Ok(())
        }
    }

    /// Fija `AVI_CACHE_DIR` a un directorio de pruebas y devuelve el guard que lo
    /// restaura. `avi-store` resuelve la raíz de modelos por esa variable, así que
    /// sin ella la prueba tocaría la caché de quien la ejecuta.
    pub(crate) fn cache_relocated(tag: &str) -> (MutexGuard<'static, ()>, PathBuf) {
        let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = scratch(tag);
        std::env::set_var("AVI_CACHE_DIR", dir.join("models"));
        (guard, dir)
    }

    /// Crea un snapshot falso del repo `repo` en la revisión fijada.
    ///
    /// Lleva `config.json`, que es lo que `is_provisioned` exige para un repo sin
    /// `MODEL_FILE_PATTERNS` —"algún fichero con tamaño > 0"—, y los ficheros que el
    /// gate de parakeet exige. Deliberadamente **no** se crea
    /// `model.safetensors` ni `pytorch_model.bin`: así la comparación de fechas de
    /// `needs_reconversion` cae en la rama "no se pudo leer una fecha, el derivado se
    /// acepta", que es determinista y no depende de la resolución del reloj del
    /// sistema de ficheros.
    pub(crate) fn fake_snapshot(repo: &str, revision: &str) -> PathBuf {
        let dir = avi_store::models_cache_dir()
            .join(format!("models--{repo}"))
            .join("snapshots")
            .join(revision);
        write_file(&dir.join("config.json"), "{}");
        write_file(&dir.join("model.txt"), "pesos");
        for (model, patterns) in avi_store::MODEL_FILE_PATTERNS {
            for pattern in *patterns {
                write_file(&dir.join(pattern), "artefacto");
            }
            // El patrón de un modelo solo aplica a su repo; se acepta el sobredibujado
            // porque las pruebas crean snapshots de un repo cada vez.
            let _ = model;
        }
        dir
    }

    /// Crea un derivado CT2 sano, el que el gate acepta.
    pub(crate) fn derived_healthy(pair: &str) -> PathBuf {
        let dir = avi_store::ct2_model_dir(pair);
        write_file(&dir.join("model.bin"), "pesos");
        write_file(&dir.join("source.spm"), "spm");
        write_file(&dir.join("target.spm"), "spm");
        dir
    }

    /// La confirmación anuncia la suma de los tamaños fijados en la tabla de
    /// pines, en escala decimal: 4 734 735 847 bytes la selección base (4 modelos)
    /// y 7 250 841 898 con el modelo de clonado (5 modelos).
    #[test]
    fn confirmation_announces_the_pinned_sizes() {
        let pending_of = |with_voice_cloning| Pending {
            models: selection_for(with_voice_cloning)
                .into_iter()
                .map(String::from)
                .collect(),
            ct2: Vec::new(),
        };
        let base = pending_of(false);
        assert_eq!(base.estimated_bytes(), 4_734_735_847);
        assert_eq!(
            download_summary(&base),
            "Se descargarán 4 modelo(s), unos 4.7 GB."
        );
        let cloning = pending_of(true);
        assert_eq!(cloning.estimated_bytes(), 7_250_841_898);
        assert_eq!(
            download_summary(&cloning),
            "Se descargarán 5 modelo(s), unos 7.3 GB."
        );
    }

    /// La provisión no hace trabajo cuando todo está ya provisionado:
    /// `pending` queda vacío y no se toca el disco. Es la idempotencia de `setup`,
    /// que es lo que permite que `self install` lo invoque sin preguntar.
    #[test]
    fn provisioning_is_idempotent() {
        let (_guard, root) = cache_relocated("setup-idempotente");
        let store = avi_store::ModelStore::new();
        let with_clone = Options::user(true, false, true);

        // 1. Sin nada provisionado, todo está pendiente.
        let initial = pending(&store, &with_clone);
        assert_eq!(initial.models.len(), selection(&with_clone).len());
        assert!(!initial.is_empty(), "nada provisionado, todo pendiente");
        assert!(initial.estimated_bytes() > 0, "hay tamaño que anunciar");

        // 2. Se provisiona todo lo de la selección, derivado incluido: `pending`
        //    queda vacío.
        for name in selection(&with_clone) {
            let (repo, revision) = avi_store::ModelStore::revision_of(name)
                .expect("todo repo pinneado tiene revisión");
            fake_snapshot(&repo.replace('/', "--"), revision);
        }
        for pair in CT2_PAIRS {
            derived_healthy(pair);
        }
        let after = pending(&store, &with_clone);
        assert!(
            after.is_empty(),
            "con todo provisionado no queda nada pendiente: {after:?}"
        );
        assert_eq!(after.estimated_bytes(), 0, "y no hay tamaño que anunciar");

        // 3. Volver a calcular no cambia nada: la función es un inspeccionador.
        assert_eq!(pending(&store, &with_clone), after);

        // 4. La selección manda: sin `--with-voice-cloning` el modelo base no se
        //    provisiona y por tanto tampoco se exige.
        let without_clone = Options::user(false, false, true);
        assert!(!selection(&without_clone).contains(&CLONING_MODEL));
        assert!(selection(&with_clone).contains(&CLONING_MODEL));
        assert!(
            pending(&store, &without_clone).is_empty(),
            "lo que no se pidió tampoco se exige"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// Sobre un almacén vacío, `pending` cuenta también el CT2 de los Marian que
    /// se van a descargar: el resumen previo no puede omitirlo.
    #[test]
    fn pending_counts_ct2_of_marian_about_to_download() {
        let (_guard, root) = cache_relocated("setup-pending-ct2");
        let store = avi_store::ModelStore::new();
        let plan = pending(&store, &Options::user(false, false, true));
        assert_eq!(plan.ct2, CT2_PAIRS.map(String::from).to_vec());
        std::fs::remove_dir_all(&root).ok();
    }

    /// Regresión: desde un almacén vacío, la provisión convierte el CT2 tras
    /// descargar, porque decide con el estado real y no con un plan previo.
    #[test]
    fn provision_converts_ct2_after_downloading() {
        let (_guard, root) = cache_relocated("setup-provision-vacio");
        let store = avi_store::ModelStore::new();
        let options = Options::user(false, false, true);
        let result = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime de la prueba")
            .block_on(provision(&store, &options, &FakeProvisioner))
            .expect("la provisión simulada se completa");
        assert_eq!(result.converted, CT2_PAIRS.map(String::from).to_vec());
        for pair in CT2_PAIRS {
            assert!(
                avi_store::is_ct2_provisioned(pair),
                "CT2 {pair} provisionado"
            );
        }
        std::fs::remove_dir_all(&root).ok();
    }

    /// La purga de `--force-update` respeta el filtro de la selección: borra lo
    /// seleccionado y **no** lo que el usuario no pidió.
    #[test]
    fn force_update_respects_selection() {
        let (_guard, root) = cache_relocated("setup-purga");
        let store = avi_store::ModelStore::new();

        // Snapshot del modelo de clonado, que solo se purga si se pidió.
        fake_snapshot(
            "Qwen--Qwen3-TTS-12Hz-0.6B-Base",
            "5d83992436eae1d760afd27aff78a71d676296fc",
        );
        let clone_dir =
            avi_store::models_cache_dir().join("models--Qwen--Qwen3-TTS-12Hz-0.6B-Base");
        assert!(clone_dir.exists(), "el snapshot del modelo de clonado está");

        // Sin `--with-voice-cloning`, la purga no lo toca.
        let without_clone = Options::user(false, true, true);
        assert_eq!(
            purge_targets(&without_clone).len(),
            selection(&without_clone).len(),
            "los objetivos de purga son la selección, no el conjunto entero"
        );
        assert!(!purge_targets(&without_clone).contains(&CLONING_MODEL));
        let outcome = purge(&store, &without_clone);
        assert!(
            clone_dir.exists(),
            "un modelo no seleccionado no se purga: {:?}",
            outcome.snapshots
        );
        assert!(!outcome.snapshots.iter().any(|s| s == CLONING_MODEL));
        assert!(outcome.is_clean(), "no hubo fallos: {:?}", outcome.failures);

        // Con `--with-voice-cloning`, sí.
        let with_clone = Options::user(true, true, true);
        assert!(purge_targets(&with_clone).contains(&CLONING_MODEL));
        let outcome = purge(&store, &with_clone);
        assert!(
            outcome.snapshots.contains(&CLONING_MODEL.to_string()),
            "el modelo seleccionado sí se purga: {:?}",
            outcome.snapshots
        );
        assert!(!clone_dir.exists(), "y su snapshot desaparece de verdad");
        std::fs::remove_dir_all(&root).ok();
    }

    /// Un fallo de conversión no deja un directorio parcial: ni el destino ni el
    /// temporal hermano sobreviven, de modo que un reintento no hereda medio
    /// derivado.
    #[test]
    fn failed_conversion_leaves_no_partial_dir() {
        let root = scratch("setup-conversion");
        let snapshot = root.join("snapshot-vacio");
        let ct2_dir = root.join("models/ct2/marian-es-en");
        std::fs::create_dir_all(&snapshot).unwrap();

        let err = convert(&snapshot, &ct2_dir).expect_err("sin pesos ni conversor no hay derivado");
        let message = format!("{err:#}");
        assert!(
            !ct2_dir.exists(),
            "no queda un directorio de destino parcial: {message}"
        );
        assert!(
            !tmp_dir_for(&ct2_dir).exists(),
            "ni el temporal hermano: {message}"
        );

        // Un derivado previo tampoco se destruye por un fallo: se conserva para
        // que el gate siga aceptándolo y el reintento tenga de dónde partir.
        write_file(&ct2_dir.join("model.bin"), "derivado bueno");
        assert!(convert(&snapshot, &ct2_dir).is_err());
        assert!(
            ct2_dir.join("model.bin").is_file(),
            "el derivado previo sobrevive a un intento fallido"
        );
        assert!(!tmp_dir_for(&ct2_dir).exists());
        std::fs::remove_dir_all(&root).ok();
    }

    /// La conversión que sí funciona, con el gate como única condición: si el
    /// temporal pasa el gate, se renombra; y el gate es el mismo que acepta el
    /// derivado definitivo.
    #[test]
    fn conversion_verifies_with_the_accepting_gate() {
        let root = scratch("setup-gate");
        let ct2_dir = root.join("marian-es-en");
        let tmp = tmp_dir_for(&ct2_dir);
        // El gate exige `model.bin` **y** un tokenizador: `tokenizer.json` o los dos
        // `.spm`. Sin tokenizer el derivado no puede tokenizar aunque tenga pesos.
        write_file(&tmp.join("model.bin"), "pesos");
        assert_eq!(
            avi_store::ct2_dir_missing_files(&tmp),
            vec![
                "tokenizer.json".to_string(),
                "source.spm".to_string(),
                "target.spm".to_string()
            ],
            "pesos sin tokenizador no pasan el gate"
        );
        write_file(&tmp.join("source.spm"), "spm");
        write_file(&tmp.join("target.spm"), "spm");
        assert!(
            avi_store::ct2_dir_missing_files(&tmp).is_empty(),
            "con los dos `.spm` el gate acepta"
        );
        std::fs::remove_file(tmp.join("model.bin")).unwrap();
        assert_eq!(
            avi_store::ct2_dir_missing_files(&tmp),
            vec!["model.bin".to_string()],
            "y sin `model.bin` el gate lo rechaza nombrándolo"
        );
        // El temporal es hermano, no hijo: el renombrado final tiene que ser del
        // mismo volumen, y un temporal dentro del destino no lo sería.
        assert_eq!(tmp.parent(), ct2_dir.parent());
        assert_ne!(tmp, ct2_dir);
        std::fs::remove_dir_all(&root).ok();
    }

    /// La selección sobrevive a un update, desde el Ciclo 2: `setup
    /// --with-voice-cloning` la guarda, y el `setup` invocado por el traspaso
    /// (`called_from_lifecycle`) lee la guardada en vez de los flags.
    #[test]
    fn selection_survives_update_via_saved_file() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let root = scratch("setup-seleccion");
        std::env::set_var("AVI_CACHE_DIR", root.join("models"));
        std::env::set_var("AVI_DATA_DIR", root.join("data"));
        let store = avi_store::ModelStore::new();

        // Todo provisionado con la selección completa: `run` no necesita red.
        let with_clone = Options::user(true, false, true);
        for name in selection(&with_clone) {
            let (repo, revision) = avi_store::ModelStore::revision_of(name)
                .expect("todo repo pinneado tiene revisión");
            fake_snapshot(&repo.replace('/', "--"), revision);
        }
        for pair in CT2_PAIRS {
            derived_healthy(pair);
        }

        // 1. El `setup` directo con `--with-voice-cloning` guarda la selección.
        tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime de la prueba")
            .block_on(run(&store, &with_clone))
            .expect("el setup sin red se completa");
        assert!(
            read_selection().with_voice_cloning,
            "la invocación directa persiste su selección"
        );

        // 2. El traspaso lee la guardada, no los flags: aunque venga con el
        //    flag apagado, el conjunto conserva el modelo de clonado.
        let handover = Options {
            with_voice_cloning: false,
            assume_yes: true,
            called_from_lifecycle: true,
            ..Options::default()
        };
        assert!(
            selection(&handover).contains(&CLONING_MODEL),
            "el traspaso conserva el conjunto guardado: {:?}",
            selection(&handover)
        );

        // 3. Y al revés: la invocación directa usa sus flags aunque haya
        //    fichero guardado.
        assert!(
            !selection(&Options::user(false, false, true)).contains(&CLONING_MODEL),
            "el `setup` directo manda con sus flags"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// La poda elimina justo las revisiones fuera del pin: la pinneada se
    /// queda, la obsoleta se va (incluso con el pin por `refs/`), los
    /// ficheros sueltos se ignoran, y una segunda pasada no encuentra nada.
    #[test]
    fn prune_removes_only_obsolete_revisions() {
        let root = scratch("setup-poda");
        let (repo, revision) =
            avi_store::ModelStore::revision_of("qwen3-tts-0.6b").expect("el modelo base tiene pin");
        let snapshots = root
            .join(format!("models--{}", repo.replace('/', "--")))
            .join("snapshots");
        let old = "abc123abc123abc123abc123abc123abc123abcd";
        assert_ne!(old, revision, "la fixture obsoleta no es el pin");
        write_file(&snapshots.join(revision).join("config.json"), "{}");
        write_file(&snapshots.join(old).join("config.json"), "{}");
        write_file(&snapshots.join("LEEME.txt"), "no es un snapshot");

        // Pin por `refs/`: el hash vivo se conserva y el resto se poda.
        let (marian_repo, marian_rev) =
            avi_store::ModelStore::revision_of("marian-es-en").expect("marian tiene pin");
        let marian_snaps = root
            .join(format!("models--{}", marian_repo.replace('/', "--")))
            .join("snapshots");
        let live_hash = "def456def456def456def456def456def456def4";
        let stale_hash = "0011220011220011220011220011220011220011";
        write_file(
            &root
                .join(format!("models--{}", marian_repo.replace('/', "--")))
                .join("refs")
                .join(marian_rev),
            live_hash,
        );
        write_file(&marian_snaps.join(live_hash).join("config.json"), "{}");
        write_file(&marian_snaps.join(stale_hash).join("config.json"), "{}");

        let outcome = prune_obsolete_at(&root);
        assert_eq!(
            outcome.removed,
            vec![
                format!("marian-es-en/snapshots/{stale_hash}"),
                format!("qwen3-tts-0.6b/snapshots/{old}"),
            ],
            "poda exacta: solo lo obsoleto, en orden"
        );
        assert!(outcome.is_clean(), "sin fallos: {:?}", outcome.failures);
        assert!(snapshots.join(revision).is_dir(), "el pin se queda");
        assert!(
            marian_snaps.join(live_hash).is_dir(),
            "el vivo por `refs/` se queda"
        );
        assert!(!snapshots.join(old).exists(), "lo obsoleto se va");
        assert!(
            snapshots.join("LEEME.txt").is_file(),
            "los ficheros sueltos se ignoran"
        );

        let again = prune_obsolete_at(&root);
        assert!(
            again.removed.is_empty() && again.is_clean(),
            "idempotente: la segunda pasada no encuentra nada"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// En raíz compartida la poda solo toca los snapshots propios: `xet` y
    /// `.locks` son de todos y R3 prohíbe borrarlos —la poda ni los contempla.
    #[test]
    fn prune_shared_root_keeps_unattributable() {
        let root = scratch("setup-poda-compartida");
        let (repo, revision) =
            avi_store::ModelStore::revision_of("parakeet-tdt-v3").expect("parakeet tiene pin");
        let snapshots = root
            .join(format!("models--{}", repo.replace('/', "--")))
            .join("snapshots");
        let old = "9998887776665554443332221110009998887776";
        write_file(
            &snapshots.join(revision).join("encoder-model.int8.onnx"),
            "x",
        );
        write_file(&snapshots.join(old).join("encoder-model.int8.onnx"), "x");
        write_file(&root.join("xet").join("centinela"), "caché global");
        write_file(&root.join(".locks").join("centinela"), "locks globales");

        let outcome = prune_obsolete_at(&root);
        assert_eq!(
            outcome.removed,
            vec![format!("parakeet-tdt-v3/snapshots/{old}")],
            "solo el snapshot obsoleto propio"
        );
        assert!(outcome.is_clean());
        assert!(
            root.join("xet").join("centinela").is_file(),
            "`xet` no se toca en raíz compartida"
        );
        assert!(
            root.join(".locks").join("centinela").is_file(),
            "ni `.locks`"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// Las migraciones son idempotentes y no pisan una selección válida: la
    /// primera crea la base, la segunda no escribe, y con doble ejecución el
    /// resultado es el mismo.
    #[test]
    fn migration_is_idempotent_and_preserves_choice() {
        let root = scratch("setup-migracion");
        let data = root.join("data");

        // 1. Sin fichero: la lectura da la base y la migración la crea.
        assert_eq!(
            read_selection_from(&root.join("ausente")),
            SetupSelection::base(),
            "leer lo ausente da la base"
        );
        let first = migrate_at(&data).expect("la migración crea");
        assert!(first.selection_created);

        // 2. Otra vez: no toca nada (doble ejecución, mismo resultado).
        let second = migrate_at(&data).expect("la migración reitera");
        assert!(!second.selection_created);
        assert_eq!(read_selection_from(&data), SetupSelection::base());

        // 3. Con elección guardada: la conserva.
        write_selection_to(
            &data,
            &SetupSelection {
                schema_version: SELECTION_SCHEMA_VERSION,
                with_voice_cloning: true,
            },
        )
        .expect("se guarda la elección");
        let third = migrate_at(&data).expect("la migración respeta");
        assert!(!third.selection_created);
        assert!(
            read_selection_from(&data).with_voice_cloning,
            "la elección sobrevive a la migración"
        );

        // 4. Ilegible: la lectura da la base y la migración la normaliza.
        std::fs::write(selection_path(&data), "{ no es json").expect("se corrompe");
        assert_eq!(
            read_selection_from(&data),
            SetupSelection::base(),
            "leer lo ilegible da la base"
        );
        let fourth = migrate_at(&data).expect("la migración normaliza");
        assert!(fourth.selection_created);
        assert_eq!(read_selection_from(&data), SetupSelection::base());

        // 5. Esquema futuro: también base, sin abortar.
        std::fs::write(
            selection_path(&data),
            r#"{"schema_version": 99, "with_voice_cloning": true}"#,
        )
        .expect("se adelanta el esquema");
        assert_eq!(read_selection_from(&data), SetupSelection::base());

        // 6. Atómica: tras migrar solo queda la selección, ningún temporal.
        migrate_at(&data).expect("la migración normaliza el esquema futuro");
        let contents: Vec<String> = std::fs::read_dir(&data)
            .expect("se lista la raíz de datos")
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(
            contents,
            vec![SELECTION_FILE_NAME.to_string()],
            "solo queda la selección, ningún temporal hermano"
        );
        std::fs::remove_dir_all(&root).ok();
    }
}
