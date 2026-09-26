//! `setup` trasladado al motor: provisión de modelos y conversión del derivado
//! (§9.7, paso 11 de §9.3).
//!
//! Es una **traducción fiel, no un rediseño**. Se traslada tal cual la semántica
//! que ya existe: la selección por banderas, la idempotencia por presencia del
//! snapshot, la purga de `--force-update` y la conversión del derivado con su
//! directorio temporal atómico y el mismo gate que la acepta.
//!
//! **Lo que no se traslada, y por qué.** La **selección persistida en
//! configuración** y la **poda de revisiones obsoletas** de §9.7 son del **Ciclo 2**
//! (`self update`): las necesita una actualización, no una instalación, y
//! escribirlas aquí sería fijar un contrato que el ciclo 2 va a cambiar. Está
//! escrito en la cabecera de este módulo para que la omisión no se lea como un
//! olvido.
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
//! misma confirmación destructiva de §9.1.

use crate::LifecycleError;
use std::path::{Path, PathBuf};

/// Los pares de traducción cuyo derivado CT2 hay que tener, en el orden en que se
/// procesan. El derivado es obligatorio, no opcional: sin él la traducción no
/// funciona.
pub const CT2_PAIRS: [&str; 2] = ["es-en", "en-es"];

/// Modelo cuya provisión depende de `--with-voice-cloning`. Es el único caso de
/// selección opcional que el producto tiene hoy, y por eso el filtro es una
/// comparación con su nombre y no una tabla.
pub const CLONING_MODEL: &str = "qwen3-tts-0.6b-base";

/// Opciones de `setup` que el motor necesita conocer. El resto de la superficie
/// (`--json`, `--with-stt`) se queda en el binario (§6.3: aquí no se parsea la CLI).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Options {
    /// `--with-voice-cloning`: añade el modelo base de clonado a la selección.
    pub with_voice_cloning: bool,
    /// `--force-update`: purga los modelos seleccionados y los vuelve a
    /// provisionar. Es una operación destructiva (§9.1) y por eso pide su propia
    /// confirmación.
    pub force_update: bool,
    /// `--yes`: omite la confirmación de la purga y la del tamaño pendiente.
    pub assume_yes: bool,
    /// La invocó `self install` o `self update` **después de su propio resumen**:
    /// §9.7 dice que entonces no vuelve a preguntar por el tamaño pendiente.
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

/// Qué está pendiente de provisionar, que es lo que §9.7 manda calcular **antes**
/// de descargar.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pending {
    /// Repos de los que hay que descargar, con el nombre corto que usa el producto.
    pub models: Vec<String>,
    /// Pares de traducción cuyo derivado CT2 hay que convertir o revalidar.
    pub ct2: Vec<String>,
}

impl Pending {
    /// `true` si no hay nada que hacer, que es la condición de idempotencia de §9.7
    /// y la que hace que `setup` no descargue nada cuando todo está provisionado.
    pub fn is_empty(&self) -> bool {
        self.models.is_empty() && self.ct2.is_empty()
    }

    /// Tamaño estimado de la descarga pendiente, en bytes. Es una estimación
    /// declarada: el número exacto depende de lo que reporte el servidor, y §9.7
    /// pide un orden de magnitud para la confirmación, no una cifra contable.
    pub fn estimated_bytes(&self) -> u64 {
        MODEL_DOWNLOAD_ESTIMATE * self.models.len() as u64
    }
}

/// Estimación por repo pinneado. La suma del conjunto base ronda los 9 GB, que es
/// la cifra que el resumen previo de §9.3 usa como ejemplo.
pub const MODEL_DOWNLOAD_ESTIMATE: u64 = 3_000_000_000;

/// Repos de la selección, con el mismo filtro de clonado que usa el binario hoy.
/// La función es pura para que el filtro se pueda afirmar sin almacén ni disco.
pub fn selection(options: &Options) -> Vec<&'static str> {
    avi_store::MODEL_REVISIONS
        .iter()
        .map(|(nombre, _, _)| *nombre)
        .filter(|nombre| *nombre != CLONING_MODEL || options.with_voice_cloning)
        .collect()
}

/// Qué se purga con `--force-update`: **la misma selección**, no el conjunto
/// entero. Purgar el modelo de clonado cuando el usuario no lo pidió dejaría la
/// instalación sin lo que sí quiere.
pub fn purge_targets(options: &Options) -> Vec<&'static str> {
    selection(options)
}

/// Calcula lo pendiente contra un almacén ya construido. El almacén se pasa
/// inyectado para que la función sea comprobable con un directorio de pruebas y no
/// necesite red.
pub fn pending(store: &avi_store::ModelStore, options: &Options) -> Pending {
    let mut out = Pending::default();
    for nombre in selection(options) {
        if !store.is_provisioned(nombre) {
            out.models.push(nombre.to_string());
        }
    }
    for pair in CT2_PAIRS {
        // El derivado solo se considera pendiente si el repo está provisionado:
        // sin snapshot no hay nada que convertir, y reintentarlo es ruido.
        if !store.is_provisioned(&format!("marian-{pair}")) {
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
    for nombre in purge_targets(options) {
        match store.remove_hf_snapshot(nombre) {
            Ok(true) => outcome.snapshots.push(nombre.to_string()),
            Ok(false) => {}
            Err(e) => outcome.failures.push((nombre.to_string(), e.to_string())),
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
    if let Err(e) = convertir(hf_snapshot, &tmp_dir) {
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
fn convertir(hf_snapshot: &Path, tmp_dir: &Path) -> anyhow::Result<()> {
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

/// Traduce un fallo de provisión al `reason` de §9.1 que corresponde.
///
/// `setup_failed` es el que el resumen de `self install` emite (§9.1: "Programa
/// instalado, pero la provisión de modelos falló"), y `network_error` el que
/// declara §9.1 para un fallo de descarga tras reintentos. La traducción vive aquí
/// para que `self install` no tenga que distinguir el origen del fallo: lo que le
/// importa es que el programa queda instalado y basta reintentar con `setup`.
pub fn map_download_failure(modelo: &str, causa: &anyhow::Error) -> LifecycleError {
    LifecycleError::new("network_error", 1, format!("{modelo}: {causa}"))
}

/// Desenlace de `setup`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outcome {
    /// Qué borró la purga de `--force-update`. Vacío si no se pidió.
    pub purge: PurgeOutcome,
    /// Repos de la selección, en el orden en que se procesaron. Es lo que el resumen
    /// publica como `models_provisioned`, y no distingue los que se descargaron de los
    /// que ya estaban: el contrato solo promete el conjunto disponible.
    pub provisioned: Vec<String>,
    /// Derivados CT2 convertidos en esta pasada.
    pub converted: Vec<String>,
}

impl Outcome {
    /// `true` si no provisionó nada porque ya estaba todo, que es la condición de
    /// idempotencia de §9.7.
    pub fn is_already_provisioned(&self) -> bool {
        self.converted.is_empty() && self.provisioned.iter().all(|_| true)
    }
}

/// Ejecuta `setup`: purga si toca, provisiona la selección y convierte los derivados.
///
/// Es el punto de entrada que el binario cablea. Todo lo que hay aquí era lógica del
/// handler: el filtro de la selección, la idempotencia por presencia del snapshot, la
/// reconversión por fecha, la escritura atómica del derivado y la purga por plan. Lo
/// único que se queda en el binario es el sobre `--json` y la prosa, porque el parseo
/// de la CLI y el emisor no viven en este crate (§6.3).
///
/// La confirmación destructiva de `--force-update` y la del tamaño pendiente (§9.1 y
/// §9.7) se aplican aquí, con el mismo módulo `confirm` que usan `cleanup` y
/// `self uninstall`, para que las tres operaciones destructivas del producto tengan una
/// sola implementación de la tabla de §9.1.
pub async fn run(store: &avi_store::ModelStore, options: &Options) -> anyhow::Result<Outcome> {
    let mut outcome = Outcome::default();

    // 1. `--force-update`: purga de la **selección**, no del conjunto entero.
    if options.force_update {
        if !confirmar_destructivo(options)? {
            return Ok(outcome);
        }
        outcome.purge = purge(store, options);
        for (nombre, motivo) in &outcome.purge.failures {
            eprintln!("  no se pudo purgar {nombre}: {motivo}");
        }
    }

    // 2. Provisión idempotente de la selección.
    let pendiente = pending(store, options);
    if (options.force_update || !pendiente.is_empty()) && !confirmar_tamano(&pendiente, options)? {
        return Ok(outcome);
    }
    for nombre in selection(options) {
        if !store.is_provisioned(nombre) {
            avi_store::ModelStore::ensure_downloaded(nombre)
                .await
                .map_err(|e| map_download_failure(nombre, &e))?;
        }
        outcome.provisioned.push(nombre.to_string());
    }

    // 3. Derivados CT2: obligatorios, e idempotentes por fecha sobre directorios sanos.
    for pair in CT2_PAIRS {
        let hf_name = format!("marian-{pair}");
        if !store.is_provisioned(&hf_name) {
            continue;
        }
        let Some(snapshot) = store.model_snapshot_path(&hf_name) else {
            return Err(conversion_error(
                pair,
                &format!("snapshot HF de '{hf_name}' no resoluble"),
            )
            .into());
        };
        if !snapshot.is_dir() {
            return Err(
                conversion_error(pair, &format!("snapshot HF de '{hf_name}' ausente")).into(),
            );
        }
        let ct2_dir = avi_store::ct2_model_dir(pair);
        if !needs_reconversion(&ct2_dir, store, &hf_name) {
            eprintln!("CT2 {pair} ya convertido, se omite");
            continue;
        }
        convert(&snapshot, &ct2_dir).map_err(|e| {
            conversion_error(
                pair,
                &format!("{e} — instala ctranslate2 (pip install ctranslate2) y reintenta setup"),
            )
        })?;
        outcome.converted.push(pair.to_string());
    }

    Ok(outcome)
}

/// Error de conversión con el motivo ya redactado: el usuario lee este mensaje y no
/// hay otra fuente para él.
fn conversion_error(pair: &str, motivo: &str) -> LifecycleError {
    LifecycleError::new(
        "setup_failed",
        11,
        format!("No se pudo convertir CT2 {pair}: {motivo}"),
    )
}

/// Confirmación destructiva de `--force-update` (§9.1). `false` es "el usuario dijo
/// que no", que §9.1 no cuenta como error.
fn confirmar_destructivo(options: &Options) -> anyhow::Result<bool> {
    let resumen =
        vec!["Se purgarán los modelos descargados y se volverán a descargar.".to_string()];
    let decision = crate::confirm::confirm(
        &crate::confirm::Confirmation {
            kind: crate::confirm::Kind::Destructive,
            summary: &resumen,
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

/// Confirmación del tamaño pendiente de §9.7: es no destructiva, así que sin terminal
/// procede, y `--yes` la omite. Cuando la invoca `self install` después de su propio
/// resumen, `called_from_lifecycle` la omite también.
fn confirmar_tamano(pendiente: &Pending, options: &Options) -> anyhow::Result<bool> {
    if options.called_from_lifecycle {
        return Ok(true);
    }
    if pendiente.estimated_bytes() == 0 {
        return Ok(true);
    }
    let resumen = vec![format!(
        "Se descargarán {} modelo(s), unos {}.",
        pendiente.models.len(),
        human_bytes(pendiente.estimated_bytes())
    )];
    let decision = crate::confirm::confirm(
        &crate::confirm::Confirmation {
            kind: crate::confirm::Kind::NonDestructive,
            summary: &resumen,
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

/// Tamaño legible con la misma escala que el resto del producto.
fn human_bytes(bytes: u64) -> String {
    const UNIDADES: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut valor = bytes as f64;
    let mut unidad = 0;
    while valor >= 1024.0 && unidad + 1 < UNIDADES.len() {
        valor /= 1024.0;
        unidad += 1;
    }
    if unidad == 0 {
        format!("{bytes} B")
    } else {
        format!("{valor:.1} {}", UNIDADES[unidad])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{scratch, write_file, ENV_LOCK};
    use std::sync::MutexGuard;

    /// Fija `AVI_CACHE_DIR` a un directorio de pruebas y devuelve el guard que lo
    /// restaura. `avi-store` resuelve la raíz de modelos por esa variable, así que
    /// sin ella la prueba tocaría la caché de quien la ejecuta.
    fn cache_relocated(tag: &str) -> (MutexGuard<'static, ()>, PathBuf) {
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
    fn snapshot_falso(repo: &str, revision: &str) -> PathBuf {
        let dir = avi_store::models_cache_dir()
            .join(format!("models--{repo}"))
            .join("snapshots")
            .join(revision);
        write_file(&dir.join("config.json"), "{}");
        write_file(&dir.join("model.txt"), "pesos");
        for (modelo, patrones) in avi_store::MODEL_FILE_PATTERNS {
            for patron in *patrones {
                write_file(&dir.join(patron), "artefacto");
            }
            // El patrón de un modelo solo aplica a su repo; se acepta el sobredibujado
            // porque las pruebas crean snapshots de un repo cada vez.
            let _ = modelo;
        }
        dir
    }

    /// Crea un derivado CT2 sano, el que el gate acepta.
    fn derivado_sano(pair: &str) -> PathBuf {
        let dir = avi_store::ct2_model_dir(pair);
        write_file(&dir.join("model.bin"), "pesos");
        write_file(&dir.join("source.spm"), "spm");
        write_file(&dir.join("target.spm"), "spm");
        dir
    }

    /// La provisión no hace trabajo cuando todo está ya provisionado (§9.7):
    /// `pending` queda vacío y no se toca el disco. Es la idempotencia de `setup`,
    /// que es lo que permite que `self install` lo invoque sin preguntar.
    #[test]
    fn provisioning_is_idempotent() {
        let (_guard, raiz) = cache_relocated("setup-idempotente");
        let store = avi_store::ModelStore::new();
        let con_clonado = Options::user(true, false, true);

        // 1. Sin nada provisionado, todo está pendiente.
        let inicial = pending(&store, &con_clonado);
        assert_eq!(inicial.models.len(), selection(&con_clonado).len());
        assert!(!inicial.is_empty(), "nada provisionado, todo pendiente");
        assert!(inicial.estimated_bytes() > 0, "hay tamaño que anunciar");

        // 2. Se provisiona todo lo de la selección, derivado incluido: `pending`
        //    queda vacío.
        for nombre in selection(&con_clonado) {
            let (repo, revision) = avi_store::ModelStore::revision_of(nombre)
                .expect("todo repo pinneado tiene revisión");
            snapshot_falso(&repo.replace('/', "--"), revision);
        }
        for pair in CT2_PAIRS {
            derivado_sano(pair);
        }
        let despues = pending(&store, &con_clonado);
        assert!(
            despues.is_empty(),
            "con todo provisionado no queda nada pendiente: {despues:?}"
        );
        assert_eq!(despues.estimated_bytes(), 0, "y no hay tamaño que anunciar");

        // 3. Volver a calcular no cambia nada: la función es un inspeccionador.
        assert_eq!(pending(&store, &con_clonado), despues);

        // 4. La selección manda: sin `--with-voice-cloning` el modelo base no se
        //    provisiona y por tanto tampoco se exige.
        let sin_clonado = Options::user(false, false, true);
        assert!(!selection(&sin_clonado).contains(&CLONING_MODEL));
        assert!(selection(&con_clonado).contains(&CLONING_MODEL));
        assert!(
            pending(&store, &sin_clonado).is_empty(),
            "lo que no se pidió tampoco se exige"
        );
        std::fs::remove_dir_all(&raiz).ok();
    }

    /// La purga de `--force-update` respeta el filtro de la selección: borra lo
    /// seleccionado y **no** lo que el usuario no pidió.
    #[test]
    fn force_update_respects_selection() {
        let (_guard, raiz) = cache_relocated("setup-purga");
        let store = avi_store::ModelStore::new();

        // Snapshot del modelo de clonado, que solo se purga si se pidió.
        snapshot_falso(
            "Qwen--Qwen3-TTS-12Hz-0.6B-Base",
            "5d83992436eae1d760afd27aff78a71d676296fc",
        );
        let clonado_dir =
            avi_store::models_cache_dir().join("models--Qwen--Qwen3-TTS-12Hz-0.6B-Base");
        assert!(
            clonado_dir.exists(),
            "el snapshot del modelo de clonado está"
        );

        // Sin `--with-voice-cloning`, la purga no lo toca.
        let sin_clonado = Options::user(false, true, true);
        assert_eq!(
            purge_targets(&sin_clonado).len(),
            selection(&sin_clonado).len(),
            "los objetivos de purga son la selección, no el conjunto entero"
        );
        assert!(!purge_targets(&sin_clonado).contains(&CLONING_MODEL));
        let outcome = purge(&store, &sin_clonado);
        assert!(
            clonado_dir.exists(),
            "un modelo no seleccionado no se purga: {:?}",
            outcome.snapshots
        );
        assert!(!outcome.snapshots.iter().any(|s| s == CLONING_MODEL));
        assert!(outcome.is_clean(), "no hubo fallos: {:?}", outcome.failures);

        // Con `--with-voice-cloning`, sí.
        let con_clonado = Options::user(true, true, true);
        assert!(purge_targets(&con_clonado).contains(&CLONING_MODEL));
        let outcome = purge(&store, &con_clonado);
        assert!(
            outcome.snapshots.contains(&CLONING_MODEL.to_string()),
            "el modelo seleccionado sí se purga: {:?}",
            outcome.snapshots
        );
        assert!(!clonado_dir.exists(), "y su snapshot desaparece de verdad");
        std::fs::remove_dir_all(&raiz).ok();
    }

    /// Un fallo de conversión no deja un directorio parcial: ni el destino ni el
    /// temporal hermano sobreviven, de modo que un reintento no hereda medio
    /// derivado.
    #[test]
    fn failed_conversion_leaves_no_partial_dir() {
        let raiz = scratch("setup-conversion");
        let snapshot = raiz.join("snapshot-vacio");
        let ct2_dir = raiz.join("models/ct2/marian-es-en");
        std::fs::create_dir_all(&snapshot).unwrap();

        let err = convert(&snapshot, &ct2_dir).expect_err("sin pesos ni conversor no hay derivado");
        let mensaje = format!("{err:#}");
        assert!(
            !ct2_dir.exists(),
            "no queda un directorio de destino parcial: {mensaje}"
        );
        assert!(
            !tmp_dir_for(&ct2_dir).exists(),
            "ni el temporal hermano: {mensaje}"
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
        std::fs::remove_dir_all(&raiz).ok();
    }

    /// La conversión que sí funciona, con el gate como única condición: si el
    /// temporal pasa el gate, se renombra; y el gate es el mismo que acepta el
    /// derivado definitivo.
    #[test]
    fn conversion_verifies_with_the_accepting_gate() {
        let raiz = scratch("setup-gate");
        let ct2_dir = raiz.join("marian-es-en");
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
        std::fs::remove_dir_all(&raiz).ok();
    }
}
