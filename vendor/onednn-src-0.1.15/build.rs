// build.rs
//
// Copyright (c) 2025 Junpei Kawamoto
//
// This software is released under the MIT License.
//
// http://opensource.org/licenses/mit-license.php

use cmake::Config;
use std::env;

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-env-changed=DOCS_RS");
    if env::var("DOCS_RS").is_ok() {
        return;
    }

    println!("cargo::rerun-if-changed=avi-link-runtimes.cmake");
    let contract_script = format!(
        "{}/avi-link-runtimes.cmake",
        env::var("CARGO_MANIFEST_DIR")
            .expect("cargo define CARGO_MANIFEST_DIR")
            .replace('\\', "/")
    );

    let cmake = Config::new("oneDNN")
        .define("CMAKE_PROJECT_INCLUDE", contract_script)
        .define("ONEDNN_LIBRARY_TYPE", "STATIC")
        .define("ONEDNN_BUILD_DOC", "OFF")
        .define("ONEDNN_BUILD_EXAMPLES", "OFF")
        .define("ONEDNN_BUILD_TESTS", "OFF")
        .define(
            "ONEDNN_BUILD_GRAPH",
            if cfg!(feature = "graph") { "ON" } else { "OFF" },
        )
        .define(
            "ONEDNN_ENABLE_WORKLOAD",
            if cfg!(feature = "inference") {
                "INFERENCE"
            } else {
                "TRAINING"
            },
        )
        .define(
            "ONEDNN_EXPERIMENTAL",
            if cfg!(feature = "experimental") {
                "ON"
            } else {
                "OFF"
            },
        )
        .define(
            "ONEDNN_VERBOSE",
            if cfg!(feature = "verbose") {
                "ON"
            } else {
                "OFF"
            },
        )
        .build();
    println!("cargo::rustc-link-search={}/lib", cmake.display());
    println!("cargo::rustc-link-lib=static=dnnl");
    link_runtimes(&cmake.join("build"));
    println!("cargo::metadata=ROOT={}", cmake.display());
    println!(
        "cargo::metadata=INCLUDE_PATH={}",
        cmake.join("include").display()
    );
    println!(
        "cargo::metadata=LIBRARY_PATH={}",
        cmake.join("lib").display()
    );
}

// Patch: el original no pedía los runtimes que usa `dnnl` y dependía de que
// otro crate los enlazara, pero rustc coloca cada petición justo detrás del
// crate que la emite y `dnnl` va detrás de los demás crates C++: con runtimes
// estáticos quedaban símbolos sin resolver. Lo que `dnnl` necesita lo declara
// el script `avi-link-runtimes.cmake`, inyectado con `CMAKE_PROJECT_INCLUDE`,
// que al final de la configuración escribe en `build_dir` el contrato
// `avi-link-runtimes.txt` con dos líneas: `cxx=` (la librería estándar de C++
// que enlaza su compilador, `stdc++` entre ellas) y `openmp=` (el runtime
// OpenMP que encontró, `gomp` entre ellos). Cómo se enlaza cada una lo decide
// el proyecto con `CXXSTDLIB_<target>` y `GOMPLIB_<target>` (el triple con `_`
// en lugar de `-`), cuyo valor se emite tal cual, igual que hace el crate `cc`
// con `CXXSTDLIB_<target>`. Si falta el contrato, una de sus líneas o la
// variable, la compilación falla nombrándolo.
fn link_runtimes(build_dir: &std::path::Path) {
    let path = build_dir.join("avi-link-runtimes.txt");
    let contract = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("no se pudo leer el contrato {}: {e}", path.display()));
    if contract_list(&contract, "cxx=", &path).contains(&"stdc++") {
        link_as_configured("CXXSTDLIB");
    }
    if contract_list(&contract, "openmp=", &path).contains(&"gomp") {
        link_as_configured("GOMPLIB");
    }
}

/// Elementos de la lista de CMake (separada por `;`) de la línea `key` del
/// contrato; falla si la línea no existe.
fn contract_list<'a>(contract: &'a str, key: &str, path: &std::path::Path) -> Vec<&'a str> {
    let line = contract
        .lines()
        .find_map(|line| line.strip_prefix(key))
        .unwrap_or_else(|| panic!("el contrato {} no tiene la línea `{key}`", path.display()));
    line.split(';').filter(|s| !s.is_empty()).collect()
}

/// Emite como petición de enlace el valor de `<prefix>_<target>`.
fn link_as_configured(prefix: &str) {
    let target = env::var("TARGET").expect("cargo define TARGET");
    let var = format!("{prefix}_{}", target.replace('-', "_"));
    println!("cargo::rerun-if-env-changed={var}");
    let request = env::var(&var)
        .unwrap_or_else(|_| panic!("{var} no está definida: indica cómo enlazar la librería que necesita oneDNN"));
    println!("cargo::rustc-link-lib={request}");
}
