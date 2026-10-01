//! Reemplazo transaccional del directorio de programa y su diario.
//!
//! El instalador heredado borra el directorio de programa antes de extraer, así
//! que un fallo de extracción deja al usuario sin programa y sin vuelta atrás,
//! y en Windows un ejecutable en uso aborta la operación con el directorio a
//! medio borrar. El algoritmo es el mismo en los cuatro targets:
//!
//! 1. **Aparcar** el contenido actual en `<programa>/.old-<txid>/` por renombrado,
//!    registrándolo en el diario. Aparcar en vez de borrar es lo que hace el
//!    reemplazo reversible, y en Windows es lo único posible con un ejecutable
//!    en uso: renombrar se permite, borrar no.
//! 2. **Colocar** el bundle nuevo. Los archivos se mueven desde el origen por
//!    renombrado —el staging es hermano del directorio de programa, luego es el
//!    mismo volumen—, salvo el ejecutable que está corriendo, que se copia.
//! 3. **Ajustar permisos**: en Unix, 0755 para ejecutables y 0644 para el resto.
//! 4. **Confirmar**: marcar el diario como confirmado y borrar `.old-<txid>/`.
//!    Lo que no se pueda borrar por estar en uso queda para el borrado diferido
//!    (borrado diferido), que es lo que impide que el commit falle por un archivo
//!    abierto.
//! 5. **Revertir** si falla el paso 2 o el 3: retirar lo colocado, restaurar lo
//!    aparcado, borrar el diario → `rolled_back`.
//!
//! **El diario se escribe antes de cada paso**, no después, y de forma atómica:
//! una interrupción en cualquier punto deja el disco en un estado que
//! [`crate::recovery`] sabe completar. El diario vive dentro del directorio de
//! programa, así que se escribe una vez aparcado el contenido: por eso no se
//! aparca a sí mismo, ni se aparcan los aparcados de una transacción anterior,
//! que los barre la recuperación antes de empezar.

use crate::faults::{self, FaultPoint};
use crate::LifecycleError;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Nombre del diario de transacción dentro del directorio de programa.
pub const JOURNAL_NAME: &str = ".transaction.json";

/// Versión del esquema de diario que esta versión del motor sabe leer.
pub const JOURNAL_SCHEMA_VERSION: u32 = 1;

/// Estado de la transacción en el punto en que se escribió el diario.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JournalState {
    /// El diario existe y no se ha aparcado nada todavía.
    Started,
    /// Contenido actual aparcado, bundle nuevo sin colocar.
    Parked,
    /// Bundle colocado, permisos sin ajustar.
    Placed,
    /// Permisos ajustados, sin confirmar.
    Permissions,
    /// Transacción confirmada: a partir de aquí, una interrupción se completa
    /// como commit y nunca como rollback.
    Committed,
}

impl JournalState {
    /// `true` si la transacción llegó a confirmarse.
    pub fn is_committed(self) -> bool {
        matches!(self, Self::Committed)
    }
}

/// Diario de transacción: qué se está haciendo, dónde está lo aparcado y qué se
/// ha colocado.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Journal {
    pub schema_version: u32,
    pub txid: String,
    pub state: JournalState,
    /// Directorio `.old-<txid>/` con el contenido anterior, si ya se aparcó.
    #[serde(default)]
    pub parked_dir: Option<PathBuf>,
    /// Rutas relativas colocadas, necesarias para el rollback.
    #[serde(default)]
    pub placed: Vec<String>,
    /// Directorio de origen del bundle. La colocación es un renombrado, así que
    /// revertir devuelve cada archivo a su sitio: sin esto, un fallo a mitad
    /// dejaría el staging vacío y el reintento instalaría nada.
    #[serde(default)]
    pub source_dir: Option<PathBuf>,
}

/// Resultado de un reemplazo confirmado.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplaceOutcome {
    pub txid: String,
    /// Directorio de programa donde quedó el bundle.
    pub program_dir: PathBuf,
    /// Rutas relativas colocadas, en el orden en que se colocaron.
    pub placed: Vec<String>,
    /// Del aparcado que no se pudo borrar por estar en uso. No es un fallo: queda
    /// para el borrado diferido y la recuperación lo recoge.
    pub leftovers: Vec<PathBuf>,
}

/// Ruta del diario en el directorio de programa.
pub fn journal_path(program_dir: &Path) -> PathBuf {
    program_dir.join(JOURNAL_NAME)
}

/// Lee el diario si existe.
pub fn read_journal(program_dir: &Path) -> anyhow::Result<Option<Journal>> {
    let path = journal_path(program_dir);
    match std::fs::read_to_string(&path) {
        Ok(text) => Ok(Some(serde_json::from_str(&text)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Reemplaza el contenido de `program_dir` por el bundle de `source_dir`
/// siguiendo el algoritmo del módulo.
///
/// Falla con `rolled_back` y la versión anterior restaurada si algo falla tras
/// abrir el diario.
///
/// `source_dir` debe estar en el mismo volumen que `program_dir`, que es lo que
/// que el staging cumple por ser hermano del
/// directorio de programa. Si no fuera así, el renombrado de la colocación
/// fallaría, la transacción revertiría y no se tocaría la versión anterior.
pub fn replace(program_dir: &Path, source_dir: &Path) -> anyhow::Result<ReplaceOutcome> {
    let txid = new_txid();
    std::fs::create_dir_all(program_dir)?;
    let parked = program_dir.join(format!("{}{txid}", crate::PARKED_DIR_PREFIX));
    let mut journal = Journal {
        schema_version: JOURNAL_SCHEMA_VERSION,
        txid: txid.clone(),
        state: JournalState::Started,
        parked_dir: None,
        placed: Vec::new(),
        source_dir: Some(source_dir.to_path_buf()),
    };
    // Un fallo escribiendo el diario no es reversible y no se puede reportar como
    // `rolled_back`: en ese punto no se ha tocado nada, así que el error sube tal
    // cual y la siguiente operación no encuentra transacción pendiente.
    write_journal(program_dir, &journal)?;

    match run(&mut journal, program_dir, source_dir, &parked) {
        Ok(outcome) => Ok(outcome),
        Err(cause) => {
            rollback(program_dir, &journal)
                .map_err(|e| anyhow::anyhow!("fallo al revertir: {e}; además: {cause}"))?;
            Err(LifecycleError::rolled_back(format!(
                "fallo al reemplazar {}: {cause}; se restauró la versión anterior",
                program_dir.display()
            ))
            .into())
        }
    }
}

/// Los cuatro pasos sobre un diario ya abierto.
fn run(
    journal: &mut Journal,
    program_dir: &Path,
    source_dir: &Path,
    parked: &Path,
) -> anyhow::Result<ReplaceOutcome> {
    faults::trip(FaultPoint::BeforePark)?;

    // 1. Aparcar por renombrado.
    let parked_entries = park_current_contents(program_dir, parked)?;
    if !parked_entries.is_empty() {
        journal.parked_dir = Some(parked.to_path_buf());
        journal.state = JournalState::Parked;
        write_journal(program_dir, journal)?;
    }

    faults::trip(FaultPoint::BeforePlace)?;

    // 2. Colocar. El diario declara qué se va a colocar **antes** de colocar
    //    nada: una interrupción a mitad del paso 2 debe saber qué retirar.
    let placed = relative_entries(source_dir)?;
    journal.placed = placed.clone();
    journal.state = JournalState::Placed;
    write_journal(program_dir, journal)?;

    for relative in &placed {
        place_entry(source_dir, program_dir, relative)?;
    }

    faults::trip(FaultPoint::BeforeFixPermissions)?;

    // 3. Ajustar permisos.
    fix_permissions(program_dir, &placed);
    journal.state = JournalState::Permissions;
    write_journal(program_dir, journal)?;

    faults::trip(FaultPoint::BeforeCommit)?;

    // 4. Confirmar y borrar el aparcado. A partir de aquí la transacción está
    //    confirmada: una interrupción posterior se completa como commit, nunca
    //    como rollback.
    journal.state = JournalState::Committed;
    write_journal(program_dir, journal)?;

    let mut leftovers = Vec::new();
    if parked.exists() {
        match std::fs::remove_dir_all(parked) {
            Ok(()) => {}
            Err(_) => {
                // Borrado diferido: lo que el SO no deja borrar, por estar
                // en uso, no impide el commit.
                leftovers = leftovers_in(parked);
            }
        }
    }
    let _ = std::fs::remove_file(journal_path(program_dir));

    Ok(ReplaceOutcome {
        txid: journal.txid.clone(),
        program_dir: program_dir.to_path_buf(),
        placed,
        leftovers,
    })
}

/// Renombra a `<programa>/.old-<txid>/` todo lo que hay en el directorio de
/// programa, y devuelve los nombres aparcados.
///
/// No aparca el diario (se escribiría dentro de sí mismo) ni los aparcados de
/// una transacción anterior, que la recuperación barre antes de empezar.
fn park_current_contents(program_dir: &Path, parked: &Path) -> anyhow::Result<Vec<String>> {
    let mut parked_entries = Vec::new();
    let entries = match std::fs::read_dir(program_dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(parked_entries),
        Err(e) => return Err(e.into()),
    };
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if name == JOURNAL_NAME || name.starts_with(crate::PARKED_DIR_PREFIX) {
            continue;
        }
        std::fs::create_dir_all(parked)?;
        std::fs::rename(entry.path(), parked.join(&name))?;
        parked_entries.push(name);
    }
    Ok(parked_entries)
}

/// Rutas relativas de todo lo que hay en `source_dir`, en profundidad, con
/// separador `/` para que el diario y el recibo usen la misma forma que el
/// manifiesto.
fn relative_entries(source_dir: &Path) -> anyhow::Result<Vec<String>> {
    let mut out = Vec::new();
    let mut stack = vec![source_dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let mut children: Vec<PathBuf> = std::fs::read_dir(&dir)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .collect();
        children.sort();
        for path in children {
            if path.is_dir() {
                stack.push(path);
            } else {
                let relative = path
                    .strip_prefix(source_dir)
                    .expect("todas las rutas cuelgan del origen")
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push(relative);
            }
        }
    }
    out.sort();
    Ok(out)
}

/// Coloca una entrada del bundle. Se mueve por renombrado, salvo cuando el
/// destino es el ejecutable que está corriendo, que se copia: en Windows un
/// ejecutable en ejecución no se puede renombrar ni borrar, y el proceso que lo
/// sustituye tiene que poder seguir vivo hasta que termine.
fn place_entry(source_dir: &Path, program_dir: &Path, relative: &str) -> anyhow::Result<()> {
    let source = source_dir.join(to_platform_path(relative));
    let dest = program_dir.join(to_platform_path(relative));
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if is_running_executable(&dest) {
        std::fs::copy(&source, &dest)?;
    } else {
        std::fs::rename(&source, &dest)?;
    }
    Ok(())
}

/// `true` si `path` es el ejecutable que está corriendo ahora mismo. La
/// comparación es la canónica de entradas de `avi-store` porque en Windows dos
/// rutas al mismo fichero pueden diferir en mayúsculas.
fn is_running_executable(path: &Path) -> bool {
    let actual = match std::env::current_exe() {
        Ok(actual) => actual,
        Err(_) => return false,
    };
    crate::canonical_path_entry_matches(path, &actual)
}

/// 0755 para lo ejecutable y 0644 para el resto, en Unix. En Windows
/// no hay permisos que ajustar.
#[cfg(unix)]
fn fix_permissions(program_dir: &Path, placed: &[String]) {
    use crate::{manifest, target};
    use std::os::unix::fs::PermissionsExt;
    let executable = manifest::target_section(target::host_triple())
        .map(|section| section.executable)
        .unwrap_or_else(|_| crate::APP_NAME.to_string());
    for relative in placed {
        let path = program_dir.join(to_platform_path(relative));
        // `relative` es `&String` y `ejecutable` es `String`: sin el desreferenciado
        // `PartialEq` no se resuelve y el crate no compila en Unix. El defecto era
        // invisible en Windows, donde esta función no existe.
        let permissions = if *relative == executable || relative.starts_with("vendor/") {
            0o755
        } else {
            0o644
        };
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(permissions));
    }
}

#[cfg(not(unix))]
fn fix_permissions(_program_dir: &Path, _placed: &[String]) {}

/// Devuelve lo que queda dentro de un directorio que no se pudo borrar, para
/// informarlo como borrado diferido.
fn leftovers_in(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            out.push(entry.path());
        }
    }
    out.sort();
    out
}

/// Revierte la transacción: retira lo colocado, restaura lo aparcado y borra el
/// diario. Es el quinto paso y lo que ejecuta la recuperación cuando
/// encuentra un diario sin confirmar.
///
/// Lo colocado se devuelve **al origen** cuando el origen sigue existiendo y ya
/// no tiene ese archivo: la colocación es un renombrado, así que retirarlo sin
/// devolverlo dejaría el staging a medias y el reintento instalaría un bundle
/// incompleto. Si el origen ya tiene el archivo —que es el caso del ejecutable
/// en ejecución, que se copió y no se movió— se borra la copia del programa.
pub(crate) fn rollback(program_dir: &Path, journal: &Journal) -> anyhow::Result<()> {
    for relative in &journal.placed {
        let placed = program_dir.join(to_platform_path(relative));
        match journal
            .source_dir
            .as_ref()
            .map(|s| s.join(to_platform_path(relative)))
        {
            Some(source) if !source.exists() => {
                if let Some(parent) = source.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::rename(&placed, &source)?;
            }
            _ => match std::fs::remove_file(&placed) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            },
        }
    }
    if let Some(parked) = &journal.parked_dir {
        if parked.is_dir() {
            for entry in std::fs::read_dir(parked)? {
                let entry = entry?;
                restore_entry(&entry.path(), &program_dir.join(entry.file_name()))?;
            }
            std::fs::remove_dir_all(parked)?;
        }
    }
    let _ = std::fs::remove_file(journal_path(program_dir));
    Ok(())
}

/// Devuelve una entrada aparcada a su sitio, fusionando si el destino ya existe.
///
/// En Windows `fs::rename` no puede reemplazar un directorio por otro, y el caso
/// de `vendor/` se da en cada reversión: el motor de la versión nueva ya está
/// colocado cuando se restaura el de la anterior. Fusionar hoja a hoja es lo que
/// deja el directorio de programa con exactamente la versión anterior.
fn restore_entry(source: &Path, dest: &Path) -> anyhow::Result<()> {
    if !dest.exists() {
        return Ok(std::fs::rename(source, dest)?);
    }
    if source.is_dir() && dest.is_dir() {
        for entry in std::fs::read_dir(source)? {
            let entry = entry?;
            restore_entry(&entry.path(), &dest.join(entry.file_name()))?;
        }
        return Ok(std::fs::remove_dir(source)?);
    }
    if dest.is_dir() {
        std::fs::remove_dir_all(dest)?;
    } else {
        std::fs::remove_file(dest)?;
    }
    Ok(std::fs::rename(source, dest)?)
}

/// Escribe el diario de forma atómica: temporal hermano y renombrado, porque un
/// diario a medio escribir es peor que no tener diario.
fn write_journal(program_dir: &Path, journal: &Journal) -> anyhow::Result<()> {
    let dest = journal_path(program_dir);
    let temp = program_dir.join(format!("{JOURNAL_NAME}.tmp-{}", std::process::id()));
    let text = serde_json::to_string_pretty(journal)?;
    std::fs::write(&temp, text.as_bytes())?;
    match std::fs::rename(&temp, &dest) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&temp);
            Err(e.into())
        }
    }
}

/// Convierte una ruta relativa del manifiesto (`/`) en una ruta de la plataforma.
fn to_platform_path(relative: &str) -> PathBuf {
    let mut path = PathBuf::new();
    for part in relative.split('/') {
        path.push(part);
    }
    path
}

/// Identificador de transacción: proceso e instante. No necesita ser único más
/// allá del directorio de programa —ahí el proceso ya identifica a quien
/// instala— ni aleatorio, solo tiene que distinguir dos transacciones seguidas
/// dentro del mismo directorio.
fn new_txid() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("{}-{}", std::process::id(), nanos)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{scratch, write_file};
    use crate::{manifest, target};

    /// Monta un bundle completo del target del host en `dir`.
    fn bundle(dir: &Path, content: &str) {
        for relative in &manifest::target_section(target::host_triple())
            .unwrap()
            .required
        {
            write_file(&dir.join(to_platform_path(relative)), content);
        }
    }

    /// Nombres de lo que hay en `dir` con el prefijo de aparcado, ordenados.
    fn parked(program_dir: &Path) -> Vec<String> {
        let mut out: Vec<String> = std::fs::read_dir(program_dir)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|e| e.file_name().to_string_lossy().to_string())
                    .filter(|name| name.starts_with(crate::PARKED_DIR_PREFIX))
                    .collect()
            })
            .unwrap_or_default();
        out.sort();
        out
    }

    /// Nombres de todo lo que hay en `dir`, ordenados.
    fn content(program_dir: &Path) -> Vec<String> {
        let mut out: Vec<String> = std::fs::read_dir(program_dir)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|e| e.file_name().to_string_lossy().to_string())
                    .collect()
            })
            .unwrap_or_default();
        out.sort();
        out
    }

    /// Nombres de primer nivel que debe tener un directorio de programa con el
    /// bundle del target instalado: las entradas planas del manifiesto más el
    /// directorio `vendor` del motor.
    fn expected_content(triple: &str) -> Vec<String> {
        let mut out: Vec<String> = manifest::target_section(triple)
            .unwrap()
            .required
            .iter()
            .filter(|r| !r.contains('/'))
            .cloned()
            .collect();
        out.push("vendor".to_string());
        out.sort();
        out
    }

    /// Rutas de relativo origen que quedan con un fichero, ordenadas. La
    /// colocación mueve ficheros, no directorios: los directorios vacíos que deja
    /// en el staging no importan, y el barrido se lleva el staging entero.
    fn files_in_source(source: &Path) -> Vec<String> {
        let mut out = Vec::new();
        let mut stack = vec![source.to_path_buf()];
        while let Some(dir) = stack.pop() {
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    if entry.path().is_dir() {
                        stack.push(entry.path());
                    } else {
                        out.push(entry.file_name().to_string_lossy().to_string());
                    }
                }
            }
        }
        out.sort();
        out
    }

    /// Un reemplazo confirmado deja el bundle nuevo completo, sin aparcado y sin
    /// diario, y el origen se queda vacío porque la colocación es un renombrado.
    #[test]
    fn committed_replacement_leaves_no_residue() {
        let dir = scratch("txn-ok");
        let triple = target::host_triple();
        let executable = manifest::target_section(triple).unwrap().executable;
        let program = dir.join("programa");
        let source = dir.join("staging");
        std::fs::create_dir_all(&program).unwrap();
        bundle(&source, "v2");
        write_file(&program.join(&executable), "v1");
        write_file(&program.join("LICENSE"), "licencia vieja");

        let outcome = replace(&program, &source).unwrap();
        assert!(outcome.leftovers.is_empty(), "el SO no tiene nada en uso");
        assert!(outcome.placed.contains(&executable));
        assert!(parked(&program).is_empty(), "sin aparcados");
        assert_eq!(
            std::fs::read_to_string(program.join(&executable)).unwrap(),
            "v2",
            "el bundle nuevo está colocado"
        );
        assert_eq!(
            std::fs::read_to_string(program.join("LICENSE")).unwrap(),
            "v2",
            "también los documentos, no solo el ejecutable"
        );
        assert!(program
            .join("vendor/qwen3-tts")
            .join(format!(
                "qwen_tts{}",
                if executable.ends_with(".exe") {
                    ".exe"
                } else {
                    ""
                }
            ))
            .is_file());
        assert_eq!(content(&program), expected_content(triple));
        assert!(read_journal(&program).unwrap().is_none(), "sin diario");
        assert!(
            files_in_source(&source).is_empty(),
            "el origen se movió, no se copió"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Una interrupción en cualquier punto del algoritmo deja la versión anterior
    /// operativa y utilizable, y la siguiente operación completa la recuperación
    /// dejando cero residuo.
    ///
    /// Los cuatro puntos son los del paso 6 del flujo. Los tres primeros fallan
    /// antes de que el bundle nuevo toque el disco de programa; el cuarto, con el
    /// bundle ya colocado, se recupera por rollback porque la transacción nunca
    /// llegó a confirmarse.
    #[test]
    #[cfg(feature = "faults")]
    fn interrupted_transaction_leaves_previous_version_usable() {
        use avi_core::exit_codes::ExitCode;

        let triple = target::host_triple();
        let executable = manifest::target_section(triple).unwrap().executable;

        for point in [
            FaultPoint::BeforePark,
            FaultPoint::BeforePlace,
            FaultPoint::BeforeFixPermissions,
            FaultPoint::BeforeCommit,
        ] {
            let dir = scratch(&format!("txn-interrumpida-{}", point.as_str()));
            let program = dir.join("programa");
            let source = dir.join("staging");
            std::fs::create_dir_all(&program).unwrap();
            bundle(&source, "v2");
            // Versión anterior completa: ejecutable, motor, librería y licencia.
            bundle(&program, "v1");

            let window = faults::armed(point);
            let err = replace(&program, &source).unwrap_err();
            drop(window);

            let failure = err
                .downcast_ref::<LifecycleError>()
                .unwrap_or_else(|| panic!("{}: el fallo no es de contrato: {err}", point.as_str()));
            assert_eq!(
                failure.reason,
                "rolled_back",
                "{}: la versión anterior se restaura",
                point.as_str()
            );
            assert_eq!(
                ExitCode::from_reason(failure.reason).code(),
                13,
                "RolledBack = 13"
            );

            // La versión anterior sigue siendo la operativa: el ejecutable se
            // puede leer y ejecutar como antes de la interrupción.
            let previous = std::fs::read_to_string(program.join(&executable)).unwrap();
            assert_eq!(
                previous,
                "v1",
                "{}: la versión anterior sigue operativa",
                point.as_str()
            );
            assert!(
                program
                    .join("vendor/qwen3-tts")
                    .join(format!(
                        "qwen_tts{}",
                        if executable.ends_with(".exe") {
                            ".exe"
                        } else {
                            ""
                        }
                    ))
                    .is_file(),
                "{}: el motor anterior sigue en su sitio",
                point.as_str()
            );

            // Cero residuo: ni diario ni aparcado.
            assert!(
                read_journal(&program).unwrap().is_none(),
                "{}: el diario se borra al revertir",
                point.as_str()
            );
            assert!(
                parked(&program).is_empty(),
                "{}: no queda el aparcado, y el origen del bundle está intacto",
                point.as_str()
            );
            let installed = manifest::validate_bundle(triple, &program);
            assert!(
                installed.is_ok(),
                "{}: el programa instalado sigue siendo un bundle válido",
                point.as_str()
            );

            // La siguiente operación completa el reemplazo: mismo bundle, mismo
            // resultado final, sin residuos de la anterior. La ventana ya se
            // cerró al terminar la aserción del fallo inyectado, así que este
            // reintento corre sin ningún punto armado.
            let outcome = replace(&program, &source).unwrap();
            assert!(outcome.leftovers.is_empty());
            assert_eq!(
                std::fs::read_to_string(program.join(&executable)).unwrap(),
                "v2",
                "{}: el reintento instala la versión nueva",
                point.as_str()
            );
            assert_eq!(
                content(&program),
                expected_content(triple),
                "{}: solo queda el bundle",
                point.as_str()
            );
            assert!(
                files_in_source(&source).is_empty(),
                "{}: el reintento coloca el bundle entero",
                point.as_str()
            );
            assert!(read_journal(&program).unwrap().is_none());
            assert!(parked(&program).is_empty());
            assert!(source.is_dir(), "el staging sigue existiendo");

            std::fs::remove_dir_all(&dir).ok();
        }
    }
}
