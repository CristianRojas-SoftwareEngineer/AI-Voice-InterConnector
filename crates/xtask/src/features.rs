//! `xtask features --check`: puerta de la cadena de features del workspace.
//!
//! Qué capacidades expone un paquete se declara a la vez en varios sitios: los
//! features del paquete raíz, los features que declara cada crate, y los `cfg`
//! del código fuente que compilan o descartan piezas según un feature. Nada ata
//! las tres declaraciones, así que un eslabón puede faltar sin que ni el
//! compilador ni los tests lo noten: el código se compila, el binario arranca y
//! la pieza sencillamente no existe en el producto.
//!
//! La invariante exigida es la clausura: todo feature que un crate declara y usa
//! en su código tiene que ser alcanzable activando algún feature del paquete
//! raíz. Es la condición para que la superficie del producto no dependa de un
//! accidente de unificación de features, y convierte un eslabón ausente en un
//! fallo de la puerta en vez de en un reporte de usuario.
//!
//! Sin dependencias nuevas: el inventario está congelado y lo que hace falta es
//! lectura de manifiestos, una expresión regular sobre el código fuente y
//! comparación de cadenas. No compila nada, de modo que corre en la puerta
//! featureless sin C++.

use anyhow::{bail, Result};
use regex::Regex;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Directorio del workspace donde viven los crates.
const CRATES_DIR: &str = "crates";
/// Nombre del manifiesto de un paquete Rust.
const MANIFEST: &str = "Cargo.toml";
/// Nombre del directorio con el código fuente de un crate.
const SRC_DIR: &str = "src";

/// Ejecuta el subcomando: `--check` verifica la clausura de la cadena.
pub(crate) fn run(check: bool, root: Option<&Path>) -> Result<()> {
    if !check {
        println!("Usa --check para verificar la cadena de features del workspace");
        return Ok(());
    }
    let base = root.map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let manifest = read_manifest(&base.join(MANIFEST))?;
    let declared = feature_table(&manifest);
    let reachable = closure(&declared, dependency_seeds(&manifest));
    verify_reachable_are_declared(&base, &reachable)?;
    verify_used_features_reachable(&base, &reachable)?;
    println!(
        "cadena de features coherente: {} feature(s) del paquete raíz, {} pareja(s) alcanzable(s)",
        declared.len(),
        reachable.len()
    );
    Ok(())
}

/// Lee un manifiesto de disco, con el fallo anotado con su ruta para que el
/// diagnóstico sea accionable sin tener que reconstruir la ruta a mano.
fn read_manifest(path: &Path) -> Result<String> {
    std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("no se pudo leer {}: {e}", path.display()))
}

/// Extrae la tabla de features de un manifiesto: nombre del feature a la lista
/// de entradas que activa. Una entrada sin `/` es un feature del propio paquete
/// y otra con `/` es una pareja `crate/feature`.
fn feature_table(manifest: &str) -> BTreeMap<String, Vec<String>> {
    let mut table = BTreeMap::new();
    let mut inside = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            inside = line == "[features]";
            continue;
        }
        if !inside {
            continue;
        }
        let Some((name, values)) = line.split_once('=') else {
            continue;
        };
        table.insert(unquote(name), parse_list(values));
    }
    table
}

/// Convierte un array inline de TOML en la lista de sus entradas, ignorando las
/// vacías que deja un separador final.
fn parse_list(values: &str) -> Vec<String> {
    let Some(open) = values.find('[') else {
        return Vec::new();
    };
    let Some(close) = values.rfind(']') else {
        return Vec::new();
    };
    values[open + 1..close]
        .split(',')
        .map(unquote)
        .filter(|item| !item.is_empty())
        .collect()
}

/// Quota las comillas de una clave o de una entrada de array.
fn unquote(value: &str) -> String {
    value
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .to_string()
}

/// Parejas `crate/feature` que el manifiesto pide al declarar una dependencia
/// del workspace con su lista de features, como la dev-dependency que activa el
/// punto de inyección de fallos. Solo cuentan las dependencias con `path`: las
/// de crates.io quedan fuera del alcance del validador, que gobierna los crates
/// del workspace y no puede leer sus manifiestos.
fn dependency_seeds(manifest: &str) -> BTreeSet<String> {
    let mut pairs = BTreeSet::new();
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            continue;
        }
        let Some((name, rest)) = line.split_once('=') else {
            continue;
        };
        if !rest.contains("path") || !rest.contains("features") {
            continue;
        }
        let krate = unquote(name);
        for feature in parse_list(rest) {
            pairs.insert(format!("{krate}/{feature}"));
        }
    }
    pairs
}

/// Clausura transitiva de los features del paquete raíz: qué parejas
/// `crate/feature` se pueden activar. Las entradas sin `/` son features del
/// propio paquete y se resuelven encolando su nombre hasta el punto fijo, que
/// es lo que hace que un feature agregador alcance lo que sus componentes
/// declaran. Las entradas `dep:` no implican ningún feature y se descartan.
fn closure(declared: &BTreeMap<String, Vec<String>>, seeds: BTreeSet<String>) -> BTreeSet<String> {
    let mut reachable = seeds;
    let mut pending: Vec<String> = declared.keys().cloned().collect();
    let mut visited: BTreeSet<String> = BTreeSet::new();
    while let Some(feature) = pending.pop() {
        if !visited.insert(feature.clone()) {
            continue;
        }
        let Some(entries) = declared.get(&feature) else {
            continue;
        };
        for entry in entries {
            if entry.starts_with("dep:") {
                continue;
            }
            match entry.split_once('/') {
                Some((krate, sub)) => {
                    reachable.insert(format!("{krate}/{sub}"));
                }
                None => pending.push(entry.clone()),
            }
        }
    }
    reachable
}

/// Comprueba que cada pareja alcanzable apunte a un feature que el crate
/// correspondiente declara de verdad. Una pareja hacia un crate que no está en el
/// workspace se ignora: el validador solo gobierna los suyos.
fn verify_reachable_are_declared(base: &Path, reachable: &BTreeSet<String>) -> Result<()> {
    let mut errors = Vec::new();
    for pair in reachable {
        let Some((krate, feature)) = pair.split_once('/') else {
            continue;
        };
        let manifest = base.join(CRATES_DIR).join(krate).join(MANIFEST);
        if !manifest.is_file() {
            continue;
        }
        let declared = feature_table(&read_manifest(&manifest)?);
        if !declared.contains_key(feature) {
            errors.push(format!(
                "el paquete raíz pide {pair} pero el crate {krate} no declara ese feature"
            ));
        }
    }
    fail(errors)
}

/// Comprueba que todo feature que un crate declara y además usa en su código
/// fuente para compilar o descartar piezas sea alcanzable desde el paquete raíz.
/// Es la invariante que delata un eslabón ausente: el código se compila igual,
/// pero la pieza nunca llega a existir en el producto que se distribuye.
fn verify_used_features_reachable(base: &Path, reachable: &BTreeSet<String>) -> Result<()> {
    let mut errors = Vec::new();
    let crates = base.join(CRATES_DIR);
    let Ok(entries) = crates.read_dir() else {
        return Ok(());
    };
    for entry in entries.flatten() {
        if !entry.path().is_dir() {
            continue;
        }
        let krate = entry.file_name().to_string_lossy().to_string();
        let manifest = entry.path().join(MANIFEST);
        if !manifest.is_file() {
            continue;
        }
        let declared = feature_table(&read_manifest(&manifest)?);
        if declared.is_empty() {
            continue;
        }
        for feature in gated_features(&entry.path().join(SRC_DIR)) {
            if !declared.contains_key(&feature) {
                continue;
            }
            let pair = format!("{krate}/{feature}");
            if !reachable.contains(&pair) {
                errors.push(format!(
                    "{pair} se usa en el código de {krate} pero ningún feature del paquete raíz lo activa"
                ));
            }
        }
    }
    fail(errors)
}

/// Features que aparecen en un `cfg(feature = ...)` del código fuente del crate.
/// El patrón exige que `feature` sea el primer argumento del `cfg`, de modo que
/// una combinación como `all(test, feature = ...)` no cuenta: ahí el feature
/// solo se exige dentro de una rama de test y no gobierna la superficie del
/// producto.
fn gated_features(src: &Path) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let pattern = Regex::new(r#"cfg\s*\(\s*feature\s*=\s*"([^"]+)""#).expect("el patrón es válido");
    let Ok(entries) = src.read_dir() else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for capture in pattern.captures_iter(&text) {
            found.insert(capture[1].to_string());
        }
    }
    found
}

/// Convierte la lista de diagnósticos en el resultado del subcomando.
fn fail(errors: Vec<String>) -> Result<()> {
    if errors.is_empty() {
        return Ok(());
    }
    for error in &errors {
        println!("ERROR: {error}");
    }
    bail!("{} divergencia(s) en la cadena de features", errors.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// La clausura ve la pareja que declara un feature del paquete raíz, que es
    /// el caso directo que el validador exige.
    #[test]
    fn closure_sees_direct_pair() {
        let mut table = BTreeMap::new();
        table.insert(
            "native-stt".to_string(),
            vec![
                "avi-stt/native-stt".to_string(),
                "avi-daemon/native-stt".to_string(),
            ],
        );
        let reachable = closure(&table, BTreeSet::new());
        assert!(reachable.contains("avi-stt/native-stt"));
        assert!(reachable.contains("avi-daemon/native-stt"));
    }

    /// Un feature agregador no puede cortar la cadena: sus componentes se
    /// resuelven y lo que declaran acaba en la clausura.
    #[test]
    fn closure_resolves_through_aggregate_feature() {
        let mut table = BTreeMap::new();
        table.insert(
            "native-translation".to_string(),
            vec!["avi-daemon/native-translation".to_string()],
        );
        table.insert("full".to_string(), vec!["native-translation".to_string()]);
        let reachable = closure(&table, BTreeSet::new());
        assert!(reachable.contains("avi-daemon/native-translation"));
    }

    /// Una pareja sembrada por una dependencia con `features` explícita cuenta
    /// aunque no pase por la tabla de features del paquete raíz: es el caso del
    /// punto de inyección de fallos, que solo piden las dev-dependencies.
    #[test]
    fn closure_includes_dependency_seeds() {
        let table = BTreeMap::new();
        let seeds = BTreeSet::from(["avi-lifecycle/faults".to_string()]);
        assert!(closure(&table, seeds).contains("avi-lifecycle/faults"));
    }

    /// Una entrada `dep:` activa una dependencia opcional, no un feature, así que
    /// no debe aportar ninguna pareja a la clausura.
    #[test]
    fn closure_ignores_dep_entries() {
        let mut table = BTreeMap::new();
        table.insert(
            "native-translation".to_string(),
            vec!["dep:avi-translation".to_string()],
        );
        assert!(closure(&table, BTreeSet::new()).is_empty());
    }

    /// La tabla se lee sin arrastrar la sección vecina: una clave repetida fuera
    /// de `[features]` no debe cambiar lo que la tabla contiene.
    #[test]
    fn feature_table_ignores_other_sections() {
        let manifest = concat!(
            "[package]\n",
            "name = \"x\"\n",
            "\n",
            "[features]\n",
            "full = [\"native-stt\", \"native-translation\"]\n",
            "\n",
            "[dev-dependencies]\n",
            "full = [\"nativo\"]\n",
        );
        let table = feature_table(manifest);
        assert_eq!(table.get("full").map(|entries| entries.len()), Some(2));
    }
}
