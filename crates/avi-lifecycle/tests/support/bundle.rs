// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! Bundle sintético y staging del sandbox. Lo declaran con
//! `#[path = "support/bundle.rs"] mod bundle;`.

use crate::support::{write, Sandbox};
use std::path::{Path, PathBuf};

/// Une un fragmento del manifiesto con la raíz del bundle.
pub fn place(dest: &Path, relative: &str) -> PathBuf {
    let mut path = dest.to_path_buf();
    for part in relative.split('/') {
        path.push(part);
    }
    path
}

impl Sandbox {
    /// Staging hermano del directorio de programa, con el token de este proceso y la
    /// etiqueta del sandbox: lo crea un proceso vivo, como el de una descarga en curso.
    pub fn staging(&self) -> PathBuf {
        let owner = avi_process::current_owner_token().expect("este proceso se puede observar");
        self.program_dir
            .parent()
            .expect("el directorio de programa tiene padre")
            .join(avi_shared::staging_names::install_staging::name(
                &format!("test-{}", self.tag),
                &owner,
            ))
    }

    /// Escribe un bundle sintético **completo** en `dest`, con los nombres exactos
    /// que `packaging/bundle-manifest.json` exige para el target del host, y devuelve
    /// el ejecutable. La lista no se escribe a mano: si el manifiesto cambiara, la
    /// seguiría la validación del paso 2 de la instalación.
    pub fn write_bundle(&self, dest: &Path) -> PathBuf {
        let section = avi_lifecycle::manifest::target_section(avi_lifecycle::target::host_triple())
            .expect("el target del host tiene sección en el manifiesto");
        for relative in &section.required {
            write(
                &place(dest, relative),
                &format!("contenido de {relative}\n"),
            );
        }
        dest.join(section.executable_path())
    }
}
