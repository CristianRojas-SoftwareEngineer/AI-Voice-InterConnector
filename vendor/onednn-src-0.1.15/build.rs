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
    // Patch: en Linux, oneDNN necesita la librería estándar de C++ y, con su
    // runtime de CPU por defecto (OpenMP), el de GNU, pero el original no los
    // pedía y dependía de que otro crate los enlazara. rustc coloca cada
    // petición justo detrás del crate que la emite, y `dnnl` va detrás de los
    // demás crates C++: pedidas aquí como `static:-bundle`, `libstdc++.a` y
    // `libgomp.a` quedan detrás de `dnnl` y resuelven todo lo que usa. Sin
    // ellas el enlace falla en vez de caer en las dinámicas.
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        println!("cargo::rustc-link-lib=static:-bundle=stdc++");
        println!("cargo::rustc-link-lib=static:-bundle=gomp");
    }
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
