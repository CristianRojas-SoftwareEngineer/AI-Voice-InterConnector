//! Limpieza de la cuarentena de macOS, que es el paso 7 de la instalación.
//!
//! Un bundle descargado con el navegador y extraído con el Finder llega a
//! `com.apple.quarantine` puesto en **todos** sus archivos. Quitarlo solo del
//! ejecutable no basta: el motor (`vendor/qwen3-tts/qwen_tts`)
//! se **ejecuta** y la librería de ONNX Runtime se **carga**, así que ambos siguen
//! heredando la cuarentena y macOS los bloquea igual. Por eso hay que quitarlo de
//! forma recursiva en todo el directorio de programa, no solo del ejecutable.
//!
//! **Lo único genuinamente de macOS es el atributo extendido**, que se lee y se borra
//! con `xattr`. Todo lo demás —la decisión de qué hacer con cada archivo, el recorrido
//! del directorio, el límite de profundidad, el resultado— **compila y se prueba en las
//! cuatro plataformas**, con la implementación de atributos inyectada. Eso no es
//! PURISMO: la puerta de macOS solo corre en el pipeline del release, así que un error
//! de compilación o de tipo en este módulo se descubriría al final de un trabajo de
//! horas. Con esta separación, el código que correrá en macOS está compilado y tipado
//! mucho antes, y lo único sin verificar es la llamada a `xattr`, que son cuatro
//! líneas.
//!
//! **Lo que sí es un no-op fuera de macOS, y es verificable**: `SinAtributos` responde
//! que ningún archivo tiene cuarentena, así que `strip` recorre el árbol, no limpia
//! nada, no falla y no toca el disco fuera del directorio. La función existe igual, con
//! el mismo tipo de resultado, porque `self install` la invoca sin preguntar por la
//! plataforma y porque el código que la usa tiene que ser el mismo en los
//! cuatro targets: la diferencia entre plataformas es el mecanismo, no la
//! experiencia.

use std::path::{Path, PathBuf};

/// Nombre del atributo extendido que la limpieza quita. Constante y no un
/// literal en las funciones para que la prueba pueda afirmar que es exactamente este.
pub const BLOCK_MARKER: &str = "com.apple.quarantine";

/// Resultado de la limpieza de cuarentena.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outcome {
    /// Archivos a los que se les quitó el atributo. Vacío cuando no había ninguno o
    /// cuando la plataforma no lo tiene.
    pub cleared: Vec<PathBuf>,
    /// Archivos en los que la operación falló. No es un fallo de la instalación: la
    /// cuarentena que no se quita degrada el arranque, y el usuario tiene derecho a
    /// saber cuáles son.
    pub failed: Vec<(PathBuf, String)>,
}

impl Outcome {
    /// `true` si no queda ningún archivo con cuarentena, incluidos los que fallaron:
    /// es la condición de que la instalación no deja cuarentena.
    pub fn is_clear(&self) -> bool {
        self.failed.is_empty()
    }

    /// `true` si no se limpió nada **y** no se cuenta ningún archivo como fallido. Es la
    /// forma de afirmar el comportamiento de las plataformas sin el atributo: la
    /// operación informa de que no hay nada que quitar y devuelve éxito.
    pub fn is_nothing_to_do(&self) -> bool {
        self.cleared.is_empty() && self.failed.is_empty()
    }
}

/// Atributos extendidos de un archivo.
///
/// **Esta es la única superficie dependiente de la plataforma del módulo.** Todo lo
/// demás —el recorrido, el límite de profundidad, la contabilidad del resultado— es
/// código normal, y se puede ejercitar en Windows y en Linux con una implementación
/// falsa que registre a qué se le pregunta.
pub trait Quarantine {
    /// ¿Tiene el archivo el atributo de cuarentena?
    fn has(&self, path: &Path) -> std::io::Result<bool>;

    /// Quita el atributo. `Ok(true)` si lo tenía, `Ok(false)` si no lo tenía, `Err` si
    /// la operación falló.
    fn clear(&self, path: &Path) -> std::io::Result<bool>;

    /// Pone el atributo.
    ///
    /// **Solo lo usan las pruebas**, y es la condición de que signifiquen algo: sin poder poner la cuarentena, se ejecutarían sobre un bundle que no la
    /// tiene y pasarían sin comprobar nada.
    fn put(&self, path: &Path) -> std::io::Result<()>;
}

/// Quita `com.apple.quarantine` de forma recursiva en `dir`, con los atributos de la
/// plataforma en la que corre.
///
/// El límite de profundidad es una defensa: un directorio de programa con un lazo de
/// directorios —un enlace simbólico a un ancestro, por ejemplo— no puede dejar la
/// limpieza colgando. La profundidad real de un bundle es de tres niveles
/// (`vendor/qwen3-tts/qwen_tts`), así que el margen es amplio.
pub fn strip(dir: &Path) -> Outcome {
    strip_bounded(dir, MAX_DEPTH, platform())
}

/// [`strip`] con el límite de profundidad y la implementación de atributos como
/// parámetros, que es lo que permite probar el recorrido en cualquier plataforma.
pub fn strip_bounded(dir: &Path, max_depth: usize, attributes: &dyn Quarantine) -> Outcome {
    let mut outcome = Outcome::default();
    walk(dir, 0, max_depth, attributes, &mut outcome);
    outcome
}

/// Profundidad máxima de la limpieza recursiva.
pub const MAX_DEPTH: usize = 32;

/// Recorrido del directorio de programa. Compila y corre en todas las plataformas.
///
/// Solo **lee** el árbol: pregunta por el atributo de cada archivo y, si lo hay, pide
/// quitarlo. Nunca escribe fuera de `dir` y nunca decide qué es de la aplicación por su
/// nombre: el directorio que se le pasa **es** el directorio de programa, y eso lo hace
/// responsable de todo lo que hay dentro.
fn walk(
    dir: &Path,
    depth: usize,
    max_depth: usize,
    attributes: &dyn Quarantine,
    outcome: &mut Outcome,
) {
    if depth > max_depth {
        return;
    }
    // El recorrido no sigue enlaces: `DirEntry::metadata` describe el enlace mismo, así
    // que solo se desciende a directorios reales y un enlace a un ancestro no forma
    // lazo ni produce errores de demasiados niveles de enlaces al listar. Por eso
    // cualquier fallo de listado o de metadatos es un fallo real y queda en `failed`:
    // la limpieza no puede afirmar que lo que hay dentro está limpio.
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            outcome.failed.push((dir.to_path_buf(), e.to_string()));
            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                outcome.failed.push((dir.to_path_buf(), e.to_string()));
                continue;
            }
        };
        let path = entry.path();
        let meta = match entry.metadata() {
            Ok(meta) => meta,
            Err(e) => {
                outcome.failed.push((path, e.to_string()));
                continue;
            }
        };
        // Al archivo o enlace sí se le pide el atributo por su ruta: un `qwen_tts`
        // enlazado a un bundle real hay que limpiarlo igual que el resto.
        match attributes.clear(&path) {
            Ok(true) => outcome.cleared.push(path.clone()),
            Ok(false) => {}
            // Un fallo **no** corta el recorrido: los otros archivos también necesitan
            // quedarse limpios, y ninguno puede quedarse con cuarentena sin constar.
            Err(e) => outcome.failed.push((path.clone(), e.to_string())),
        }
        if meta.is_dir() {
            walk(&path, depth + 1, max_depth, attributes, outcome);
        }
    }
}

/// Los atributos de la plataforma en la que corre el binario.
pub fn platform() -> &'static dyn Quarantine {
    #[cfg(target_os = "macos")]
    {
        &Xattr
    }
    #[cfg(not(target_os = "macos"))]
    {
        &NoAttributes
    }
}

/// Atributos extendidos de macOS, con `xattr`, que es la herramienta del sistema que
/// los gestiona. No hay crate nueva por esto: `xattr` viene con macOS.
#[cfg(target_os = "macos")]
pub struct Xattr;

/// Ruta fija del `xattr` del sistema: un `xattr` puesto antes en el `PATH` no lo
/// suplanta, y si falta la operación falla.
#[cfg(target_os = "macos")]
const XATTR: &str = "/usr/bin/xattr";

#[cfg(target_os = "macos")]
impl Quarantine for Xattr {
    fn has(&self, path: &Path) -> std::io::Result<bool> {
        // `xattr -p` devuelve 1 cuando el atributo no está, y ese 1 no es un error: es
        // la respuesta. Por eso se consulta antes de borrar en lugar de interpretar el
        // fallo de `xattr -d` como "no lo tenía".
        let output = std::process::Command::new(XATTR)
            .arg("-p")
            .arg(BLOCK_MARKER)
            .arg(path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()?;
        Ok(output.success())
    }

    fn clear(&self, path: &Path) -> std::io::Result<bool> {
        if !self.has(path)? {
            return Ok(false);
        }
        let output = std::process::Command::new(XATTR)
            .arg("-d")
            .arg(BLOCK_MARKER)
            .arg(path)
            .output()?;
        if output.status.success() {
            Ok(true)
        } else {
            Err(std::io::Error::other(
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ))
        }
    }

    fn put(&self, path: &Path) -> std::io::Result<()> {
        // El valor es un sello de 16 dígitos hexadecimales, que es lo que pone el
        // navegador; a `xattr` le da igual el contenido, solo la existencia.
        let output = std::process::Command::new(XATTR)
            .arg("-w")
            .arg(BLOCK_MARKER)
            .arg("0000-0000-0000-0000")
            .arg(path)
            .output()?;
        if output.status.success() {
            Ok(())
        } else {
            Err(std::io::Error::other(
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ))
        }
    }
}

/// La implementación fuera de macOS: no hay `com.apple.quarantine`, así que no hay nada
/// que quitar.
///
/// No es un `cfg` en el recorrido sino una implementación más, y esa es la diferencia:
/// el recorrido **se ejecuta igual** en Windows y en Linux, se comprueba que no pregunta
/// fuera del directorio y se comprueba que no toca el disco. Un `cfg` que devolviera
/// `Outcome::default()` no afirmaría nada de eso.
#[cfg(not(target_os = "macos"))]
pub struct NoAttributes;

#[cfg(not(target_os = "macos"))]
impl Quarantine for NoAttributes {
    fn has(&self, _path: &Path) -> std::io::Result<bool> {
        Ok(false)
    }

    fn clear(&self, _path: &Path) -> std::io::Result<bool> {
        Ok(false)
    }

    fn put(&self, _path: &Path) -> std::io::Result<()> {
        Ok(())
    }
}

/// ¿Tiene el archivo el atributo de cuarentena? Lo usan las pruebas para afirmar tanto
/// el punto de partida como el punto de llegada, y para ponerlo, [`Quarantine::put`].
pub fn has(path: &Path) -> std::io::Result<bool> {
    platform().has(path)
}

/// Pone el atributo de cuarentena en un archivo. **Solo para las pruebas**; ver
/// [`Quarantine::put`].
pub fn put(path: &Path) -> std::io::Result<()> {
    platform().put(path)
}
