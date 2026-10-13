// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

// Expone a la biblioteca el triple con el que se compila este binario.
//
// Cargo pone `TARGET` en el entorno de los scripts de compilación y es la
// única fuente exacta del triple: `std::env::consts` da arquitectura y sistema,
// pero ni el vendedor (`pc`, `unknown`, `apple`) ni la variante del target, y los
// targets soportados se nombran por triple completo. `AVI_TARGET_TRIPLE` queda
// estampado en el código con `env!` en `target::host_triple`.
fn main() {
    let triple = std::env::var("TARGET").expect("Cargo expone TARGET al script de compilación");
    println!("cargo:rustc-env=AVI_TARGET_TRIPLE={triple}");
    println!("cargo:rerun-if-changed=build.rs");
}
