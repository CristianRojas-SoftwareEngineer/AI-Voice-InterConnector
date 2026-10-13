// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! Nombres de la caché de modelos. Lo declaran con
//! `#[path = "support/repo.rs"] mod repo;`.

/// Nombre del directorio de repo en la caché de HuggingFace, tal y como lo construye
/// `avi-store`.
pub fn repo_dir(repo: &str) -> String {
    format!("models--{}", repo.replace('/', "--"))
}
