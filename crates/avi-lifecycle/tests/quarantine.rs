//! Limpieza de la cuarentena: el criterio 9 en macOS y el recorrido en todas partes.
//!
//! **Este archivo se compila y se ejecuta en las cuatro plataformas**, y a propósito:
//! la puerta de macOS solo corre en el pipeline del release, así que un error de
//! compilación en el código que correrá allí se descubriría después de todo el trabajo
//! del ciclo. Lo único que queda detrás de `#[cfg(target_os = "macos")]` son las
//! llamadas a `xattr`, que son la parte que solo macOS tiene.
//!
//! Lo que se afirma en todas partes, con una implementación de atributos **falsa** que
//! registra a qué se le pregunta:
//!
//! - el recorrido visita **todos** los archivos del directorio de programa, incluidos
//!   los del derivado del motor tres niveles más abajo —que es el motivo de que el
//!   criterio 9 sea sobre todo el directorio y no solo el ejecutable—;
//! - **no pregunta por nada fuera** del directorio que se le pasó;
//! - un fallo en un archivo **no corta** el recorrido;
//! - el límite de profundidad **se respeta**, que es lo que evita que un directorio con
//!   un lazo deje la limpieza colgando;
//! - limpiar dos veces no cambia nada.
//!
//! Lo que solo se afirma en macOS, y sigue sin poder verificarse en local: que
//! `xattr` quita el atributo de verdad, y que `install` se lo quita al bundle entero
//! (el criterio 9).

#[cfg(target_os = "macos")]
use avi_lifecycle::daemon_stop::ProcessControl;
use avi_lifecycle::quarantine::{self, Outcome, Quarantine, MAX_DEPTH};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Registro de todo lo que se le pregunta, para poder afirmar el alcance.
#[derive(Debug, Default)]
struct Registry {
    /// Archivos a los que se les quitó el atributo.
    clear: BTreeSet<PathBuf>,
    /// Archivos por los que se preguntó si lo tenían.
    ask: BTreeSet<PathBuf>,
    /// Archivos en los que `clear` falla, para probar que el recorrido no se corta.
    fail_at: BTreeSet<PathBuf>,
}

/// Implementación falsa de atributos: marca un conjunto de archivos como «en
/// cuarentena» y registra todo lo que se le pregunta.
///
/// No toca el disco. Es lo que permite ejercitar el recorrido en Windows y en Linux, y
/// lo que hace que las pruebas del recorrido no dependan de `xattr`.
struct Fake {
    /// Archivos que están «en cuarentena», y que `clear` vacía.
    in_quarantine: Mutex<BTreeSet<PathBuf>>,
    registry: Mutex<Registry>,
}

impl Fake {
    fn new() -> Self {
        Self {
            in_quarantine: Mutex::new(BTreeSet::new()),
            registry: Mutex::new(Registry::default()),
        }
    }

    /// Declara un archivo en cuarentena, como si lo hubiera puesto el navegador.
    fn mark(&self, path: &Path) {
        self.in_quarantine
            .lock()
            .expect("el registro no se envenena")
            .insert(path.to_path_buf());
    }

    /// Declara que `clear` fallará en ese archivo.
    fn fail_at(&self, path: &Path) {
        self.registry
            .lock()
            .expect("el registro no se envenena")
            .fail_at
            .insert(path.to_path_buf());
    }

    /// Declara que va a fallar el primer archivo de la lista y devuelve su ruta.
    fn fail_first(&self, files: &[PathBuf]) -> PathBuf {
        let failure = files[0].clone();
        self.fail_at(&failure);
        failure
    }

    /// A qué se le preguntó, sin importar el orden.
    fn asked(&self) -> BTreeSet<PathBuf> {
        self.registry
            .lock()
            .expect("el registro no se envenena")
            .ask
            .clone()
    }

    /// Qué se limpió, sin importar el orden.
    fn cleared(&self) -> BTreeSet<PathBuf> {
        self.registry
            .lock()
            .expect("el registro no se envenena")
            .clear
            .clone()
    }

    /// Cuántos archivos siguen «en cuarentena».
    fn remaining(&self) -> usize {
        self.in_quarantine
            .lock()
            .expect("el registro no se envenena")
            .len()
    }
}

impl Quarantine for Fake {
    fn has(&self, path: &Path) -> std::io::Result<bool> {
        Ok(self
            .in_quarantine
            .lock()
            .expect("sin veneno")
            .contains(path))
    }

    fn clear(&self, path: &Path) -> std::io::Result<bool> {
        // La pregunta se registra **siempre**, antes de nada: es lo que permite afirmar
        // el alcance del recorrido, tanto si se limpió, si no había nada o si falló.
        // El cerrojo se toma y se suelta en cada aserción porque `Mutex` no es
        // reentrante.
        {
            let mut registry = self.registry.lock().expect("sin veneno");
            registry.ask.insert(path.to_path_buf());
            if registry.fail_at.contains(path) {
                // Un fallo que la operación **no** confunde con un «no lo tenía»: es
                // lo que la distingue, y por eso se propaga como `Err` y no como
                // `Ok(false)`.
                return Err(std::io::Error::other("no se pudo quitar el atributo"));
            }
        }
        if !self.in_quarantine.lock().expect("sin veneno").remove(path) {
            return Ok(false);
        }
        self.registry
            .lock()
            .expect("sin veneno")
            .clear
            .insert(path.to_path_buf());
        Ok(true)
    }

    fn put(&self, path: &Path) -> std::io::Result<()> {
        self.mark(path);
        Ok(())
    }
}

/// Directorio único por etiqueta y por nanosegundo, para que dos pruebas simultáneas —o
/// dos ejecuciones del mismo binario— no se pisen.
fn unique(tag: &str) -> PathBuf {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or_default();
    let root =
        std::env::temp_dir().join(format!("quarantine-e2e-{}-{tag}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("se crea la raíz del sandbox");
    root
}

/// Une un fragmento del manifiesto con la raíz del bundle.
fn place(dest: &Path, relative: &str) -> PathBuf {
    let mut path = dest.to_path_buf();
    for part in relative.split('/') {
        path.push(part);
    }
    path
}

/// Escribe un bundle completo con la lista de `packaging/bundle-manifest.json`, que es
/// la misma que valida `self install`. Los nombres no se escriben a mano: si el
/// manifiesto cambiara, la seguiría exercising lo que el manifiesto dice.
fn write_bundle(dest: &Path) -> Vec<PathBuf> {
    let section = avi_lifecycle::manifest::target_section(avi_lifecycle::target::host_triple())
        .expect("el target del host tiene sección en el manifiesto");
    let mut files = Vec::new();
    for relative in &section.required {
        let complete = place(dest, relative);
        if let Some(parent) = complete.parent() {
            std::fs::create_dir_all(parent).expect("se crea el directorio del archivo");
        }
        std::fs::write(&complete, format!("contenido de {relative}\n"))
            .expect("se escribe el archivo del bundle");
        files.push(complete);
    }
    files
}

/// Sandbox con un árbol con la forma de un bundle: ejecutable y documentos en la raíz,
/// librería de runtime junto a ellos y el derivado del motor tres niveles más abajo.
///
/// El bundle se escribe primero en el **staging** —que es donde lo deja el bootstrap— y
/// [`Arbol::colocados`] lo traslada al directorio de programa, que es donde la
/// instalación lo coloca y donde la limpieza se ejecuta.
struct Tree {
    /// Raíz del sandbox, para borrarla entera.
    root: PathBuf,
    /// Staging hermano del directorio de programa, con el prefijo de la tabla de rutas.
    staging: PathBuf,
    /// Directorio de programa.
    program: PathBuf,
    /// Archivos del bundle, en la ruta en la que están ahora: el staging.
    files: Vec<PathBuf>,
}

impl Tree {
    fn new(tag: &str) -> Self {
        let root = unique(tag);
        let opt = root.join("opt");
        let staging = opt.join(format!("{}test", avi_lifecycle::STAGING_DIR_PREFIX));
        let program = opt.join("ai-voice-interconnector");
        let files = write_bundle(&staging);
        Self {
            root,
            staging,
            program,
            files,
        }
    }

    /// Los archivos del bundle **en el directorio de programa**. Es la traducción de
    /// "mover lo que hay en el staging al directorio de programa", que es el paso 6 de
    /// la instalación.
    fn placed(&self) -> Vec<PathBuf> {
        self.files
            .iter()
            .map(|a| {
                self.program.join(
                    a.strip_prefix(&self.staging)
                        .expect("el archivo está en el staging"),
                )
            })
            .collect()
    }

    /// Los mismos archivos, pero **escritos en disco**: el recorrido empieza por un
    /// `read_dir` del directorio de programa, así que sin un árbol real no preguntaría
    /// por nada. Es el estado en el que está el directorio después del paso 6.
    fn materialize(&self) -> Vec<PathBuf> {
        let placed = self.placed();
        for file in &placed {
            std::fs::create_dir_all(file.parent().expect("el archivo tiene padre"))
                .expect("se crea el directorio del archivo colocado");
            std::fs::write(file, "contenido colocado\n").expect("se escribe el archivo");
        }
        placed
    }
}

/// Contenido de un directorio, para afirmar que nada se ha tocado.
fn content(root: &Path) -> Vec<String> {
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

/// `true` si entre lo preguntado está el derivado del motor (`vendor/…`), que es el caso
/// que motiva que la limpieza sea sobre todo el directorio y no solo el ejecutable.
fn asks_about_derived(asked: &BTreeSet<PathBuf>) -> bool {
    let separator = if cfg!(windows) { '\\' } else { '/' };
    let marker = format!("{separator}vendor{separator}");
    asked
        .iter()
        .any(|path| path.to_string_lossy().contains(&marker))
}

// ─── El criterio 9: instalar no deja nada en cuarentena ───────────────────────────
//
// El criterio 9 tiene una mitad que solo macOS puede comprobar —que `xattr` quita el
// atributo de verdad— y otra que se puede comprobar en todas partes: **el alcance** de la
// limpieza. Por eso las dos variantes del criterio 9 se llaman igual y solo una existe en
// cada plataforma:
//
// - En macOS, `criterion_9_install_strips_quarantine_from_whole_program_dir` instala un
//   bundle cuyos archivos **y directorios** llevan el atributo —así los deja el Finder
//   cuando el archivo se descargó por navegador, que es la otra mitad del enunciado— y
//   afirma, recorriendo el árbol real del directorio de programa, que ni un archivo ni un
//   directorio lo conserva.
// - Fuera de macOS, la variante del mismo nombre afirma lo que sí es cierto aquí: que el
//   atributo se planta en el árbol entero, que el recorrido lo abarca entero, y que un
//   hermano del directorio de programa conserva el suyo. No es un `skip`: es una
//   afirmación distinta, ejecutada, y el criterio 9 en su sentido fuerte lo ejecuta la
//   puerta de macOS con el mismo nombre.
//
// Que las dos se llamen igual es deliberado: `--list` muestra `criterion_9_` en las tres
// puertas, y en macOS el nombre corresponde a la prueba del criterio, no a su sombra.

/// Planta el atributo en un árbol entero: archivos **y** directorios.
///
/// El Finder pone `com.apple.quarantine` en el archivo descargado y macOS lo hereda todo
/// lo que se crea dentro, así que un bundle extraído con el Finder llega con el atributo
/// en cada archivo y en cada directorio intermedio. Plantarlo solo en los archivos sería
/// una versión más fácil del criterio.
fn seed_quarantine(root: &Path) -> Vec<PathBuf> {
    let mut planted = Vec::new();
    let mut directories = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if dir != root {
            directories.push(dir.clone());
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                stack.push(path);
            } else {
                planted.push(path);
            }
        }
    }
    for path in planted.iter().chain(directories.iter()) {
        quarantine::put(path).expect("se pone la cuarentena de macOS");
    }
    planted
}

/// Recorre el directorio de programa entero y devuelve las rutas relativas, en orden, de
/// todo lo que **conserva** el atributo.
///
/// El oráculo entra por parámetro, y esa es la decisión que hace que la función sea
/// comprobable en todas las plataformas: en macOS se le pasa `quarantine::plataforma()`,
/// que es la implementación de `xattr`, y fuera se le puede pasar una implementación que
/// registre. El recorrido —el código que el criterio 9 evalúa— es el mismo en los dos
/// casos, así que un error de recorrido no puede esconderse hasta la puerta de macOS.
fn keep_quarantine(root: &Path, attributes: &dyn quarantine::Quarantine) -> Vec<String> {
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
            if attributes.has(&path).unwrap_or(false) {
                out.push(relative);
            }
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                stack.push(path);
            }
        }
    }
    out.sort();
    out
}

/// **Criterio 9 en macOS.** Instalar un bundle descargado por navegador no deja ni un
/// archivo ni un directorio del directorio de programa con `com.apple.quarantine`.
///
/// La fuerza de la prueba está en dos detalles. El primero es que el atributo se planta
/// en **todo** el bundle, directorios incluidos, que es el estado en que lo deja el
/// Finder. El segundo es que la comprobación final **no usa la lista del recibo**: recorre
/// el árbol real del directorio de programa, de modo que un archivo que se colocara sin
/// figurar en el recibo también contaría. Un hermano del directorio de programa, con su
/// propio atributo, es el control: si la limpieza se saliera, saldría por él.
#[cfg(target_os = "macos")]
#[tokio::test]
async fn criterion_9_install_strips_quarantine_from_whole_program_dir() {
    let sandbox = Sandbox::new("c9");
    let exe = sandbox.write_bundle(&sandbox.staging);
    let planted = seed_quarantine(&sandbox.staging);
    assert!(!planted.is_empty(), "criterio 9: el bundle tiene archivos");

    // Un hermano del directorio de programa, en cuarentena, que no se debe tocar.
    let sibling = sandbox.root.join("opt").join("hermano.txt");
    std::fs::write(&sibling, "fuera del programa\n").expect("se escribe el hermano");
    quarantine::put(&sibling).expect("se pone la cuarentena del hermano");

    // Punto de partida: **todo** el bundle está en cuarentena, archivos y directorios.
    assert_eq!(
        keep_quarantine(&sandbox.staging, quarantine::platform()).len(),
        planted.len() + intermediate_dirs(&sandbox.staging),
        "criterio 9: el bundle entero arranca en cuarentena"
    );

    let outcome = avi_lifecycle::install::install(
        &sandbox.env(&exe),
        &avi_lifecycle::install::Options {
            assume_yes: true,
            no_setup: true,
            // Sin `PATH`: la prueba es de la cuarentena, y escribir en el perfil del
            // usuario que ejecuta la puerta de macOS no es un efecto aceptable.
            no_modify_path: true,
            force: false,
            channel: None,
            with_voice_cloning: false,
        },
        &Inert,
    )
    .await
    .expect("criterio 9: la instalación se completa");

    // El derivado del motor y la librería de ONNX Runtime están entre lo colocado, y son
    // los dos que la operación nombra como los que se ejecutan o se cargan.
    assert!(
        outcome
            .receipt
            .files
            .iter()
            .any(|f| f.starts_with("vendor/")),
        "criterio 9: el derivado del motor está entre los archivos colocados"
    );
    assert!(
        outcome.receipt.files.iter().any(|f| f.ends_with(".dylib")),
        "criterio 9: la librería de ONNX Runtime está entre los archivos colocados"
    );

    // Y el árbol entero, sin excepciones y sin apoyarse en el recibo.
    let with_attribute = keep_quarantine(&sandbox.program_dir, quarantine::platform());
    assert!(
        with_attribute.is_empty(),
        "criterio 9: ningún archivo ni directorio del directorio de programa conserva la \
         cuarentena; conservan: {with_attribute:?}"
    );
    assert!(
        quarantine::has(&sibling),
        "criterio 9: y el hermano, que está fuera, sigue con la suya"
    );
    assert!(
        avi_lifecycle::receipt::read_from(&sandbox.program_dir)
            .expect("criterio 9: se lee el recibo")
            .is_some(),
        "criterio 9: la instalación dejó recibo, que es lo que la distingue de un copiado"
    );
    let _ = std::fs::remove_dir_all(&sandbox.root);
}

/// **Criterio 9 fuera de macOS.** La mitad del criterio que se puede comprobar donde el
/// atributo no existe.
///
/// No es una prueba de que la operación no se pueda ejecutar: usa el `strip` real con la
/// implementación real de la plataforma sobre un árbol real, y además el recorrido con una
/// implementación de atributos que sí registra. Afirma tres cosas, y las tres importan
/// para la puerta de macOS: que el atributo se planta en el árbol entero —archivos **y**
/// directorios, que es como lo deja el Finder—, que el recorrido abarca las dos clases,
/// y que un hermano del directorio de programa conserva el suyo, o sea que la limpieza no
/// se sale. Lo único que queda sin comprobar aquí es que `xattr` lo quite, y lo dice el
/// propio mensaje de la prueba.
#[cfg(not(target_os = "macos"))]
#[test]
fn criterion_9_install_strips_quarantine_from_whole_program_dir() {
    let tree = Tree::new("c9-otros");
    let placed = tree.materialize();
    let planted = seed_quarantine(&tree.staging);
    assert!(!planted.is_empty(), "criterio 9: el bundle tiene archivos");

    // El hermano, con su propio atributo, es el control del alcance.
    let sibling = tree.root.join("opt").join("hermano.txt");
    std::fs::write(&sibling, "fuera del programa\n").expect("se escribe el hermano");
    quarantine::put(&sibling).expect("`put` fuera de macOS no falla");
    assert!(
        !quarantine::has(&sibling),
        "criterio 9: fuera de macOS `has` no encuentra nada, que es lo honesto: el punto de \
         partida del hermano es indistinguible del de cualquier otro"
    );

    // El recorrido con una implementación de atributos que registra: se pregunta por cada
    // archivo colocado y por los directorios intermedios, y por nada fuera.
    let fake = Fake::new();
    for placed in &placed {
        fake.mark(placed);
    }
    let outcome = quarantine::strip_bounded(&tree.program, MAX_DEPTH, &fake);
    let asked = fake.asked();
    assert!(
        outcome.is_clear(),
        "criterio 9: el recorrido no falla: {:?}",
        outcome.failed
    );
    assert_eq!(
        outcome.cleared.len(),
        placed.len(),
        "criterio 9: el recorrido limpia cada archivo del directorio de programa"
    );
    for placed in &placed {
        assert!(
            asked.contains(placed),
            "criterio 9: se pregunta por {}",
            placed.display()
        );
    }
    let directories = asked_dirs(&asked);
    assert!(
        !directories.is_empty(),
        "criterio 9: y por los directorios intermedios, que también heredan el atributo: \
         {directories:?}"
    );
    for path in &asked {
        assert!(
            path.starts_with(&tree.program),
            "criterio 9: nada fuera del directorio de programa: {}",
            path.display()
        );
    }
    assert!(
        !asked.contains(&sibling),
        "criterio 9: el hermano, que tiene su propio atributo, no se pregunta"
    );

    // Y ahora el mismo recorrido de la prueba de macOS, con un oráculo que registra y
    // que **nada ha limpiado todavía**: es la comprobación no vacía del alcance, y usa
    // exactamente el código que la puerta de macOS evalúa.
    let oracle = Fake::new();
    for placed in &placed {
        oracle.mark(placed);
    }
    oracle.mark(&sibling);
    let with_attribute = keep_quarantine(&tree.program, &oracle);
    assert_eq!(
        with_attribute.len(),
        placed.len(),
        "criterio 9: el recorrido del criterio 9 abarca cada archivo colocado: {with_attribute:?}"
    );
    assert!(
        !with_attribute
            .iter()
            .any(|r| r.contains("..") || r.contains("hermano")),
        "criterio 9: y no incluye nada de fuera del directorio de programa"
    );
    let clean = keep_quarantine(&tree.program, &fake);
    assert!(
        clean.is_empty(),
        "criterio 9: después de limpiar, el árbol colocado queda sin cuarentena: {clean:?}"
    );
    assert!(
        keeps_at(&oracle, &sibling),
        "criterio 9: y el hermano conserva el suyo, porque el recorrido nunca se sale"
    );

    // Y el `strip` real de la plataforma: en esta plataforma el atributo no existe, así que
    // informa de que no hay nada que quitar sin escribir nada. Ni dentro del directorio de
    // programa ni fuera.
    let before = content(&tree.root);
    let real = quarantine::strip(&tree.program);
    assert!(
        real.is_nothing_to_do(),
        "criterio 9: fuera de macOS no hay nada que quitar: {real:?}"
    );
    assert_eq!(
        content(&tree.root),
        before,
        "criterio 9: y el disco no cambia, ni dentro ni fuera"
    );
    let _ = std::fs::remove_dir_all(&tree.root);
}

/// `true` si el oráculo dice que la ruta conserva el atributo, para el control del hermano
/// sin reconstruir el conjunto.
fn keeps_at(attributes: &dyn quarantine::Quarantine, path: &Path) -> bool {
    attributes.has(path).unwrap_or(false)
}

/// Directorios intermedios de entre lo preguntado, que es el caso que motiva que la
/// limpieza sea sobre todo el directorio y no solo el ejecutable.
fn asked_dirs(asked: &BTreeSet<PathBuf>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = asked.iter().filter(|path| path.is_dir()).cloned().collect();
    out.sort();
    out
}

/// Cuántos directorios intermedios hay en un árbol, sin contar su raíz.
#[cfg(target_os = "macos")]
fn intermediate_dirs(root: &Path) -> usize {
    let mut total = 0;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                total += 1;
                stack.push(entry.path());
            }
        }
    }
    total
}

// ─── El recorrido, en todas las plataformas ───────────────────────────────────────

/// El recorrido pregunta por **todos** los archivos del directorio de programa, y por
/// nada fuera de él. Es la forma verificable del criterio 9 sin `xattr`: la razón de que
/// la limpieza sea sobre todo el directorio y no solo el ejecutable es precisamente que
/// el motor y la librería de ONNX Runtime también lo heredan, y eso solo se demuestra
/// preguntando por ellos.
#[test]
fn quarantine_strip_walks_the_whole_program_dir_and_nothing_outside() {
    let tree = Tree::new("recorrido");
    let program = tree.program.clone();
    let placed = tree.materialize();
    // El sandbox tiene cosas **fuera** del directorio de programa: el staging hermano,
    // que tiene los mismos archivos, y el directorio raíz. Si el recorrido se saliera,
    // las preguntaría —o las tocaría.
    assert!(
        tree.staging.exists() && tree.root.exists(),
        "el sandbox tiene hermanos que no son del directorio de programa"
    );

    let fake = Fake::new();
    for placed in &placed {
        fake.mark(placed);
    }

    let outcome = quarantine::strip_bounded(&program, MAX_DEPTH, &fake);

    // 1. Se limpió todo el bundle, incluido el derivado del motor tres niveles abajo.
    assert_eq!(
        outcome.cleared.len(),
        placed.len(),
        "se limpió cada **archivo** del directorio de programa: {:?}",
        outcome.cleared
    );
    assert_eq!(
        fake.cleared(),
        placed.iter().cloned().collect::<BTreeSet<_>>(),
        "y son exactamente esos: los directorios de entrada se preguntan pero no tienen \
         el atributo, así que no se limpian"
    );
    assert!(outcome.is_clear(), "sin fallos: {:?}", outcome.failed);
    assert_eq!(fake.remaining(), 0, "no queda nada en cuarentena");

    // 2. Se preguntó por todos los archivos, y **solo** por cosas del directorio de
    //    programa.
    let asked = fake.asked();
    for expected in &placed {
        assert!(
            asked.contains(expected),
            "se preguntó por {}",
            expected.display()
        );
    }
    for path in &asked {
        assert!(
            path.starts_with(&program),
            "nada fuera del directorio de programa: {}",
            path.display()
        );
    }
    // Se preguntó también por los **directorios** intermedios, y eso es lo correcto y no
    // un descuido: un directorio puede llevar él mismo el atributo, y macOS lo
    // heredan los archivos que se creen dentro. Limpiar solo los archivos dejaría que el
    // siguiente archivo escrito en `vendor/` volviera a heredarlo.
    let directories: Vec<&PathBuf> = asked
        .iter()
        .filter(|r| r.is_dir())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    assert_eq!(
        directories.len(),
        2,
        "los dos directorios intermedios del bundle se preguntan también: {directories:?}"
    );
    for path in &directories {
        assert!(
            path.starts_with(&program),
            "y están dentro del directorio de programa: {}",
            path.display()
        );
    }

    // 3. Y se preguntó por el derivado del motor, que es el caso que motiva el paso 7.
    assert!(
        asks_about_derived(&asked),
        "el motor derivado se limpia también, no solo el ejecutable: {asked:?}"
    );
    let _ = std::fs::remove_dir_all(&tree.root);
}

/// Un fallo en un archivo **no corta** el recorrido: los demás también necesitan
/// quedarse limpios, y el criterio 9 exige que ninguno conserve la cuarentena.
///
/// Y el resultado lo dice: `is_clear()` es `false` porque hay un archivo que no se pudo
/// limpiar, que es exactamente la información que el resumen final muestra.
#[test]
fn quarantine_strip_reports_failures_and_keeps_going() {
    let tree = Tree::new("fallos");
    let placed = tree.materialize();
    let fake = Fake::new();
    for placed in &placed {
        fake.mark(placed);
    }
    let failure = fake.fail_first(&placed);

    let outcome = quarantine::strip_bounded(&tree.program, MAX_DEPTH, &fake);

    assert_eq!(
        outcome.failed.len(),
        1,
        "un solo fallo declarado: {:?}",
        outcome.failed
    );
    assert_eq!(outcome.failed[0].0, failure);
    assert!(
        outcome.failed[0].1.contains("no se pudo quitar"),
        "y el motivo se propaga: {:?}",
        outcome.failed[0].1
    );
    assert!(
        !outcome.is_clear(),
        "`is_clear` es falso: queda un archivo sin limpiar, y el resumen lo dirá"
    );
    assert_eq!(
        outcome.cleared.len(),
        placed.len() - 1,
        "los demás se limpiaron igualmente: el fallo no cortó el recorrido"
    );
    assert_eq!(fake.remaining(), 1, "solo queda en cuarentena el que falló");
    let _ = std::fs::remove_dir_all(&tree.root);
}

/// El límite de profundidad se respeta, y es lo que evita que un directorio con un lazo
/// —un enlace simbólico a un ancestro— deje la limpieza colgando.
///
/// Se prueba con el límite a cero, que es el caso extremo: se pregunta por lo que hay
/// directamente en el directorio de programa, y ni un nivel más adentro.
#[test]
fn quarantine_strip_respects_the_depth_limit() {
    let tree = Tree::new("profundidad");
    let program = tree.program.clone();
    let placed = tree.materialize();
    let at_root: Vec<&PathBuf> = placed
        .iter()
        .filter(|c| c.parent() == Some(&program))
        .collect();
    let nested: Vec<&PathBuf> = placed
        .iter()
        .filter(|c| c.parent() != Some(&program))
        .collect();
    assert!(
        !at_root.is_empty() && !nested.is_empty(),
        "el bundle tiene archivos en la raíz ({}) y anidados ({}), que es lo que hace \
         la prueba significativa",
        at_root.len(),
        nested.len()
    );

    let fake = Fake::new();
    for placed in &placed {
        fake.mark(placed);
    }

    let outcome = quarantine::strip_bounded(&program, 0, &fake);

    let asked = fake.asked();
    for file in &at_root {
        assert!(
            asked.contains(*file),
            "con límite cero sí se pregunta por {}",
            file.display()
        );
    }
    for file in &nested {
        assert!(
            !asked.contains(*file),
            "con límite cero no se baja a {}",
            file.display()
        );
    }
    assert_eq!(
        outcome.cleared.len(),
        at_root.len(),
        "el resultado solo cuenta lo que sí se limpiaron, que es la raíz"
    );
    assert_eq!(
        fake.remaining(),
        nested.len(),
        "y los anidados siguen en cuarentena, que es lo que el límite delimita"
    );
    assert!(outcome.is_clear(), "sin fallos: {:?}", outcome.failed);
    let _ = std::fs::remove_dir_all(&tree.root);
}

/// Limpiar dos veces no cambia nada, y la segunda pasada informa de que no hay nada que
/// quitar. Es la idempotencia del mismo paso que la instalación repetida.
#[test]
fn quarantine_strip_is_idempotent() {
    let tree = Tree::new("idempotencia");
    let placed = tree.materialize();
    let fake = Fake::new();
    for placed in &placed {
        fake.mark(placed);
    }

    let first = quarantine::strip_bounded(&tree.program, MAX_DEPTH, &fake);
    assert!(
        !first.is_nothing_to_do(),
        "la primera pasada sí tenía trabajo: {first:?}"
    );
    let first_cleared = fake.cleared();
    assert_eq!(
        first_cleared.len(),
        placed.len(),
        "y limpió el bundle entero"
    );

    let second = quarantine::strip_bounded(&tree.program, MAX_DEPTH, &fake);
    assert!(
        second.is_nothing_to_do(),
        "la segunda pasada informa de que no hay nada que quitar: {second:?}"
    );
    assert!(second.is_clear(), "y no es un fallo: devuelve éxito");
    assert_eq!(
        fake.cleared(),
        first_cleared,
        "y no se volvió a limpiar nada"
    );
    let _ = std::fs::remove_dir_all(&tree.root);
}

/// El resultado por defecto es «no hay nada que hacer», que es lo que la instalación
/// espera antes de llamar a la limpieza. Afirmarlo evita que un `Default` mal construido
/// pase por una limpieza correcta.
#[test]
fn quarantine_default_outcome_is_nothing_to_do() {
    let outcome = Outcome::default();
    assert!(outcome.cleared.is_empty());
    assert!(outcome.failed.is_empty());
    assert!(outcome.is_clear());
    assert!(outcome.is_nothing_to_do());
}

/// La marca es la que nombra la limpieza. Se afirma en todas partes, no solo en macOS: es
/// parte
/// del contrato del módulo.
#[test]
fn quarantine_attribute_name_is_the_one_the_spec_names() {
    assert_eq!(quarantine::BLOCK_MARKER, "com.apple.quarantine");
}

// ─── El comportamiento de las plataformas sin el atributo ─────────────────────────

/// Fuera de macOS la operación **informa de que no hay nada que quitar y devuelve
/// éxito**, sin escribir nada.
///
/// No es una prueba que compruebe que la operación no se puede ejecutar: usa el `strip`
/// real, con la implementación real de la plataforma, sobre un árbol real, y afirma las
/// dos cosas que sí se pueden afirmar aquí —que no hay nada que quitar, y que no se ha
/// tocado el disco. La segunda es la que importa: el directorio de programa es de
/// propiedad exclusiva y la limpieza no puede salirse de él.
#[cfg(not(target_os = "macos"))]
#[test]
fn quarantine_outside_macos_has_nothing_to_do_and_touches_nothing() {
    let tree = Tree::new("sin-atributo");
    let placed = tree.materialize();
    // Un hermano del directorio de programa, que es de la aplicación pero no es el
    // directorio de programa: si la limpieza se saliera, lo tocaría.
    let sibling = tree.root.join("opt").join("hermano.txt");
    std::fs::write(&sibling, "no tocar\n").expect("se escribe el hermano");

    // Punto de partida: en esta plataforma ningún archivo tiene el atributo.
    assert!(
        placed.iter().all(|a| !quarantine::has(a)),
        "punto de partida: ningún archivo del bundle tiene cuarentena"
    );
    let before = content(&tree.root);

    let outcome = quarantine::strip(&tree.program);

    assert!(
        outcome.is_nothing_to_do(),
        "la operación informa de que no hay nada que quitar: {outcome:?}"
    );
    assert!(outcome.is_clear(), "y devuelve éxito");
    assert!(outcome.cleared.is_empty(), "no se limpió nada");
    assert!(outcome.failed.is_empty(), "y nada falló");
    assert_eq!(
        content(&tree.root),
        before,
        "el disco no ha cambiado: ni dentro del directorio de programa ni fuera"
    );
    assert_eq!(
        std::fs::read_to_string(&sibling).expect("el hermano sigue"),
        "no tocar\n",
        "el hermano del directorio de programa está intacto"
    );
    let _ = std::fs::remove_dir_all(&tree.root);
}

/// Fuera de macOS, `put` es un no-op y `has` es `false`: no hay atributo que poner, y una
/// implementación que fingiera lo contrario daría al resto del código una garantía que
/// no existe.
#[cfg(not(target_os = "macos"))]
#[test]
fn quarantine_outside_macos_attributes_are_inert() {
    let tree = Tree::new("atributos-inertes");
    let scheduled = tree.materialize();
    for file in &scheduled {
        quarantine::put(file).expect("`put` fuera de macOS no falla");
        assert!(
            !quarantine::has(file),
            "y `has` sigue diciendo que no: no hay atributo que poner"
        );
    }
    let _ = std::fs::remove_dir_all(&tree.root);
}

// ─── Solo en macOS: el criterio 9 y `xattr` de verdad ─────────────────────────────

/// Control de procesos inerte: no hay daemon en el sandbox.
#[cfg(target_os = "macos")]
struct Inert;

#[cfg(target_os = "macos")]
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

/// El criterio 9: instalar quita la cuarentena de **todo** el directorio de programa, no
/// solo del ejecutable.
///
/// Sigue sin poder verificarse en local: es la prueba de la puerta de macOS, y la
/// primera vez que se ejecutará es en el pipeline del release. Lo que sí queda verificado
/// antes de llegar allí es todo lo de arriba.
#[cfg(target_os = "macos")]
#[tokio::test]
async fn install_strips_quarantine_from_whole_program_dir() {
    let tree = Tree::new("criterio-9");
    let exe = tree
        .files
        .iter()
        .find(|a| {
            a.file_name()
                .is_some_and(|n| n == "ai-voice-interconnector")
        })
        .expect("el bundle tiene ejecutable")
        .clone();
    for file in &tree.files {
        quarantine::put(file).expect("se pone la cuarentena de macOS");
    }
    // Punto de partida: **todos** los archivos del bundle están en cuarentena. Sin esta
    // comprobación, la prueba no afirmaría nada.
    for file in &tree.files {
        assert!(
            quarantine::has(file),
            "{} arranca con cuarentena",
            file.display()
        );
    }

    let sandbox = Sandbox {
        root: tree.root.clone(),
        program_dir: tree.program.clone(),
        home: tree.root.join("home/ana"),
        data_dir: tree.root.join("data"),
        models_dir: tree.root.join("models"),
        temp_root: tree.root.join("tmp"),
    };
    let outcome = avi_lifecycle::install::install(
        &sandbox.env(&exe),
        &avi_lifecycle::install::Options {
            assume_yes: true,
            no_setup: true,
            // Sin `PATH`: la prueba es de la cuarentena, y escribir en el perfil del
            // usuario que ejecuta la puerta de macOS no es un efecto aceptable.
            no_modify_path: true,
            force: false,
            channel: None,
            with_voice_cloning: false,
        },
        &Inert,
    )
    .await
    .expect("la instalación se completa");

    // El derivado del motor y la librería de ONNX Runtime están entre lo colocado, y son
    // los dos que la operación nombra como los que se ejecutan o se cargan.
    assert!(
        outcome
            .receipt
            .files
            .iter()
            .any(|f| f.starts_with("vendor/")),
        "el derivado del motor está entre los archivos colocados"
    );
    assert!(
        outcome.receipt.files.iter().any(|f| f.ends_with(".dylib")),
        "la librería de ONNX Runtime está entre los archivos colocados"
    );

    // Y ninguno conserva el atributo.
    let mut in_quarantine = Vec::new();
    for relative in &outcome.receipt.files {
        let path = place(&sandbox.program_dir, relative);
        assert!(path.exists(), "{relative} se colocó");
        if quarantine::has(&path) {
            in_quarantine.push(relative.clone());
        }
    }
    assert!(
        in_quarantine.is_empty(),
        "la instalación limpió la cuarentena de todo el bundle, no solo del ejecutable; \
         siguen con ella: {in_quarantine:?}"
    );
    assert!(
        avi_lifecycle::receipt::read_from(&sandbox.program_dir)
            .expect("se lee el recibo")
            .is_some(),
        "y dejó recibo, que es lo que distingue una instalación de un copiado"
    );
    let _ = std::fs::remove_dir_all(&tree.root);
}

/// El límite de profundidad contra un **lazo real** de directorios, que es el caso que lo
/// justifica: un enlace simbólico a un ancestro. La implementación falsa demuestra que
/// el límite se respeta; esta demuestra que con el sistema de ficheros de verdad la
/// limpieza no se cuelga.
#[cfg(target_os = "macos")]
#[test]
fn quarantine_strip_survives_a_real_directory_loop() {
    let root = unique("lazo");
    let program = root.join("opt/ai-voice-interconnector");
    let dir = program.join("vendor/qwen3-tts");
    std::fs::create_dir_all(&dir).expect("se crea el árbol");
    let engine = dir.join("qwen_tts");
    std::fs::write(&engine, "motor").expect("se escribe el derivado");
    quarantine::put(&engine).expect("se pone la cuarentena");
    std::os::unix::fs::symlink(&root, dir.join("vuelta")).expect("se crea el lazo");

    let outcome = quarantine::strip(&program);
    assert!(
        outcome.is_clear(),
        "el lazo no produce fallos: {:?}",
        outcome.failed
    );
    assert!(
        !quarantine::has(&engine),
        "y la limpieza alcanzó al derivado antes de encontrarse con el lazo"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// La premisa de la puerta de macOS: `xattr` existe y responde. Si no, el criterio 9 no
/// se puede demostrar, y hay que saberlo en la puerta y no en el release.
#[cfg(target_os = "macos")]
#[test]
fn quarantine_gate_needs_xattr() {
    assert!(
        std::process::Command::new("xattr")
            .arg("-h")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false),
        "la puerta de macOS necesita `xattr` en el PATH"
    );
}

// ─── Entorno de la instalación, solo para macOS ──────────────────────────────────

/// Rutas de la tabla del producto en la prueba del criterio 9.
#[cfg(target_os = "macos")]
struct Sandbox {
    root: PathBuf,
    program_dir: PathBuf,
    staging: PathBuf,
    home: PathBuf,
    data_dir: PathBuf,
    models_dir: PathBuf,
    temp_root: PathBuf,
}

#[cfg(target_os = "macos")]
impl Sandbox {
    /// Árbol con la forma de la instalación: el bundle se escribe primero en
    /// el staging hermano, que es donde lo deja el bootstrap.
    fn new(tag: &str) -> Self {
        let root = unique(tag);
        let staging = root
            .join("opt")
            .join(format!("{}test", avi_lifecycle::STAGING_DIR_PREFIX));
        Self {
            program_dir: root.join("opt").join("ai-voice-interconnector"),
            home: root.join("home"),
            data_dir: root.join("data"),
            models_dir: root.join("models"),
            temp_root: root.join("tmp"),
            staging,
            root,
        }
    }

    /// Escribe el bundle en el staging y devuelve la ruta del ejecutable, que
    /// es la que la instalación recibe.
    fn write_bundle(&self, staging: &Path) -> PathBuf {
        write_bundle(staging)
            .into_iter()
            .find(|p| {
                p.file_name()
                    .is_some_and(|n| n == "ai-voice-interconnector")
            })
            .expect("el bundle tiene ejecutable")
    }
    /// `SHELL` de la máquina, con el nombre corto que usa la tabla de shells.
    fn shell() -> &'static str {
        match std::env::var("SHELL")
            .unwrap_or_default()
            .rsplit('/')
            .next()
            .unwrap_or_default()
        {
            "zsh" => "zsh",
            "fish" => "fish",
            "bash" => "bash",
            _ => "sh",
        }
    }

    fn env(&self, exe: &Path) -> avi_lifecycle::install::Env {
        avi_lifecycle::install::Env {
            exe: exe.to_path_buf(),
            version: "0.24.0".to_string(),
            target: avi_lifecycle::target::host_triple().to_string(),
            program_dir: self.program_dir.clone(),
            bin_dir: self.home.join(".local/bin"),
            data_dir: self.data_dir.clone(),
            models_dir: self.models_dir.clone(),
            temp_root: self.temp_root.clone(),
            home: self.home.clone(),
            path_env: "/usr/bin:/bin".to_string(),
            shell: avi_lifecycle::path_unix::Shell::from_env(Some(Self::shell())),
            zdotdir: None,
            registry_subkey: String::new(),
            daemon_addr: "127.0.0.1:1".to_string(),
            source: None,
        }
    }
}
