//! Recibo de instalación: qué se instaló, dónde, por qué canal y qué
//! integración de `PATH` se aplicó (§8.1).
//!
//! El recibo es lo que permite reparar una instalación, revertir exactamente
//! lo que se hizo en `PATH` al desinstalar y operar sobre la instalación
//! registrada aunque el comando lo ejecute otra copia del binario (§8.2). Vive
//! dentro del directorio de programa y se escribe de forma atómica, con fichero
//! temporal hermano y renombrado, de modo que un corte a mitad no deje un
//! recibo corrupto que la siguiente ejecución no pueda leer.
//!
//! **Lectura tolerante**: los campos desconocidos se ignoran, para que un
//! binario antiguo pueda leer un recibo nuevo. Un `schema_version` mayor que el
//! soportado aborta la operación con un mensaje que pide actualizar, porque un
//! binario antiguo no puede saber qué hizo con el disco el nuevo.
//!
//! **Los valores efectivos de las raíces se registran** (`roots`): aunque la
//! variable de reubicación deje de estar definida, la actualización y la
//! desinstalación operan sobre las mismas ubicaciones (§7, párrafo de
//! reubicación).

use crate::channel::Channel;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Nombre del recibo dentro del directorio de programa (§8.1).
pub const RECEIPT_NAME: &str = "install-receipt.json";

/// Versión del esquema que esta versión del motor sabe leer.
pub const RECEIPT_SCHEMA_VERSION: u32 = 1;

/// Integración de `PATH` efectivamente aplicada (§8.1 y §9.3.1). Los tres
/// campos son mutuamente excluyentes según la plataforma: en Unix el enlace
/// simbólico y los bloques de perfil, en Windows la entrada de registro; lo que
/// no aplique es `null` y no una cadena vacía, para que se distinga "no se
/// integró" de "se integró a vacío".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathIntegration {
    /// `false` si se pasó `--no-modify-path`: el comando se invoca por su ruta
    /// completa y no se toca ningún perfil ni el registro.
    pub modify_path: bool,
    /// Enlace simbólico del directorio del enlace, en Unix.
    #[serde(default)]
    pub symlink: Option<PathBuf>,
    /// Archivos de arranque de shell con el bloque delimitado, en Unix.
    #[serde(default)]
    pub profile_blocks: Option<Vec<PathBuf>>,
    /// Entrada añadida al valor `Path` de `HKCU\Environment`, en Windows.
    #[serde(default)]
    pub registry_entry: Option<PathBuf>,
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
/// instalación (§7, reubicación). Se guardan como valores efectivos, no como
/// nombres de variable, para que una desinstalación posterior no dependa de que
/// la variable siga definida.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Roots {
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
}

/// Recibo de instalación completo, con la estructura de §8.1 y sin campos
/// inventados.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    /// Instante de la instalación en RFC 3339 UTC, que es lo que muestra el
    /// ejemplo de §8.1.
    pub installed_at: String,
    /// Origen del bundle, si la instalación vino de un archivo publicado; vacío
    /// para una reparación o para el canal `dev`.
    #[serde(default)]
    pub source: Option<String>,
}

impl InstallReceipt {
    /// Recibo de una instalación nueva. `roots` y `install_dir` se pasan ya
    /// resueltos: quien escribe es quien sabe qué valor efectivo se aplicó.
    // Los ocho campos son los de §8.1 que el instalador conoce uno a uno; un
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

/// Raíces efectivas de una instalación registrada: las del recibo si existe, y
/// las que se resuelven ahora si no hay recibo.
///
/// Es lo que permite que `self update` y `self uninstall` operen sobre la
/// instalación registrada sea cual sea la copia del binario que ejecuta el
/// comando (§8.2), aunque la variable de reubicación ya no esté definida.
pub fn effective_roots(receipt: Option<&InstallReceipt>) -> Roots {
    match receipt {
        Some(receipt) => receipt.roots.clone(),
        None => Roots {
            data_dir: crate::data_dir(),
            cache_dir: crate::models_cache_dir(),
        },
    }
}

/// Ruta del recibo dentro del directorio de programa.
pub fn receipt_path(program_dir: &Path) -> PathBuf {
    program_dir.join(RECEIPT_NAME)
}

/// Lee el recibo del directorio de programa. `Ok(None)` si no hay recibo: una
/// instalación sin recibo **no se adopta** (§14.2), se trata como no instalada.
pub fn read_from(program_dir: &Path) -> anyhow::Result<Option<InstallReceipt>> {
    let path = receipt_path(program_dir);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    Ok(Some(parse(&text)?))
}

/// Parsea un recibo, exigiendo que su `schema_version` sea compatible.
pub fn parse(text: &str) -> anyhow::Result<InstallReceipt> {
    let value: serde_json::Value = serde_json::from_str(text)
        .map_err(|e| anyhow::anyhow!("el recibo de instalación no es JSON válido: {e}"))?;
    let declared = value
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| {
            anyhow::anyhow!("el recibo de instalación no declara `schema_version`: {text}")
        })?;
    if declared > u64::from(RECEIPT_SCHEMA_VERSION) {
        return Err(anyhow::anyhow!(
            "el recibo declara schema_version {declared} y esta versión de {} solo \
             entiende hasta {RECEIPT_SCHEMA_VERSION}: actualiza {} para gestionarlo",
            crate::APP_NAME,
            crate::APP_NAME
        ));
    }
    Ok(serde_json::from_value(value)?)
}

/// Escribe el recibo de forma atómica: temporal hermano, escritura, renombrado
/// sobre el destino. Un corte a mitad deja el recibo anterior intacto.
pub fn write_to(receipt: &InstallReceipt, program_dir: &Path) -> anyhow::Result<()> {
    let destino = receipt_path(program_dir);
    let temporal = program_dir.join(format!("{RECEIPT_NAME}.tmp-{}", std::process::id()));
    let texto = serde_json::to_string_pretty(receipt)?;
    std::fs::write(&temporal, texto.as_bytes())?;
    match std::fs::rename(&temporal, &destino) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&temporal);
            Err(e.into())
        }
    }
}

/// Instante actual en RFC 3339 UTC (`2026-09-25T18:00:00Z`), el formato del
/// ejemplo de §8.1. Sin `chrono` ni `time` en el árbol, y añadir una dependencia
/// por una fecha no lo compensa: el cálculo es de día juliano a fecha civil.
///
/// Lo comparte el pidfile del daemon, que también lleva `started_at` en el mismo
/// formato; por eso es `pub(crate)` y no privado.
pub(crate) fn now_rfc3339() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default();
    let (year, month, day, hour, minute, second) = civil_from_unix(secs);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Descompone un instante Unix en fecha civil UTC. Algoritmo dedays-from-civil,
/// que es exacto para cualquier fecha del calendario gregoriano.
fn civil_from_unix(secs: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    (
        year,
        m as u32,
        d as u32,
        (rem / 3600) as u32,
        ((rem % 3600) / 60) as u32,
        (rem % 60) as u32,
    )
}

/// Inversa de `civil_from_unix`, para las pruebas del formato: no hace falta
/// ningún crate de fechas para afirmar que lo escrito se puede volver a leer.
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
    let doy = (153 * mp + 2) / 5 + day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hour * 3600 + minute * 60 + second)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{scratch, ENV_LOCK};

    fn recibo_de_ejemplo() -> InstallReceipt {
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
        let recibo = recibo_de_ejemplo();
        write_to(&recibo, &dir).unwrap();

        let leido = read_from(&dir)
            .unwrap()
            .expect("el recibo está donde se escribió");
        assert_eq!(
            leido, recibo,
            "el recibo sobrevive al viaje de ida y vuelta"
        );
        assert_eq!(leido.schema_version, RECEIPT_SCHEMA_VERSION);
        assert_eq!(leido.app, crate::APP_NAME);
        assert_eq!(leido.channel, Channel::Script);
        assert_eq!(leido.channel.as_str(), "script");

        // El temporal no sobrevive al renombrado.
        let contenidos: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(
            contenidos,
            vec![RECEIPT_NAME.to_string()],
            "solo queda el recibo, ningún temporal hermano"
        );

        // Sobrescribir un recibo existente también es atómico: el temporal se
        // va y el destino queda con el contenido nuevo.
        let mut otro = recibo.clone();
        otro.version = "0.25.0".to_string();
        write_to(&otro, &dir).unwrap();
        assert_eq!(read_from(&dir).unwrap().unwrap().version, "0.25.0");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);

        // La fecha escrita es RFC 3339 UTC y se puede releer como instante.
        let ahora = unix_from_rfc3339(&leido.installed_at).expect("`installed_at` es RFC 3339");
        let antes = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        assert!(ahora <= antes && antes - ahora < 60, "instante razonable");

        // Sin recibo no hay instalación: se distingue de un recibo ilegible.
        assert_eq!(read_from(&scratch("receipt-vacio")).unwrap(), None);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Los campos desconocidos se ignoran: un binario antiguo puede leer un
    /// recibo escrito por uno nuevo.
    #[test]
    fn receipt_ignores_unknown_fields() {
        let dir = scratch("receipt-desconocidos");
        let mut texto =
            serde_json::to_string_pretty(&recibo_de_ejemplo()).expect("el recibo se serializa");
        // Campo nuevo en la raíz, en `path_integration` y en `roots`.
        texto = texto.replacen(
            "{",
            "{\n  \"campo_del_futuro\": {\"anidado\": [1, 2, 3]},",
            1,
        );
        texto = texto.replacen(
            "\"path_integration\": {",
            "\"path_integration\": {\n    \"future_field\": 42,",
            1,
        );
        texto = texto.replacen(
            "\"roots\": {",
            "\"roots\": {\n    \"future_root\": null,",
            1,
        );
        std::fs::write(receipt_path(&dir), texto).unwrap();

        let mut leido = read_from(&dir).unwrap().expect("el recibo se lee");
        let original = recibo_de_ejemplo();
        // `installed_at` se afirma por separado: dos recibos construidos en el
        // mismo segundo tienen la misma fecha, y en el siguiente no, así que
        // compararlo aquí haría la prueba intermitente.
        leido.installed_at = original.installed_at.clone();
        assert_eq!(leido, original, "lo desconocido no altera lo conocido");
        assert_eq!(
            leido.path_integration.symlink,
            original.path_integration.symlink
        );

        // Y en el otro sentido: un recibo sin `source` (campo opcional) también
        // se lee, porque es opcional por definición.
        let mut sin_source = recibo_de_ejemplo();
        sin_source.source = None;
        write_to(&sin_source, &dir).unwrap();
        assert_eq!(read_from(&dir).unwrap().unwrap().source, None);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Un `schema_version` mayor aborta con un mensaje que pide actualizar, y
    /// uno menor se lee.
    #[test]
    fn receipt_rejects_newer_schema() {
        let dir = scratch("receipt-schema");
        let mut texto =
            serde_json::to_string(&recibo_de_ejemplo()).expect("el recibo se serializa");
        texto = texto.replacen("\"schema_version\":1", "\"schema_version\":2", 1);
        std::fs::write(receipt_path(&dir), texto).unwrap();
        let err = read_from(&dir).unwrap_err();
        let mensaje = err.to_string();
        assert!(
            mensaje.contains('2') && mensaje.contains("actualiza"),
            "el mensaje nombra la versión y pide actualizar: {mensaje}"
        );
        assert!(
            read_from(&dir).is_err(),
            "un recibo más nuevo aborta la operación"
        );

        // Sin `schema_version` tampoco hay recibo utilizable: se dice por qué.
        std::fs::write(receipt_path(&dir), "{\"app\":\"x\"}").unwrap();
        let err = read_from(&dir).unwrap_err();
        assert!(
            err.to_string().contains("schema_version"),
            "recibo sin schema_version: {err}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Los valores efectivos de `roots` sobreviven a que la variable de
    /// reubicación deje de estar definida: la desinstalación opera sobre las
    /// mismas ubicaciones.
    #[test]
    fn receipt_roots_survive_missing_env() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = scratch("receipt-roots");
        let reubicadas = dir.join("reubicadas");
        std::env::set_var("AVI_DATA_DIR", reubicadas.join("data"));
        std::env::set_var("AVI_CACHE_DIR", reubicadas.join("cache"));
        let roots = Roots {
            data_dir: crate::data_dir(),
            cache_dir: crate::models_cache_dir(),
        };
        assert_eq!(
            roots.data_dir,
            reubicadas.join("data"),
            "la raíz se reubicó"
        );

        let recibo = InstallReceipt::new(
            "0.24.0",
            crate::target::host_triple(),
            Channel::Dev,
            &dir,
            vec!["ai-voice-interconnector".to_string()],
            PathIntegration::none(),
            roots.clone(),
            None,
        );
        write_to(&recibo, &dir).unwrap();

        // La variable desaparece: es el caso de §7 cuando el usuario borra su
        // entorno o cambia de máquina el directorio de programa.
        std::env::remove_var("AVI_DATA_DIR");
        std::env::remove_var("AVI_CACHE_DIR");
        assert_ne!(
            crate::data_dir(),
            roots.data_dir,
            "sin la variable, la raíz se resuelve en otro sitio"
        );

        let leido = read_from(&dir).unwrap().expect("el recibo sigue ahí");
        assert_eq!(
            leido.roots, roots,
            "el recibo conserva los valores efectivos"
        );
        let efectivas = effective_roots(Some(&leido));
        assert_eq!(efectivas, roots, "y son los que se usan para operar");
        assert_ne!(
            effective_roots(None),
            roots,
            "sin recibo se resuelven las de ahora, que no son las de la instalación"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
