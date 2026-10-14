// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! Ejecutor del árbol de propiedad: planificación y ejecución del borrado de una
//! selección de nodos, con la poda de los contenedores que quedan vacíos.
//!
//! El árbol (`avi_shared::paths::ownership_tree`) distingue tres papeles: las hojas
//! (`Leaf`) son directorios cuyo contenido es todo de la aplicación, los
//! contenedores (`Container`) solo albergan nodos de la aplicación y la raíz externa
//! (`External`) es una ubicación del usuario, la caché HF compartida.
//!
//! **Dos pasadas.** [`execute`] borra primero las rutas de la selección; después
//! recorre de abajo arriba los contenedores ancestros de lo borrado y aplica
//! `remove_dir`, que **nunca** es recursivo: un contenedor con cualquier entrada
//! sigue en pie y la poda de esa rama se detiene ahí, porque todo ancestro suyo lo
//! contiene y tampoco está vacío.
//!
//! **Planificación separada de la ejecución.** [`plan`] predice la poda sobre el
//! disco actual sin modificarlo; la simulación (`--dry-run`) y la ejecución real
//! parten del mismo plan, de modo que lo que se anuncia es lo que se hace. Un
//! contenedor que no quedará vacío solo se informa si contiene entradas que no son
//! nodos del árbol (ajenas): si lo que lo retiene son nodos conservados, como las
//! hojas que `--keep-data` deja, no hay nada que avisar.
//!
//! La raíz externa no se borra entera ni se poda nunca: la poda solo considera
//! nodos `Container`, y [`execute`] rechaza como fallo una ruta de la selección que
//! sea una raíz externa o la contenga.

use avi_shared::paths::{NodeKind, OwnershipNode};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Contenedor que no quedará vacío por entradas ajenas a la aplicación.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Foreign {
    /// Contenedor que se conserva.
    pub container: PathBuf,
    /// Entradas directas que no son nodos del árbol.
    pub entries: Vec<PathBuf>,
}

/// Plan del borrado de una selección de nodos.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    /// Rutas de la selección: hojas, o contenido de ellas, que se borran.
    pub removals: Vec<PathBuf>,
    /// Contenedores que quedarán vacíos y se podan, de abajo arriba.
    pub prune: Vec<PathBuf>,
    /// Contenedores que se conservan por tener entradas ajenas.
    pub foreign: Vec<Foreign>,
}

/// Resultado de [`execute`].
#[derive(Debug, Default)]
pub struct Report {
    /// Rutas de la selección que se borraron (o ya no existían).
    pub removed: Vec<PathBuf>,
    /// Contenedores podados, de abajo arriba.
    pub pruned: Vec<PathBuf>,
    /// Rutas que no se pudieron borrar, con la causa.
    pub failed: Vec<(PathBuf, std::io::Error)>,
}

/// Planifica el borrado de `removals` sobre `tree` sin tocar el disco.
pub fn plan(tree: &[OwnershipNode], removals: &[PathBuf]) -> Plan {
    let removal_keys: BTreeSet<String> = removals.iter().map(|p| key(p)).collect();
    let node_keys: BTreeSet<String> = tree.iter().map(|n| key(&n.path)).collect();
    let mut pruned_keys: BTreeSet<String> = BTreeSet::new();
    let mut prune = Vec::new();
    let mut foreign = Vec::new();

    for container in ancestor_containers(tree, removals) {
        let Ok(entries) = std::fs::read_dir(container) else {
            // Un contenedor que no existe o no se puede leer no se poda.
            continue;
        };
        let remaining: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                let k = key(p);
                !removal_keys.contains(&k) && !pruned_keys.contains(&k)
            })
            .collect();
        if remaining.is_empty() {
            pruned_keys.insert(key(container));
            prune.push(container.to_path_buf());
            continue;
        }
        let entries: Vec<PathBuf> = remaining
            .into_iter()
            .filter(|p| !node_keys.contains(&key(p)))
            .collect();
        if !entries.is_empty() {
            foreign.push(Foreign {
                container: container.to_path_buf(),
                entries,
            });
        }
    }
    Plan {
        removals: removals.to_vec(),
        prune,
        foreign,
    }
}

/// Ejecuta el plan: borra la selección y poda los contenedores que queden vacíos.
pub fn execute(tree: &[OwnershipNode], plan: &Plan) -> Report {
    let mut report = Report::default();
    let externals: Vec<String> = tree
        .iter()
        .filter(|n| n.kind == NodeKind::External)
        .map(|n| key(&n.path))
        .collect();

    // Primera pasada: la selección.
    for path in &plan.removals {
        let k = key(path);
        if externals.iter().any(|ext| k == *ext || is_under(&k, ext)) {
            report.failed.push((
                path.clone(),
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "la raíz externa del usuario no se borra entera",
                ),
            ));
            continue;
        }
        match remove_path(path) {
            Ok(()) => report.removed.push(path.clone()),
            Err(e) => report.failed.push((path.clone(), e)),
        }
    }

    // Segunda pasada: contenedores de abajo arriba; la rama se detiene en el primero
    // que no se puede quitar.
    let mut blocked: Vec<String> = Vec::new();
    for container in ancestor_containers(tree, &plan.removals) {
        let k = key(container);
        if blocked.iter().any(|b| is_under(&k, b)) {
            continue;
        }
        match std::fs::remove_dir(container) {
            Ok(()) => report.pruned.push(container.to_path_buf()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) if e.kind() == std::io::ErrorKind::DirectoryNotEmpty => blocked.push(k),
            Err(e) => {
                blocked.push(k);
                report.failed.push((container.to_path_buf(), e));
            }
        }
    }
    report
}

/// Borra una ruta, sea archivo o directorio. `NotFound` **es** éxito: el plan dice lo
/// que tiene que dejar de existir, y si ya no existe el objetivo está cumplido. No es
/// un caso teórico: la parada del daemon borra el pidfile cuando no había daemon vivo
/// y el pidfile es un destino de `--all`.
pub fn remove_path(path: &Path) -> std::io::Result<()> {
    let outcome = if path.is_dir() && !path.is_symlink() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    };
    match outcome {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// Contenedores del árbol que son ancestros estrictos de alguna ruta de la selección,
/// ordenados de más profundo a menos.
fn ancestor_containers<'a>(tree: &'a [OwnershipNode], removals: &[PathBuf]) -> Vec<&'a Path> {
    let removal_keys: Vec<String> = removals.iter().map(|p| key(p)).collect();
    let mut found: Vec<(String, &Path)> = tree
        .iter()
        .filter(|n| n.kind == NodeKind::Container)
        .map(|n| (key(&n.path), n.path.as_path()))
        .filter(|(k, _)| removal_keys.iter().any(|r| is_under(k, r)))
        .collect();
    // Un hijo siempre tiene la clave más larga que su padre.
    found.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then_with(|| a.0.cmp(&b.0)));
    found.into_iter().map(|(_, path)| path).collect()
}

fn key(path: &Path) -> String {
    crate::canonical_path_key(path)
}

/// `true` si `descendant` cuelga estrictamente de `ancestor`, sobre claves canónicas
/// (separador `\`). Una clave vacía no es ancestro de nadie.
fn is_under(ancestor: &str, descendant: &str) -> bool {
    !ancestor.is_empty() && descendant.starts_with(&format!("{ancestor}\\"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{scratch, write_file};

    fn node(path: PathBuf, kind: NodeKind) -> OwnershipNode {
        OwnershipNode { path, kind }
    }

    /// Árbol de la forma de Windows: `app` contiene `data` y el contenedor `cache`,
    /// que lleva `models` y `tmp`.
    struct Fixture {
        _root: crate::test_support::Scratch,
        app: PathBuf,
        data: PathBuf,
        cache: PathBuf,
        models: PathBuf,
        tmp: PathBuf,
        tree: Vec<OwnershipNode>,
    }

    fn fixture(tag: &str) -> Fixture {
        let root = scratch(tag);
        let app = root.join("app");
        let data = app.join("data");
        let cache = app.join("cache");
        let models = cache.join("models");
        let tmp = cache.join("tmp");
        for leaf in [&data, &models, &tmp] {
            write_file(&leaf.join("x"), "x");
        }
        let tree = vec![
            node(app.clone(), NodeKind::Container),
            node(data.clone(), NodeKind::Leaf),
            node(cache.clone(), NodeKind::Container),
            node(models.clone(), NodeKind::Leaf),
            node(tmp.clone(), NodeKind::Leaf),
        ];
        Fixture {
            _root: root,
            app,
            data,
            cache,
            models,
            tmp,
            tree,
        }
    }

    #[test]
    fn selecting_every_leaf_prunes_all_containers() {
        let f = fixture("ownership-all");
        let removals = vec![f.data.clone(), f.models.clone(), f.tmp.clone()];
        let planned = plan(&f.tree, &removals);
        assert_eq!(planned.prune, vec![f.cache.clone(), f.app.clone()]);
        assert!(planned.foreign.is_empty());

        let report = execute(&f.tree, &planned);
        assert!(report.failed.is_empty(), "{:?}", report.failed);
        assert_eq!(report.pruned, vec![f.cache.clone(), f.app.clone()]);
        assert!(!f.app.exists(), "el contenedor raíz desaparece");
    }

    #[test]
    fn kept_leaf_blocks_pruning_without_reporting() {
        let f = fixture("ownership-kept");
        let removals = vec![f.models.clone(), f.tmp.clone()];
        let planned = plan(&f.tree, &removals);
        assert_eq!(planned.prune, vec![f.cache.clone()]);
        assert!(
            planned.foreign.is_empty(),
            "lo que retiene a `app` es un nodo conservado: no se informa"
        );
        execute(&f.tree, &planned);
        assert!(!f.cache.exists());
        assert!(f.app.exists() && f.data.exists());
    }

    #[test]
    fn kept_leaf_inside_container_keeps_the_container() {
        let f = fixture("ownership-keep-models");
        let removals = vec![f.data.clone(), f.tmp.clone()];
        let planned = plan(&f.tree, &removals);
        assert!(planned.prune.is_empty());
        assert!(planned.foreign.is_empty());
        execute(&f.tree, &planned);
        assert!(f.models.exists() && f.cache.exists() && f.app.exists());
    }

    #[test]
    fn foreign_entry_keeps_the_container_and_is_reported() {
        let f = fixture("ownership-foreign");
        let stranger = f.cache.join("ajeno.txt");
        write_file(&stranger, "a");
        let removals = vec![f.data.clone(), f.models.clone(), f.tmp.clone()];
        let planned = plan(&f.tree, &removals);
        assert!(planned.prune.is_empty());
        assert_eq!(
            planned.foreign,
            vec![Foreign {
                container: f.cache.clone(),
                entries: vec![stranger.clone()],
            }],
            "solo el contenedor que retiene la entrada ajena se informa, no sus ancestros"
        );
        let report = execute(&f.tree, &planned);
        assert!(report.failed.is_empty() && report.pruned.is_empty());
        assert!(stranger.exists() && f.app.exists());
    }

    #[test]
    fn prune_is_never_recursive_on_late_entries() {
        let f = fixture("ownership-late");
        let removals = vec![f.data.clone(), f.models.clone(), f.tmp.clone()];
        let planned = plan(&f.tree, &removals);
        // Aparece una entrada entre la planificación y la ejecución.
        let late = f.cache.join("tardio.txt");
        write_file(&late, "t");
        let report = execute(&f.tree, &planned);
        assert!(report.pruned.is_empty() && report.failed.is_empty());
        assert!(late.exists(), "remove_dir no borra contenido");
    }

    #[test]
    fn partial_selection_inside_a_leaf_does_not_prune() {
        let f = fixture("ownership-inside-leaf");
        let removals = vec![f.data.join("x")];
        let planned = plan(&f.tree, &removals);
        assert!(planned.prune.is_empty() && planned.foreign.is_empty());
        let report = execute(&f.tree, &planned);
        assert_eq!(report.removed, removals);
        assert!(f.data.exists() && f.app.exists());
    }

    #[test]
    fn missing_removal_counts_as_removed() {
        let f = fixture("ownership-missing");
        let ghost = f.data.join("no-existe");
        let planned = plan(&f.tree, std::slice::from_ref(&ghost));
        let report = execute(&f.tree, &planned);
        assert_eq!(report.removed, vec![ghost]);
        assert!(report.failed.is_empty());
    }

    #[test]
    fn external_root_is_never_removed_whole_nor_pruned() {
        let root = scratch("ownership-external");
        let hub = root.join("hub");
        let repo = hub.join("models--x--y");
        write_file(&repo.join("f"), "f");
        let tree = vec![node(hub.clone(), NodeKind::External)];

        let whole = plan(&tree, std::slice::from_ref(&hub));
        let report = execute(&tree, &whole);
        assert_eq!(report.failed.len(), 1, "la raíz entera se rechaza");
        assert!(hub.exists());

        let partial = plan(&tree, std::slice::from_ref(&repo));
        assert!(partial.prune.is_empty());
        let report = execute(&tree, &partial);
        assert!(report.failed.is_empty() && report.pruned.is_empty());
        assert!(!repo.exists());
        assert!(hub.exists(), "la raíz externa queda vacía pero en pie");
    }

    #[test]
    fn common_parent_above_the_tree_is_never_touched() {
        let f = fixture("ownership-parent");
        let parent = f.app.parent().expect("padre común").to_path_buf();
        let removals = vec![f.data.clone(), f.models.clone(), f.tmp.clone()];
        execute(&f.tree, &plan(&f.tree, &removals));
        assert!(!f.app.exists());
        assert!(parent.exists(), "el padre no es nodo y no se poda");
    }

    #[test]
    fn plan_does_not_modify_the_disk() {
        let f = fixture("ownership-dry");
        let removals = vec![f.data.clone(), f.models.clone(), f.tmp.clone()];
        let _ = plan(&f.tree, &removals);
        assert!(f.data.exists() && f.models.exists() && f.tmp.exists());
    }
}
