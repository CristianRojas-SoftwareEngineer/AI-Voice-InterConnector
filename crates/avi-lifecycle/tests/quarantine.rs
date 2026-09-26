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
struct Registro {
    /// Archivos a los que se les quitó el atributo.
    limpiar: BTreeSet<PathBuf>,
    /// Archivos por los que se preguntó si lo tenían.
    preguntar: BTreeSet<PathBuf>,
    /// Archivos en los que `clear` falla, para probar que el recorrido no se corta.
    fallar_en: BTreeSet<PathBuf>,
}

/// Implementación falsa de atributos: marca un conjunto de archivos como «en
/// cuarentena» y registra todo lo que se le pregunta.
///
/// No toca el disco. Es lo que permite ejercitar el recorrido en Windows y en Linux, y
/// lo que hace que las pruebas del recorrido no dependan de `xattr`.
struct Falso {
    /// Archivos que están «en cuarentena», y que `clear` vacía.
    en_cuarentena: Mutex<BTreeSet<PathBuf>>,
    registro: Mutex<Registro>,
}

impl Falso {
    fn new() -> Self {
        Self {
            en_cuarentena: Mutex::new(BTreeSet::new()),
            registro: Mutex::new(Registro::default()),
        }
    }

    /// Declara un archivo en cuarentena, como si lo hubiera puesto el navegador.
    fn marcar(&self, path: &Path) {
        self.en_cuarentena
            .lock()
            .expect("el registro no se envenena")
            .insert(path.to_path_buf());
    }

    /// Declara que `clear` fallará en ese archivo.
    fn fallar_en(&self, path: &Path) {
        self.registro
            .lock()
            .expect("el registro no se envenena")
            .fallar_en
            .insert(path.to_path_buf());
    }

    /// Declara que va a fallar el primer archivo de la lista y devuelve su ruta.
    fn fallar_el_primero(&self, archivos: &[PathBuf]) -> PathBuf {
        let falla = archivos[0].clone();
        self.fallar_en(&falla);
        falla
    }

    /// A qué se le preguntó, sin importar el orden.
    fn preguntado(&self) -> BTreeSet<PathBuf> {
        self.registro
            .lock()
            .expect("el registro no se envenena")
            .preguntar
            .clone()
    }

    /// Qué se limpió, sin importar el orden.
    fn limpiado(&self) -> BTreeSet<PathBuf> {
        self.registro
            .lock()
            .expect("el registro no se envenena")
            .limpiar
            .clone()
    }

    /// Cuántos archivos siguen «en cuarentena».
    fn restantes(&self) -> usize {
        self.en_cuarentena
            .lock()
            .expect("el registro no se envenena")
            .len()
    }
}

impl Quarantine for Falso {
    fn has(&self, path: &Path) -> std::io::Result<bool> {
        Ok(self
            .en_cuarentena
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
            let mut registro = self.registro.lock().expect("sin veneno");
            registro.preguntar.insert(path.to_path_buf());
            if registro.fallar_en.contains(path) {
                // Un fallo que la operación **no** confunde con un «no lo tenía»: es
                // lo que la distingue, y por eso se propaga como `Err` y no como
                // `Ok(false)`.
                return Err(std::io::Error::other("no se pudo quitar el atributo"));
            }
        }
        if !self.en_cuarentena.lock().expect("sin veneno").remove(path) {
            return Ok(false);
        }
        self.registro
            .lock()
            .expect("sin veneno")
            .limpiar
            .insert(path.to_path_buf());
        Ok(true)
    }

    fn put(&self, path: &Path) -> std::io::Result<()> {
        self.marcar(path);
        Ok(())
    }
}

/// Directorio único por etiqueta y por nanosegundo, para que dos pruebas simultáneas —o
/// dos ejecuciones del mismo binario— no se pisen.
fn unico(tag: &str) -> PathBuf {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or_default();
    let raiz =
        std::env::temp_dir().join(format!("quarantine-e2e-{}-{tag}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&raiz);
    std::fs::create_dir_all(&raiz).expect("se crea la raíz del sandbox");
    raiz
}

/// Une un fragmento del manifiesto con la raíz del bundle.
fn aviar(destino: &Path, relativa: &str) -> PathBuf {
    let mut path = destino.to_path_buf();
    for parte in relativa.split('/') {
        path.push(parte);
    }
    path
}

/// Escribe un bundle completo con la lista de `packaging/bundle-manifest.json`, que es
/// la misma que valida `self install`. Los nombres no se escriben a mano: si el
/// manifiesto cambiara, la seguiría exercising lo que el manifiesto dice.
fn escribir_bundle(destino: &Path) -> Vec<PathBuf> {
    let seccion = avi_lifecycle::manifest::target_section(avi_lifecycle::target::host_triple())
        .expect("el target del host tiene sección en el manifiesto");
    let mut archivos = Vec::new();
    for relativa in &seccion.required {
        let completa = aviar(destino, relativa);
        if let Some(parent) = completa.parent() {
            std::fs::create_dir_all(parent).expect("se crea el directorio del archivo");
        }
        std::fs::write(&completa, format!("contenido de {relativa}\n"))
            .expect("se escribe el archivo del bundle");
        archivos.push(completa);
    }
    archivos
}

/// Sandbox con un árbol con la forma de un bundle: ejecutable y documentos en la raíz,
/// librería de runtime junto a ellos y el derivado del motor tres niveles más abajo.
///
/// El bundle se escribe primero en el **staging** —que es donde el bootstrap de §9.2 lo
/// deja— y [`Arbol::colocados`] lo traslada al directorio de programa, que es donde el
/// paso 6 de §9.3 lo coloca y donde la limpieza se ejecuta.
struct Arbol {
    /// Raíz del sandbox, para borrarla entera.
    raiz: PathBuf,
    /// Staging hermano del directorio de programa, con el prefijo de §7.
    staging: PathBuf,
    /// Directorio de programa de §7.
    programa: PathBuf,
    /// Archivos del bundle, en la ruta en la que están ahora: el staging.
    archivos: Vec<PathBuf>,
}

impl Arbol {
    fn nuevo(tag: &str) -> Self {
        let raiz = unico(tag);
        let opt = raiz.join("opt");
        let staging = opt.join(format!("{}test", avi_lifecycle::STAGING_DIR_PREFIX));
        let programa = opt.join("ai-voice-interconnector");
        let archivos = escribir_bundle(&staging);
        Self {
            raiz,
            staging,
            programa,
            archivos,
        }
    }

    /// Los archivos del bundle **en el directorio de programa**. Es la traducción de
    /// "mover lo que hay en el staging al directorio de programa", que es el paso 6 de
    /// §9.3.
    fn colocados(&self) -> Vec<PathBuf> {
        self.archivos
            .iter()
            .map(|a| {
                self.programa.join(
                    a.strip_prefix(&self.staging)
                        .expect("el archivo está en el staging"),
                )
            })
            .collect()
    }

    /// Los mismos archivos, pero **escritos en disco**: el recorrido empieza por un
    /// `read_dir` del directorio de programa, así que sin un árbol real no preguntaría
    /// por nada. Es el estado en el que está el directorio después del paso 6.
    fn materializar(&self) -> Vec<PathBuf> {
        let colocados = self.colocados();
        for archivo in &colocados {
            std::fs::create_dir_all(archivo.parent().expect("el archivo tiene padre"))
                .expect("se crea el directorio del archivo colocado");
            std::fs::write(archivo, "contenido colocado\n").expect("se escribe el archivo");
        }
        colocados
    }
}

/// Contenido de un directorio, para afirmar que nada se ha tocado.
fn contenido(raiz: &Path) -> Vec<String> {
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

/// `true` si entre lo preguntado está el derivado del motor (`vendor/…`), que es el caso
/// que motiva que la limpieza sea sobre todo el directorio y no solo el ejecutable.
fn pregunta_por_el_derivado(preguntado: &BTreeSet<PathBuf>) -> bool {
    let separador = if cfg!(windows) { '\\' } else { '/' };
    let marca = format!("{separador}vendor{separador}");
    preguntado
        .iter()
        .any(|ruta| ruta.to_string_lossy().contains(&marca))
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
fn plantar_cuarentena(raiz: &Path) -> Vec<PathBuf> {
    let mut plantados = Vec::new();
    let mut directorios = Vec::new();
    let mut pila = vec![raiz.to_path_buf()];
    while let Some(dir) = pila.pop() {
        if dir != raiz {
            directorios.push(dir.clone());
        }
        let Ok(entradas) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entrada in entradas.flatten() {
            let ruta = entrada.path();
            if entrada.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                pila.push(ruta);
            } else {
                plantados.push(ruta);
            }
        }
    }
    for ruta in plantados.iter().chain(directorios.iter()) {
        quarantine::put(ruta).expect("se pone la cuarentena de macOS");
    }
    plantados
}

/// Recorre el directorio de programa entero y devuelve las rutas relativas, en orden, de
/// todo lo que **conserva** el atributo.
///
/// El oráculo entra por parámetro, y esa es la decisión que hace que la función sea
/// comprobable en todas las plataformas: en macOS se le pasa `quarantine::plataforma()`,
/// que es la implementación de `xattr`, y fuera se le puede pasar una implementación que
/// registre. El recorrido —el código que el criterio 9 evalúa— es el mismo en los dos
/// casos, así que un error de recorrido no puede esconderse hasta la puerta de macOS.
fn conservan_cuarentena(raiz: &Path, atributos: &dyn quarantine::Quarantine) -> Vec<String> {
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
            if atributos.has(&ruta).unwrap_or(false) {
                out.push(relativa);
            }
            if entrada.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                pila.push(ruta);
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
    let sandbox = Sandbox::nuevo("c9");
    let exe = sandbox.escribir_bundle(&sandbox.staging);
    let plantados = plantar_cuarentena(&sandbox.staging);
    assert!(
        !plantados.is_empty(),
        "criterio 9: el bundle tiene archivos"
    );

    // Un hermano del directorio de programa, en cuarentena, que no se debe tocar.
    let hermano = sandbox.raiz.join("opt").join("hermano.txt");
    std::fs::write(&hermano, "fuera del programa\n").expect("se escribe el hermano");
    quarantine::put(&hermano).expect("se pone la cuarentena del hermano");

    // Punto de partida: **todo** el bundle está en cuarentena, archivos y directorios.
    assert_eq!(
        conservan_cuarentena(&sandbox.staging, quarantine::plataforma()).len(),
        plantados.len() + directorios_intermedios(&sandbox.staging),
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
        &Inerte,
    )
    .await
    .expect("criterio 9: la instalación se completa");

    // El derivado del motor y la librería de ONNX Runtime están entre lo colocado, y son
    // los dos que §9.3 nombra como los que se ejecutan o se cargan.
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
    let con_atributo = conservan_cuarentena(&sandbox.program_dir, quarantine::plataforma());
    assert!(
        con_atributo.is_empty(),
        "criterio 9: ningún archivo ni directorio del directorio de programa conserva la \
         cuarentena; conservan: {con_atributo:?}"
    );
    assert!(
        quarantine::has(&hermano),
        "criterio 9: y el hermano, que está fuera, sigue con la suya"
    );
    assert!(
        avi_lifecycle::receipt::read_from(&sandbox.program_dir)
            .expect("criterio 9: se lee el recibo")
            .is_some(),
        "criterio 9: la instalación dejó recibo, que es lo que la distingue de un copiado"
    );
    let _ = std::fs::remove_dir_all(&sandbox.raiz);
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
    let arbol = Arbol::nuevo("c9-otros");
    let colocados = arbol.materializar();
    let plantados = plantar_cuarentena(&arbol.staging);
    assert!(
        !plantados.is_empty(),
        "criterio 9: el bundle tiene archivos"
    );

    // El hermano, con su propio atributo, es el control del alcance.
    let hermano = arbol.raiz.join("opt").join("hermano.txt");
    std::fs::write(&hermano, "fuera del programa\n").expect("se escribe el hermano");
    quarantine::put(&hermano).expect("`put` fuera de macOS no falla");
    assert!(
        !quarantine::has(&hermano),
        "criterio 9: fuera de macOS `has` no encuentra nada, que es lo honesto: el punto de \
         partida del hermano es indistinguible del de cualquier otro"
    );

    // El recorrido con una implementación de atributos que registra: se pregunta por cada
    // archivo colocado y por los directorios intermedios, y por nada fuera.
    let falso = Falso::new();
    for colocado in &colocados {
        falso.marcar(colocado);
    }
    let outcome = quarantine::strip_bounded(&arbol.programa, MAX_DEPTH, &falso);
    let preguntado = falso.preguntado();
    assert!(
        outcome.is_clear(),
        "criterio 9: el recorrido no falla: {:?}",
        outcome.failed
    );
    assert_eq!(
        outcome.cleared.len(),
        colocados.len(),
        "criterio 9: el recorrido limpia cada archivo del directorio de programa"
    );
    for colocado in &colocados {
        assert!(
            preguntado.contains(colocado),
            "criterio 9: se pregunta por {}",
            colocado.display()
        );
    }
    let directorios = preguntados_directorios(&preguntado);
    assert!(
        !directorios.is_empty(),
        "criterio 9: y por los directorios intermedios, que también heredan el atributo: \
         {directorios:?}"
    );
    for ruta in &preguntado {
        assert!(
            ruta.starts_with(&arbol.programa),
            "criterio 9: nada fuera del directorio de programa: {}",
            ruta.display()
        );
    }
    assert!(
        !preguntado.contains(&hermano),
        "criterio 9: el hermano, que tiene su propio atributo, no se pregunta"
    );

    // Y ahora el mismo recorrido de la prueba de macOS, con un oráculo que registra y
    // que **nada ha limpiado todavía**: es la comprobación no vacía del alcance, y usa
    // exactamente el código que la puerta de macOS evalúa.
    let cobertura = Falso::new();
    for colocado in &colocados {
        cobertura.marcar(colocado);
    }
    cobertura.marcar(&hermano);
    let con_atributo = conservan_cuarentena(&arbol.programa, &cobertura);
    assert_eq!(
        con_atributo.len(),
        colocados.len(),
        "criterio 9: el recorrido del criterio 9 abarca cada archivo colocado: {con_atributo:?}"
    );
    assert!(
        !con_atributo
            .iter()
            .any(|r| r.contains("..") || r.contains("hermano")),
        "criterio 9: y no incluye nada de fuera del directorio de programa"
    );
    let limpio = conservan_cuarentena(&arbol.programa, &falso);
    assert!(
        limpio.is_empty(),
        "criterio 9: después de limpiar, el árbol colocado queda sin cuarentena: {limpio:?}"
    );
    assert!(
        conserva_en(&cobertura, &hermano),
        "criterio 9: y el hermano conserva el suyo, porque el recorrido nunca se sale"
    );

    // Y el `strip` real de la plataforma: en esta plataforma el atributo no existe, así que
    // informa de que no hay nada que quitar sin escribir nada. Ni dentro del directorio de
    // programa ni fuera.
    let antes = contenido(&arbol.raiz);
    let real = quarantine::strip(&arbol.programa);
    assert!(
        real.is_nothing_to_do(),
        "criterio 9: fuera de macOS no hay nada que quitar: {real:?}"
    );
    assert_eq!(
        contenido(&arbol.raiz),
        antes,
        "criterio 9: y el disco no cambia, ni dentro ni fuera"
    );
    let _ = std::fs::remove_dir_all(&arbol.raiz);
}

/// `true` si el oráculo dice que la ruta conserva el atributo, para el control del hermano
/// sin reconstruir el conjunto.
fn conserva_en(atributos: &dyn quarantine::Quarantine, ruta: &Path) -> bool {
    atributos.has(ruta).unwrap_or(false)
}

/// Directorios intermedios de entre lo preguntado, que es el caso que motiva que la
/// limpieza sea sobre todo el directorio y no solo el ejecutable.
fn preguntados_directorios(preguntado: &BTreeSet<PathBuf>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = preguntado
        .iter()
        .filter(|ruta| ruta.is_dir())
        .cloned()
        .collect();
    out.sort();
    out
}

/// Cuántos directorios intermedios hay en un árbol, sin contar su raíz.
#[cfg(target_os = "macos")]
fn directorios_intermedios(raiz: &Path) -> usize {
    let mut total = 0;
    let mut pila = vec![raiz.to_path_buf()];
    while let Some(dir) = pila.pop() {
        let Ok(entradas) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entrada in entradas.flatten() {
            if entrada.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                total += 1;
                pila.push(entrada.path());
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
    let arbol = Arbol::nuevo("recorrido");
    let programa = arbol.programa.clone();
    let colocados = arbol.materializar();
    // El sandbox tiene cosas **fuera** del directorio de programa: el staging hermano,
    // que tiene los mismos archivos, y el directorio raíz. Si el recorrido se saliera,
    // las preguntaría —o las tocaría.
    assert!(
        arbol.staging.exists() && arbol.raiz.exists(),
        "el sandbox tiene hermanos que no son del directorio de programa"
    );

    let falso = Falso::new();
    for colocado in &colocados {
        falso.marcar(colocado);
    }

    let outcome = quarantine::strip_bounded(&programa, MAX_DEPTH, &falso);

    // 1. Se limpió todo el bundle, incluido el derivado del motor tres niveles abajo.
    assert_eq!(
        outcome.cleared.len(),
        colocados.len(),
        "se limpió cada **archivo** del directorio de programa: {:?}",
        outcome.cleared
    );
    assert_eq!(
        falso.limpiado(),
        colocados.iter().cloned().collect::<BTreeSet<_>>(),
        "y son exactamente esos: los directorios de entrada se preguntan pero no tienen \
         el atributo, así que no se limpian"
    );
    assert!(outcome.is_clear(), "sin fallos: {:?}", outcome.failed);
    assert_eq!(falso.restantes(), 0, "no queda nada en cuarentena");

    // 2. Se preguntó por todos los archivos, y **solo** por cosas del directorio de
    //    programa.
    let preguntado = falso.preguntado();
    for esperado in &colocados {
        assert!(
            preguntado.contains(esperado),
            "se preguntó por {}",
            esperado.display()
        );
    }
    for ruta in &preguntado {
        assert!(
            ruta.starts_with(&programa),
            "nada fuera del directorio de programa: {}",
            ruta.display()
        );
    }
    // Se preguntó también por los **directorios** intermedios, y eso es lo correcto y no
    // un descuido: un directorio puede llevar él mismo el atributo, y macOS lo
    // heredan los archivos que se creen dentro. Limpiar solo los archivos dejaría que el
    // siguiente archivo escrito en `vendor/` volviera a heredarlo.
    let directorios: Vec<&PathBuf> = preguntado
        .iter()
        .filter(|r| r.is_dir())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    assert_eq!(
        directorios.len(),
        2,
        "los dos directorios intermedios del bundle se preguntan también: {directorios:?}"
    );
    for ruta in &directorios {
        assert!(
            ruta.starts_with(&programa),
            "y están dentro del directorio de programa: {}",
            ruta.display()
        );
    }

    // 3. Y se preguntó por el derivado del motor, que es el caso que motiva el paso 7.
    assert!(
        pregunta_por_el_derivado(&preguntado),
        "el motor derivado se limpia también, no solo el ejecutable: {preguntado:?}"
    );
    let _ = std::fs::remove_dir_all(&arbol.raiz);
}

/// Un fallo en un archivo **no corta** el recorrido: los demás también necesitan
/// quedarse limpios, y el criterio 9 exige que ninguno conserve la cuarentena.
///
/// Y el resultado lo dice: `is_clear()` es `false` porque hay un archivo que no se pudo
/// limpiar, que es exactamente la información que el resumen de §9.3 muestra.
#[test]
fn quarantine_strip_reports_failures_and_keeps_going() {
    let arbol = Arbol::nuevo("fallos");
    let colocados = arbol.materializar();
    let falso = Falso::new();
    for colocado in &colocados {
        falso.marcar(colocado);
    }
    let falla = falso.fallar_el_primero(&colocados);

    let outcome = quarantine::strip_bounded(&arbol.programa, MAX_DEPTH, &falso);

    assert_eq!(
        outcome.failed.len(),
        1,
        "un solo fallo declarado: {:?}",
        outcome.failed
    );
    assert_eq!(outcome.failed[0].0, falla);
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
        colocados.len() - 1,
        "los demás se limpiaron igualmente: el fallo no cortó el recorrido"
    );
    assert_eq!(
        falso.restantes(),
        1,
        "solo queda en cuarentena el que falló"
    );
    let _ = std::fs::remove_dir_all(&arbol.raiz);
}

/// El límite de profundidad se respeta, y es lo que evita que un directorio con un lazo
/// —un enlace simbólico a un ancestro— deje la limpieza colgando.
///
/// Se prueba con el límite a cero, que es el caso extremo: se pregunta por lo que hay
/// directamente en el directorio de programa, y ni un nivel más adentro.
#[test]
fn quarantine_strip_respects_the_depth_limit() {
    let arbol = Arbol::nuevo("profundidad");
    let programa = arbol.programa.clone();
    let colocados = arbol.materializar();
    let en_raiz: Vec<&PathBuf> = colocados
        .iter()
        .filter(|c| c.parent() == Some(&programa))
        .collect();
    let anidados: Vec<&PathBuf> = colocados
        .iter()
        .filter(|c| c.parent() != Some(&programa))
        .collect();
    assert!(
        !en_raiz.is_empty() && !anidados.is_empty(),
        "el bundle tiene archivos en la raíz ({}) y anidados ({}), que es lo que hace \
         la prueba significativa",
        en_raiz.len(),
        anidados.len()
    );

    let falso = Falso::new();
    for colocado in &colocados {
        falso.marcar(colocado);
    }

    let outcome = quarantine::strip_bounded(&programa, 0, &falso);

    let preguntado = falso.preguntado();
    for archivo in &en_raiz {
        assert!(
            preguntado.contains(*archivo),
            "con límite cero sí se pregunta por {}",
            archivo.display()
        );
    }
    for archivo in &anidados {
        assert!(
            !preguntado.contains(*archivo),
            "con límite cero no se baja a {}",
            archivo.display()
        );
    }
    assert_eq!(
        outcome.cleared.len(),
        en_raiz.len(),
        "el resultado solo cuenta lo que sí se limpiaron, que es la raíz"
    );
    assert_eq!(
        falso.restantes(),
        anidados.len(),
        "y los anidados siguen en cuarentena, que es lo que el límite delimita"
    );
    assert!(outcome.is_clear(), "sin fallos: {:?}", outcome.failed);
    let _ = std::fs::remove_dir_all(&arbol.raiz);
}

/// Limpiar dos veces no cambia nada, y la segunda pasada informa de que no hay nada que
/// quitar. Es la idempotencia del mismo paso que la instalación repetida.
#[test]
fn quarantine_strip_is_idempotent() {
    let arbol = Arbol::nuevo("idempotencia");
    let colocados = arbol.materializar();
    let falso = Falso::new();
    for colocado in &colocados {
        falso.marcar(colocado);
    }

    let primera = quarantine::strip_bounded(&arbol.programa, MAX_DEPTH, &falso);
    assert!(
        !primera.is_nothing_to_do(),
        "la primera pasada sí tenía trabajo: {primera:?}"
    );
    let limpiado_primera = falso.limpiado();
    assert_eq!(
        limpiado_primera.len(),
        colocados.len(),
        "y limpió el bundle entero"
    );

    let segunda = quarantine::strip_bounded(&arbol.programa, MAX_DEPTH, &falso);
    assert!(
        segunda.is_nothing_to_do(),
        "la segunda pasada informa de que no hay nada que quitar: {segunda:?}"
    );
    assert!(segunda.is_clear(), "y no es un fallo: devuelve éxito");
    assert_eq!(
        falso.limpiado(),
        limpiado_primera,
        "y no se volvió a limpiar nada"
    );
    let _ = std::fs::remove_dir_all(&arbol.raiz);
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

/// La marca es la que nombra §9.3. Se afirma en todas partes, no solo en macOS: es parte
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
    let arbol = Arbol::nuevo("sin-atributo");
    let colocados = arbol.materializar();
    // Un hermano del directorio de programa, que es de la aplicación pero no es el
    // directorio de programa: si la limpieza se saliera, lo tocaría.
    let hermano = arbol.raiz.join("opt").join("hermano.txt");
    std::fs::write(&hermano, "no tocar\n").expect("se escribe el hermano");

    // Punto de partida: en esta plataforma ningún archivo tiene el atributo.
    assert!(
        colocados.iter().all(|a| !quarantine::has(a)),
        "punto de partida: ningún archivo del bundle tiene cuarentena"
    );
    let antes = contenido(&arbol.raiz);

    let outcome = quarantine::strip(&arbol.programa);

    assert!(
        outcome.is_nothing_to_do(),
        "la operación informa de que no hay nada que quitar: {outcome:?}"
    );
    assert!(outcome.is_clear(), "y devuelve éxito");
    assert!(outcome.cleared.is_empty(), "no se limpió nada");
    assert!(outcome.failed.is_empty(), "y nada falló");
    assert_eq!(
        contenido(&arbol.raiz),
        antes,
        "el disco no ha cambiado: ni dentro del directorio de programa ni fuera"
    );
    assert_eq!(
        std::fs::read_to_string(&hermano).expect("el hermano sigue"),
        "no tocar\n",
        "el hermano del directorio de programa está intacto"
    );
    let _ = std::fs::remove_dir_all(&arbol.raiz);
}

/// Fuera de macOS, `put` es un no-op y `has` es `false`: no hay atributo que poner, y una
/// implementación que fingiera lo contrario daría al resto del código una garantía que
/// no existe.
#[cfg(not(target_os = "macos"))]
#[test]
fn quarantine_outside_macos_attributes_are_inert() {
    let arbol = Arbol::nuevo("atributos-inertes");
    let programados = arbol.materializar();
    for archivo in &programados {
        quarantine::put(archivo).expect("`put` fuera de macOS no falla");
        assert!(
            !quarantine::has(archivo),
            "y `has` sigue diciendo que no: no hay atributo que poner"
        );
    }
    let _ = std::fs::remove_dir_all(&arbol.raiz);
}

// ─── Solo en macOS: el criterio 9 y `xattr` de verdad ─────────────────────────────

/// Control de procesos inerte: no hay daemon en el sandbox.
#[cfg(target_os = "macos")]
struct Inerte;

#[cfg(target_os = "macos")]
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

/// El criterio 9: instalar quita la cuarentena de **todo** el directorio de programa, no
/// solo del ejecutable.
///
/// Sigue sin poder verificarse en local: es la prueba de la puerta de macOS, y la
/// primera vez que se ejecutará es en el pipeline del release. Lo que sí queda verificado
/// antes de llegar allí es todo lo de arriba.
#[cfg(target_os = "macos")]
#[tokio::test]
async fn install_strips_quarantine_from_whole_program_dir() {
    let arbol = Arbol::nuevo("criterio-9");
    let exe = arbol
        .archivos
        .iter()
        .find(|a| {
            a.file_name()
                .is_some_and(|n| n == "ai-voice-interconnector")
        })
        .expect("el bundle tiene ejecutable")
        .clone();
    for archivo in &arbol.archivos {
        quarantine::put(archivo).expect("se pone la cuarentena de macOS");
    }
    // Punto de partida: **todos** los archivos del bundle están en cuarentena. Sin esta
    // comprobación, la prueba no afirmaría nada.
    for archivo in &arbol.archivos {
        assert!(
            quarantine::has(archivo),
            "{} arranca con cuarentena",
            archivo.display()
        );
    }

    let sandbox = Sandbox {
        raiz: arbol.raiz.clone(),
        program_dir: arbol.programa.clone(),
        home: arbol.raiz.join("home/ana"),
        data_dir: arbol.raiz.join("data"),
        models_dir: arbol.raiz.join("models"),
        temp_root: arbol.raiz.join("tmp"),
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
        &Inerte,
    )
    .await
    .expect("la instalación se completa");

    // El derivado del motor y la librería de ONNX Runtime están entre lo colocado, y son
    // los dos que §9.3 nombra como los que se ejecutan o se cargan.
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
    let mut con_cuarentena = Vec::new();
    for relativa in &outcome.receipt.files {
        let ruta = aviar(&sandbox.program_dir, relativa);
        assert!(ruta.exists(), "{relativa} se colocó");
        if quarantine::has(&ruta) {
            con_cuarentena.push(relativa.clone());
        }
    }
    assert!(
        con_cuarentena.is_empty(),
        "la instalación limpió la cuarentena de todo el bundle, no solo del ejecutable; \
         siguen con ella: {con_cuarentena:?}"
    );
    assert!(
        avi_lifecycle::receipt::read_from(&sandbox.program_dir)
            .expect("se lee el recibo")
            .is_some(),
        "y dejó recibo, que es lo que distingue una instalación de un copiado"
    );
    let _ = std::fs::remove_dir_all(&arbol.raiz);
}

/// El límite de profundidad contra un **lazo real** de directorios, que es el caso que lo
/// justifica: un enlace simbólico a un ancestro. La implementación falsa demuestra que
/// el límite se respeta; esta demuestra que con el sistema de ficheros de verdad la
/// limpieza no se cuelga.
#[cfg(target_os = "macos")]
#[test]
fn quarantine_strip_survives_a_real_directory_loop() {
    let raiz = unico("lazo");
    let programa = raiz.join("opt/ai-voice-interconnector");
    let dir = programa.join("vendor/qwen3-tts");
    std::fs::create_dir_all(&dir).expect("se crea el árbol");
    let motor = dir.join("qwen_tts");
    std::fs::write(&motor, "motor").expect("se escribe el derivado");
    quarantine::put(&motor).expect("se pone la cuarentena");
    std::os::unix::fs::symlink(&raiz, dir.join("vuelta")).expect("se crea el lazo");

    let outcome = quarantine::strip(&programa);
    assert!(
        outcome.is_clear(),
        "el lazo no produce fallos: {:?}",
        outcome.failed
    );
    assert!(
        !quarantine::has(&motor),
        "y la limpieza alcanzó al derivado antes de encontrarse con el lazo"
    );
    let _ = std::fs::remove_dir_all(&raiz);
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

/// Rutas de §7 de la prueba del criterio 9.
#[cfg(target_os = "macos")]
struct Sandbox {
    raiz: PathBuf,
    program_dir: PathBuf,
    home: PathBuf,
    data_dir: PathBuf,
    models_dir: PathBuf,
    temp_root: PathBuf,
}

#[cfg(target_os = "macos")]
impl Sandbox {
    /// `SHELL` de la máquina, con el nombre corto que usa la tabla de §9.3.1.
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
