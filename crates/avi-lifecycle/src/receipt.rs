//! Recibo de instalación: qué se instaló, dónde, por qué canal y qué
//! integración de `PATH` se aplicó.
//!
//! El recibo es lo que permite reparar una instalación, revertir exactamente
//! lo que se hizo en `PATH` al desinstalar y operar sobre la instalación
//! registrada aunque el comando lo ejecute otra copia del binario. Vive
//! dentro del directorio de programa y se escribe de forma atómica, con fichero
//! temporal hermano y renombrado, de modo que un corte a mitad no deje un
//! recibo corrupto que la siguiente ejecución no pueda leer.
//!
//! **Lectura estricta**: el recibo debe tener exactamente el esquema vigente.
//! Un campo desconocido, un campo ausente o un `schema_version` distinto del
//! soportado es un error, y la operación que lo lee falla nombrando el fichero.
//! `Ok(None)` significa únicamente que el fichero no existe: un recibo ilegible
//! nunca se trata como ausente.
//!
//! **Los valores efectivos de las raíces se registran** (`roots`): aunque la
//! variable de reubicación deje de estar definida, la actualización y la
//! desinstalación operan sobre las mismas ubicaciones.

use crate::channel::Channel;
use avi_process::registry::now_rfc3339;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Nombre del recibo dentro del directorio de programa.
pub const RECEIPT_NAME: &str = "install-receipt.json";

/// Versión del esquema que esta versión del motor sabe leer.
pub const RECEIPT_SCHEMA_VERSION: u32 = 1;

/// Integración de `PATH` efectivamente aplicada. Los tres
/// campos son mutuamente excluyentes según la plataforma: en Unix el enlace
/// simbólico y los bloques de perfil, en Windows la entrada de registro; lo que
/// no aplique es `null` y no una cadena vacía, para que se distinga "no se
/// integró" de "se integró a vacío".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PathIntegration {
    /// `false` si se pasó `--no-modify-path`: el comando se invoca por su ruta
    /// completa y no se toca ningún perfil ni el registro.
    pub modify_path: bool,
    /// Enlace simbólico del directorio del enlace, en Unix.
    #[serde(deserialize_with = "required")]
    pub symlink: Option<PathBuf>,
    /// Archivos de arranque de shell con el bloque delimitado, en Unix.
    #[serde(deserialize_with = "required")]
    pub profile_blocks: Option<Vec<PathBuf>>,
    /// Entrada añadida al valor `Path` de `HKCU\Environment`, en Windows.
    #[serde(deserialize_with = "required")]
    pub registry_entry: Option<PathBuf>,
}

/// Deserializa un campo opcional exigiendo que la clave exista: el recibo escribe
/// `null` para lo que no aplica, así que una clave ausente es un recibo mal formado.
pub(crate) fn required<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

impl PathIntegration {
    /// Integración de Unix: el enlace del directorio del enlace y los perfiles
    /// tocados.
    pub fn unix(symlink: PathBuf, profile_blocks: Vec<PathBuf>) -> Self {
        Self {
            modify_path: true,
            symlink: Some(symlink),
            profile_blocks: Some(profile_blocks),
            registry_entry: None,
        }
    }

    /// Integración de Windows: la entrada del valor `Path` del usuario.
    pub fn windows(registry_entry: PathBuf) -> Self {
        Self {
            modify_path: true,
            symlink: None,
            profile_blocks: None,
            registry_entry: Some(registry_entry),
        }
    }

    /// Integración explícitamente no deseada (`--no-modify-path`).
    pub fn none() -> Self {
        Self::default()
    }
}

/// Raíces de datos y de modelos tal como quedaron resueltas en el momento de la
/// instalación. Se guardan como valores efectivos, no como
/// nombres de variable, para que una desinstalación posterior no dependa de que
/// la variable siga definida.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Roots {
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
}

/// Recibo de instalación completo, con la estructura que el instalador escribe y
/// sin campos inventados.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallReceipt {
    pub schema_version: u32,
    pub app: String,
    pub version: String,
    pub target: String,
    pub channel: Channel,
    pub install_dir: PathBuf,
    /// Rutas relativas de los archivos colocados, en el orden en que se
    /// colocaron.
    pub files: Vec<String>,
    pub path_integration: PathIntegration,
    pub roots: Roots,
    /// Instante de la instalación en RFC 3339 UTC.
    pub installed_at: String,
    /// Origen del bundle, si la instalación vino de un archivo publicado; `null`
    /// para una reparación.
    #[serde(deserialize_with = "required")]
    pub source: Option<String>,
}

impl InstallReceipt {
    /// Recibo de una instalación nueva. `roots` y `install_dir` se pasan ya
    /// resueltos: quien escribe es quien sabe qué valor efectivo se aplicó.
    // Los ocho campos son los que el instalador conoce uno a uno; un
    // agrupador aquí solo movería la misma lista un sitio más arriba.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        version: &str,
        target: &str,
        channel: Channel,
        install_dir: &Path,
        files: Vec<String>,
        path_integration: PathIntegration,
        roots: Roots,
        source: Option<String>,
    ) -> Self {
        Self {
            schema_version: RECEIPT_SCHEMA_VERSION,
            app: crate::APP_NAME.to_string(),
            version: version.to_string(),
            target: target.to_string(),
            channel,
            install_dir: install_dir.to_path_buf(),
            files,
            path_integration,
            roots,
            installed_at: now_rfc3339(),
            source,
        }
    }
}

/// Ruta del recibo dentro del directorio de programa.
pub fn receipt_path(program_dir: &Path) -> PathBuf {
    program_dir.join(RECEIPT_NAME)
}

/// Lee el recibo del directorio de programa. `Ok(None)` solo si el fichero no
/// existe: una instalación sin recibo **no se adopta**, se trata como no instalada.
/// Un fichero que no se puede leer o que no cumple el esquema es un error cuyo
/// mensaje nombra la ruta del recibo.
pub fn read_from(program_dir: &Path) -> anyhow::Result<Option<InstallReceipt>> {
    let path = receipt_path(program_dir);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(anyhow::anyhow!(
                "no se pudo leer el recibo de instalación {}: {e}",
                path.display()
            ))
        }
    };
    parse(&text).map(Some).map_err(|e| {
        anyhow::anyhow!(
            "el recibo de instalación {} es ilegible: {e}",
            path.display()
        )
    })
}

/// Parsea un recibo, exigiendo que su `schema_version` sea exactamente el vigente.
pub fn parse(text: &str) -> anyhow::Result<InstallReceipt> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| anyhow::anyhow!("no es JSON válido: {e}"))?;
    let declared = value
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| anyhow::anyhow!("no declara `schema_version`"))?;
    if declared != u64::from(RECEIPT_SCHEMA_VERSION) {
        return Err(anyhow::anyhow!(
            "declara schema_version {declared} y solo se admite {RECEIPT_SCHEMA_VERSION}"
        ));
    }
    serde_json::from_value(value).map_err(|e| anyhow::anyhow!("no cumple el esquema: {e}"))
}

/// Escribe el recibo de forma atómica: temporal hermano, escritura, renombrado
/// sobre el destino. Un corte a mitad deja el recibo anterior intacto.
pub fn write_to(receipt: &InstallReceipt, program_dir: &Path) -> anyhow::Result<()> {
    let dest = receipt_path(program_dir);
    let temp = program_dir.join(format!("{RECEIPT_NAME}.tmp-{}", std::process::id()));
    let text = serde_json::to_string_pretty(receipt)?;
    std::fs::write(&temp, text.as_bytes())?;
    match std::fs::rename(&temp, &dest) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&temp);
            Err(e.into())
        }
    }
}

/// Inversa de la conversión de instante Unix a fecha civil de
/// `avi_process::registry::now_rfc3339`, para las pruebas del formato: no hace
/// falta ningún crate de fechas para afirmar que lo escrito se puede volver a
/// leer.
#[cfg(test)]
fn unix_from_rfc3339(text: &str) -> Option<i64> {
    let (date, time) = text.split_once('T')?;
    let time = time.strip_suffix('Z')?;
    let mut date = date.split('-');
    let year: i64 = date.next()?.parse().ok()?;
    let month: u32 = date.next()?.parse().ok()?;
    let day: u32 = date.next()?.parse().ok()?;
    let mut time = time.split(':');
    let hour: i64 = time.next()?.parse().ok()?;
    let minute: i64 = time.next()?.parse().ok()?;
    let second: i64 = time.next()?.parse().ok()?;

    // Días desde 1970-01-01 hasta la fecha, por la vía inversa del algoritmo.
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp: i64 = if month > 2 {
        (month - 3) as i64
    } else {
        (month + 9) as i64
    };
    let day_of_year = (153 * mp + 2) / 5 + day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + day_of_year;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hour * 3600 + minute * 60 + second)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{scratch, ENV_LOCK};

    fn sample_receipt() -> InstallReceipt {
        InstallReceipt::new(
            "0.24.0",
            "x86_64-unknown-linux-gnu",
            Channel::Script,
            Path::new("/home/ana/.local/opt/ai-voice-interconnector"),
            vec![
                "ai-voice-interconnector".to_string(),
                "vendor/qwen3-tts/qwen_tts".to_string(),
            ],
            PathIntegration::unix(
                PathBuf::from("/home/ana/.local/bin/ai-voice-interconnector"),
                vec![PathBuf::from("/home/ana/.bashrc")],
            ),
            Roots {
                data_dir: PathBuf::from("/home/ana/.local/share/ai-voice-interconnector"),
                cache_dir: PathBuf::from("/home/ana/.cache/ai-voice-interconnector"),
            },
            Some("https://example.invalid/bundle.tar.gz".to_string()),
        )
    }

    /// Escribir y volver a leer devuelve el mismo recibo, y no queda el temporal
    /// hermano en el disco: eso es lo que hace atómica la escritura.
    #[test]
    fn receipt_roundtrip_is_atomic() {
        let dir = scratch("receipt-roundtrip");
        let receipt = sample_receipt();
        write_to(&receipt, &dir).unwrap();

        let loaded = read_from(&dir)
            .unwrap()
            .expect("el recibo está donde se escribió");
        assert_eq!(
            loaded, receipt,
            "el recibo sobrevive al viaje de ida y vuelta"
        );
        assert_eq!(loaded.schema_version, RECEIPT_SCHEMA_VERSION);
        assert_eq!(loaded.app, crate::APP_NAME);
        assert_eq!(loaded.channel, Channel::Script);
        assert_eq!(loaded.channel.as_str(), "script");

        // El temporal no sobrevive al renombrado.
        let contents: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(
            contents,
            vec![RECEIPT_NAME.to_string()],
            "solo queda el recibo, ningún temporal hermano"
        );

        // Sobrescribir un recibo existente también es atómico: el temporal se
        // va y el destino queda con el contenido nuevo.
        let mut other = receipt.clone();
        other.version = "0.25.0".to_string();
        write_to(&other, &dir).unwrap();
        assert_eq!(read_from(&dir).unwrap().unwrap().version, "0.25.0");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);

        // La fecha escrita es RFC 3339 UTC y se puede releer como instante.
        let now = unix_from_rfc3339(&loaded.installed_at).expect("`installed_at` es RFC 3339");
        let before = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        assert!(now <= before && before - now < 60, "instante razonable");

        // Sin recibo no hay instalación: se distingue de un recibo ilegible.
        assert_eq!(read_from(&scratch("receipt-empty")).unwrap(), None);
    }

    /// Un campo desconocido, en la raíz o anidado, hace ilegible el recibo y el
    /// error nombra el campo y el fichero.
    #[test]
    fn receipt_rejects_unknown_fields() {
        let dir = scratch("receipt-unknown");
        let base = serde_json::to_string_pretty(&sample_receipt()).expect("se serializa");
        for (anchor, extra) in [
            ("{", "{\n  \"campo_extra\": 1,"),
            (
                "\"path_integration\": {",
                "\"path_integration\": {\n    \"campo_extra\": 42,",
            ),
            ("\"roots\": {", "\"roots\": {\n    \"campo_extra\": null,"),
        ] {
            std::fs::write(receipt_path(&dir), base.replacen(anchor, extra, 1)).unwrap();
            let message = read_from(&dir).unwrap_err().to_string();
            assert!(
                message.contains("campo_extra") && message.contains(RECEIPT_NAME),
                "el error nombra el campo y el fichero: {message}"
            );
        }
    }

    /// Cada clave opcional debe estar presente (con `null` si no aplica): su
    /// ausencia es un recibo mal formado, no un `None`.
    #[test]
    fn receipt_requires_optional_keys() {
        let dir = scratch("receipt-missing-keys");
        for key in ["source", "symlink", "profile_blocks", "registry_entry"] {
            let mut value = serde_json::to_value(sample_receipt()).expect("se serializa");
            let object = if key == "source" {
                value.as_object_mut()
            } else {
                value["path_integration"].as_object_mut()
            }
            .expect("es un objeto");
            assert!(object.remove(key).is_some(), "el recibo escribe `{key}`");
            std::fs::write(receipt_path(&dir), value.to_string()).unwrap();
            let message = read_from(&dir).unwrap_err().to_string();
            assert!(
                message.contains(key),
                "sin `{key}` el recibo es ilegible: {message}"
            );
        }
    }

    /// Solo se admite el `schema_version` vigente: ni mayor, ni menor, ni
    /// ausente; y un texto que no es JSON tampoco se lee.
    #[test]
    fn receipt_requires_the_current_schema_version() {
        let dir = scratch("receipt-schema");
        for declared in ["0", "2"] {
            let text = serde_json::to_string(&sample_receipt())
                .expect("el recibo se serializa")
                .replacen(
                    "\"schema_version\":1",
                    &format!("\"schema_version\":{declared}"),
                    1,
                );
            std::fs::write(receipt_path(&dir), text).unwrap();
            let message = read_from(&dir).unwrap_err().to_string();
            assert!(
                message.contains(declared) && message.contains(RECEIPT_NAME),
                "el mensaje nombra la versión y el fichero: {message}"
            );
        }

        std::fs::write(receipt_path(&dir), "{\"app\":\"x\"}").unwrap();
        let err = read_from(&dir).unwrap_err();
        assert!(
            err.to_string().contains("schema_version"),
            "recibo sin schema_version: {err}"
        );

        std::fs::write(receipt_path(&dir), "no es JSON").unwrap();
        let err = read_from(&dir).unwrap_err();
        assert!(
            err.to_string().contains("JSON"),
            "recibo que no es JSON: {err}"
        );
    }

    /// Los valores efectivos de `roots` sobreviven a que la variable de
    /// reubicación deje de estar definida: la desinstalación opera sobre las
    /// mismas ubicaciones.
    #[test]
    fn receipt_roots_survive_missing_env() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = scratch("receipt-roots");
        let relocated = dir.join("relocated");
        std::env::set_var("AVI_DATA_DIR", relocated.join("data"));
        std::env::set_var("AVI_CACHE_DIR", relocated.join("cache"));
        let roots = Roots {
            data_dir: crate::data_dir().unwrap(),
            cache_dir: crate::models_cache_dir().unwrap(),
        };
        assert_eq!(roots.data_dir, relocated.join("data"), "la raíz se reubicó");

        let receipt = InstallReceipt::new(
            "0.24.0",
            crate::target::host_triple(),
            Channel::Script,
            &dir,
            vec!["ai-voice-interconnector".to_string()],
            PathIntegration::none(),
            roots.clone(),
            None,
        );
        write_to(&receipt, &dir).unwrap();

        // La variable desaparece: es el caso cuando el usuario borra su
        // entorno o cambia de máquina el directorio de programa.
        std::env::remove_var("AVI_DATA_DIR");
        std::env::remove_var("AVI_CACHE_DIR");
        assert_ne!(
            crate::data_dir().unwrap(),
            roots.data_dir,
            "sin la variable, la raíz se resuelve en otro sitio"
        );

        let loaded = read_from(&dir).unwrap().expect("el recibo sigue ahí");
        assert_eq!(
            loaded.roots, roots,
            "el recibo conserva los valores efectivos"
        );
    }
}
