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

    let cmake = Config::new("oneDNN")
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
// estáticos quedaban símbolos sin resolver. Lo que `dnnl` necesita sale de su
// propia configuración de CMake en `build_dir`: la librería estándar de C++
// que enlaza su compilador (`stdc++` en `CMAKE_CXX_IMPLICIT_LINK_LIBRARIES`) y
// el runtime OpenMP que encontró (`gomp` en `OpenMP_CXX_LIB_NAMES`). Cómo se
// enlaza cada una lo decide el proyecto con `CXXSTDLIB_<target>` y
// `GOMPLIB_<target>` (el triple con `_` en lugar de `-`), cuyo valor se emite
// tal cual, igual que hace el crate `cc` con `CXXSTDLIB_<target>`. Si la
// variable falta, la compilación falla nombrándola.
fn link_runtimes(build_dir: &std::path::Path) {
    let cache = std::fs::read_to_string(build_dir.join("CMakeCache.txt"))
        .expect("CMake deja CMakeCache.txt en el directorio de build");
    let compiler = std::fs::read_dir(build_dir.join("CMakeFiles"))
        .expect("CMake deja CMakeFiles en el directorio de build")
        .filter_map(|entry| {
            std::fs::read_to_string(entry.ok()?.path().join("CMakeCXXCompiler.cmake")).ok()
        })
        .next()
        .expect("CMake deja CMakeCXXCompiler.cmake en CMakeFiles/<versión>");
    let cxx_libs = cmake_list(&compiler, "set(CMAKE_CXX_IMPLICIT_LINK_LIBRARIES \"", "\")");
    if cxx_libs.contains(&"stdc++") {
        link_as_configured("CXXSTDLIB");
    }
    let openmp_libs = cmake_list(&cache, "OpenMP_CXX_LIB_NAMES:STRING=", "\n");
    if openmp_libs.contains(&"gomp") {
        link_as_configured("GOMPLIB");
    }
}

/// Elementos de la lista de CMake (separada por `;`) que sigue a `prefix` hasta
/// `end`; vacía si `prefix` no aparece.
fn cmake_list<'a>(text: &'a str, prefix: &str, end: &str) -> Vec<&'a str> {
    text.split_once(prefix)
        .and_then(|(_, rest)| rest.split(end).next())
        .map(|list| list.split(';').filter(|s| !s.is_empty()).collect())
        .unwrap_or_default()
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
