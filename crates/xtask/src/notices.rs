// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! Generador de la región de avisos de `THIRD-PARTY-LICENSES.md`.
//!
//! Reproduce los textos de licencia de todo lo que viaja en el paquete: los crates de
//! Cargo distribuidos (leídos de sus archivos `LICENSE*`, `COPYING*` y `NOTICE*` o de
//! `packaging/notices/crates/`) y los componentes que no vienen de Cargo (tabla
//! [`COMPONENTS`]). Los textos idénticos se agrupan por cuerpo, sin las líneas de
//! copyright, y cada grupo lista sus titulares y quién lo usa.
//!
//! La expresión SPDX de cada crate decide qué textos hacen falta: `AND` los exige todos,
//! `OR` elige una alternativa por el orden MIT, Apache-2.0 y el resto, y `WITH` pide la
//! licencia base. Un crate sin el texto de alguna licencia que necesita es un error.

use crate::DistPackage;
use anyhow::{anyhow, bail, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// Marcas de la región de avisos dentro de `THIRD-PARTY-LICENSES.md`.
pub(crate) const NOTICES_START: &str = "<!-- avisos:inicio -->";
pub(crate) const NOTICES_END: &str = "<!-- avisos:fin -->";

// ---------------------------------------------------------------------------------------
// Expresiones SPDX
// ---------------------------------------------------------------------------------------

/// Una licencia SPDX con su excepción opcional (`WITH`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct License {
    pub(crate) id: String,
    pub(crate) exception: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Expr {
    License(License),
    And(Vec<Expr>),
    Or(Vec<Expr>),
}

/// Separa la expresión en palabras y paréntesis; la sintaxis antigua `/` equivale a `OR`.
fn tokenize(expression: &str) -> Vec<String> {
    fn flush(current: &mut String, tokens: &mut Vec<String>) {
        if !current.is_empty() {
            tokens.push(std::mem::take(current));
        }
    }
    let mut tokens = Vec::new();
    let mut current = String::new();
    for c in expression.chars() {
        match c {
            '(' | ')' => {
                flush(&mut current, &mut tokens);
                tokens.push(c.to_string());
            }
            '/' => {
                flush(&mut current, &mut tokens);
                tokens.push("OR".to_string());
            }
            c if c.is_whitespace() => flush(&mut current, &mut tokens),
            c => current.push(c),
        }
    }
    flush(&mut current, &mut tokens);
    tokens
}

struct Parser {
    tokens: Vec<String>,
    position: usize,
}

impl Parser {
    fn peek(&self) -> Option<&str> {
        self.tokens.get(self.position).map(String::as_str)
    }

    fn next(&mut self) -> Option<String> {
        let token = self.tokens.get(self.position).cloned();
        self.position += 1;
        token
    }

    /// Precedencia SPDX: `WITH` liga más que `AND`, y `AND` más que `OR`.
    fn parse_or(&mut self) -> Result<Expr> {
        let mut items = vec![self.parse_and()?];
        while self.peek() == Some("OR") {
            self.next();
            items.push(self.parse_and()?);
        }
        Ok(if items.len() == 1 {
            items.remove(0)
        } else {
            Expr::Or(items)
        })
    }

    fn parse_and(&mut self) -> Result<Expr> {
        let mut items = vec![self.parse_with()?];
        while self.peek() == Some("AND") {
            self.next();
            items.push(self.parse_with()?);
        }
        Ok(if items.len() == 1 {
            items.remove(0)
        } else {
            Expr::And(items)
        })
    }

    fn parse_with(&mut self) -> Result<Expr> {
        let primary = self.parse_primary()?;
        if self.peek() != Some("WITH") {
            return Ok(primary);
        }
        self.next();
        let exception = self
            .next()
            .ok_or_else(|| anyhow!("falta la excepción después de WITH"))?;
        match primary {
            Expr::License(license) => Ok(Expr::License(License {
                id: license.id,
                exception: Some(exception),
            })),
            _ => bail!("WITH solo se aplica a una licencia"),
        }
    }

    fn parse_primary(&mut self) -> Result<Expr> {
        match self.next().as_deref() {
            Some("(") => {
                let inner = self.parse_or()?;
                match self.next().as_deref() {
                    Some(")") => Ok(inner),
                    _ => bail!("falta el paréntesis de cierre"),
                }
            }
            Some(")" | "AND" | "OR" | "WITH") | None => bail!("se esperaba una licencia"),
            Some(id) => Ok(Expr::License(License {
                id: id.to_string(),
                exception: None,
            })),
        }
    }
}

fn parse(expression: &str) -> Result<Expr> {
    let mut parser = Parser {
        tokens: tokenize(expression),
        position: 0,
    };
    let expr = parser
        .parse_or()
        .map_err(|e| anyhow!("expresión de licencia `{expression}`: {e}"))?;
    if parser.position < parser.tokens.len() {
        bail!("expresión de licencia `{expression}`: sobran símbolos después de la expresión");
    }
    Ok(expr)
}

/// Orden de preferencia en un `OR`: MIT, luego Apache-2.0 y después el resto.
fn preference(license: &License) -> u8 {
    match license.id.as_str() {
        "MIT" => 0,
        "Apache-2.0" => 1,
        _ => 2,
    }
}

fn resolve(expr: &Expr) -> Vec<License> {
    match expr {
        Expr::License(license) => vec![license.clone()],
        Expr::And(items) => {
            let mut all = Vec::new();
            for item in items {
                for license in resolve(item) {
                    if !all.contains(&license) {
                        all.push(license);
                    }
                }
            }
            all
        }
        // En un empate gana la primera alternativa; una alternativa compuesta vale lo que
        // su licencia menos preferida.
        Expr::Or(items) => items
            .iter()
            .map(resolve)
            .min_by_key(|licenses| licenses.iter().map(preference).max().unwrap_or(2))
            .unwrap_or_default(),
    }
}

/// Licencias cuyo texto hay que reproducir para una expresión SPDX.
pub(crate) fn required_licenses(expression: &str) -> Result<Vec<License>> {
    Ok(resolve(&parse(expression)?))
}

// ---------------------------------------------------------------------------------------
// Textos de los crates
// ---------------------------------------------------------------------------------------

/// Un archivo de licencia leído, con su nombre.
struct SourceText {
    name: String,
    content: String,
}

fn is_license_file_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    ["license", "licence", "copying", "notice", "unlicense"]
        .iter()
        .any(|prefix| lower.starts_with(prefix))
}

fn is_notice_file_name(name: &str) -> bool {
    name.to_lowercase().starts_with("notice")
}

/// Archivos de licencia del directorio del crate más, si existe, su copia versionada en
/// `packaging/notices/crates/<nombre>-<versión>.txt`.
fn crate_texts(root: &Path, package: &DistPackage) -> Result<Vec<SourceText>> {
    let mut names = Vec::new();
    let entries = std::fs::read_dir(&package.manifest_dir)
        .map_err(|e| anyhow!("no se pudo leer {}: {}", package.manifest_dir.display(), e))?;
    for entry in entries {
        let entry = entry.map_err(|e| {
            anyhow!(
                "no se pudo leer una entrada de {}: {}",
                package.manifest_dir.display(),
                e
            )
        })?;
        let name = entry.file_name().to_string_lossy().to_string();
        if entry.path().is_file() && is_license_file_name(&name) {
            names.push(name);
        }
    }
    names.sort();
    let mut texts = Vec::new();
    for name in names {
        let path = package.manifest_dir.join(&name);
        let content = std::fs::read_to_string(&path)
            .map_err(|e| anyhow!("no se pudo leer {}: {}", path.display(), e))?;
        texts.push(SourceText { name, content });
    }
    let copy = root
        .join("packaging")
        .join("notices")
        .join("crates")
        .join(format!("{}-{}.txt", package.name, package.version));
    if copy.is_file() {
        let content = std::fs::read_to_string(&copy)
            .map_err(|e| anyhow!("no se pudo leer {}: {}", copy.display(), e))?;
        texts.push(SourceText {
            name: format!("{}-{}.txt", package.name, package.version),
            content,
        });
    }
    Ok(texts)
}

/// Texto en minúsculas y con los espacios colapsados, para comparar sin formato.
fn collapse(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Si el texto contiene la licencia SPDX indicada, reconocida por una frase característica.
fn text_matches(license_id: &str, text: &str) -> Result<bool> {
    let normalized = collapse(text);
    let has_all = |phrases: &[&str]| phrases.iter().all(|phrase| normalized.contains(phrase));
    Ok(match license_id {
        "MIT" => has_all(&[
            "permission is hereby granted, free of charge, to any person obtaining a copy",
        ]),
        "Apache-2.0" => has_all(&["apache license", "version 2.0, january 2004"]),
        "BSD-3-Clause" => has_all(&[
            "redistribution and use in source and binary forms",
            "neither the name",
        ]),
        "BSD-2-Clause" => has_all(&[
            "redistribution and use in source and binary forms",
            "redistributions in binary form must reproduce",
        ]),
        "ISC" => has_all(&["permission to use, copy, modify", "with or without fee"]),
        "Zlib" => has_all(&["altered source versions must be plainly marked"]),
        "MPL-2.0" => has_all(&["mozilla public license", "version 2.0"]),
        "Unicode-3.0" => has_all(&["unicode license v3"]),
        "CDLA-Permissive-2.0" => has_all(&["community data license agreement"]),
        other => {
            bail!("la licencia `{other}` no tiene frase de reconocimiento: añádela a text_matches")
        }
    })
}

/// Un texto elegido para un crate con las licencias requeridas que contiene.
struct Chosen<'a> {
    text: &'a SourceText,
    licenses: BTreeSet<String>,
}

/// Textos que un crate debe reproducir: los que contienen alguna licencia requerida por
/// su expresión SPDX y todos sus `NOTICE`. Falla si no hay ningún archivo de licencia o si
/// alguna licencia requerida no aparece en ninguno.
fn choose_texts<'a>(
    package: &DistPackage,
    texts: &'a [SourceText],
    required: &[License],
) -> Result<Vec<Chosen<'a>>> {
    let label = format!("{} {}", package.name, package.version);
    if texts.is_empty() {
        bail!(
            "{label} no tiene archivo de licencia ni copia en packaging/notices/crates/: \
             añade el texto upstream como `{}-{}.txt`",
            package.name,
            package.version
        );
    }
    let mut chosen = Vec::new();
    let mut covered: BTreeSet<&str> = BTreeSet::new();
    for text in texts {
        let mut licenses = BTreeSet::new();
        for license in required {
            if text_matches(&license.id, &text.content)? {
                licenses.insert(license.id.clone());
                covered.insert(license.id.as_str());
            }
        }
        if !licenses.is_empty() || is_notice_file_name(&text.name) {
            chosen.push(Chosen { text, licenses });
        }
    }
    for license in required {
        if !covered.contains(license.id.as_str()) {
            bail!(
                "{label} declara {} pero ninguno de sus archivos de licencia contiene ese texto: \
                 añade el texto upstream como `{}-{}.txt` en packaging/notices/crates/",
                license.id,
                package.name,
                package.version
            );
        }
    }
    Ok(chosen)
}

// ---------------------------------------------------------------------------------------
// Componentes que no vienen de Cargo
// ---------------------------------------------------------------------------------------

const WINDOWS: &str = "x86_64-pc-windows-msvc";
const LINUX_X64: &str = "x86_64-unknown-linux-gnu";
const LINUX_ARM64: &str = "aarch64-unknown-linux-gnu";
const MACOS_ARM64: &str = "aarch64-apple-darwin";
const ALL_TARGETS: &[&str] = &[WINDOWS, LINUX_X64, LINUX_ARM64, MACOS_ARM64];

/// De dónde sale un texto de un componente.
enum TextSource {
    /// Un archivo del repositorio; `{ort}` se sustituye por la versión fijada de ONNX Runtime.
    File(&'static str),
    /// El comentario de licencia que empieza en `begin` dentro de un archivo fuente.
    CommentBlock {
        file: &'static str,
        begin: &'static str,
    },
}

struct ComponentText {
    /// Licencia que declara el texto, para el encabezado de su grupo.
    license: &'static str,
    source: TextSource,
}

/// Un componente del paquete que no viene de Cargo.
struct Component {
    name: &'static str,
    /// Triples del manifiesto del paquete en los que viaja.
    platforms: &'static [&'static str],
    /// Expresión SPDX informativa; se interpreta en las pruebas.
    spdx: &'static str,
    texts: &'static [ComponentText],
    /// Entradas de `required` del manifiesto del paquete que este componente atribuye.
    covers: &'static [&'static str],
    /// Qué es y cómo se relaciona con el resto, en una frase.
    note: &'static str,
}

const GCC_EXCEPTION: ComponentText = ComponentText {
    license: "GCC-exception-3.1",
    source: TextSource::File("packaging/notices/gcc-exception-3.1.txt"),
};

const QWEN_BINARIES: &[&str] = &["vendor/qwen3-tts/qwen_tts.exe", "vendor/qwen3-tts/qwen_tts"];
const MAIN_BINARIES: &[&str] = &["ai-voice-interconnector.exe", "ai-voice-interconnector"];

/// Atribución de cada archivo no documental del paquete. Una prueba exige que toda entrada
/// de `required` del manifiesto del paquete aparezca en `covers` de algún componente de su
/// plataforma.
const COMPONENTS: &[Component] = &[
    Component {
        name: "AI Voice-InterConnector",
        platforms: ALL_TARGETS,
        spdx: "MPL-2.0",
        texts: &[ComponentText {
            license: "MPL-2.0",
            source: TextSource::File("LICENSE"),
        }],
        covers: MAIN_BINARIES,
        note: "El ejecutable enlaza los crates de la sección de inventario.",
    },
    Component {
        name: "Runtime de GCC enlazado en el ejecutable (libgomp y libstdc++)",
        platforms: &[LINUX_X64, LINUX_ARM64],
        spdx: "GPL-3.0-or-later WITH GCC-exception-3.1",
        texts: &[GCC_EXCEPTION],
        covers: &["ai-voice-interconnector"],
        note: "Enlazado estáticamente desde el gcc-toolset de la imagen de build.",
    },
    Component {
        name: "ONNX Runtime",
        platforms: ALL_TARGETS,
        spdx: "MIT",
        texts: &[
            ComponentText {
                license: "MIT",
                source: TextSource::File("packaging/notices/onnxruntime-{ort}/LICENSE"),
            },
            ComponentText {
                license: "ThirdPartyNotices de ONNX Runtime",
                source: TextSource::File("packaging/notices/onnxruntime-{ort}/ThirdPartyNotices.txt"),
            },
        ],
        covers: &["onnxruntime.dll", "libonnxruntime.so", "libonnxruntime.dylib"],
        note: "Librería oficial de Microsoft, con el aviso de terceros que ella misma publica.",
    },
    Component {
        name: "Runtime de Visual C++",
        platforms: &[WINDOWS],
        spdx: "LicenseRef-Microsoft-VC-Redistributable",
        texts: &[],
        covers: &[
            "vcruntime140.dll",
            "vcruntime140_1.dll",
            "msvcp140.dll",
            "msvcp140_1.dll",
            "vcomp140.dll",
        ],
        note: "Archivos redistribuibles del Visual C++ Redistributable, que su licencia permite desplegar junto a la aplicación.",
    },
    Component {
        name: "FFmpeg",
        platforms: ALL_TARGETS,
        spdx: "GPL-3.0-or-later",
        texts: &[],
        covers: &["ffmpeg.exe", "ffmpeg"],
        note: "Ejecutable independiente que la aplicación invoca como proceso; sus fuentes se ofrecen en SOURCE-OFFER.md.",
    },
    Component {
        name: "Motor TTS qwen_tts (código propio y vendorizado)",
        platforms: ALL_TARGETS,
        spdx: "MIT AND BSD-2-Clause",
        texts: &[
            ComponentText {
                license: "MIT",
                source: TextSource::File("vendor/qwen3-tts/LICENSE"),
            },
            ComponentText {
                license: "MIT",
                source: TextSource::File("vendor/qwen3-tts/third_party/ingot/LICENSE"),
            },
            ComponentText {
                license: "MIT",
                source: TextSource::File("packaging/notices/llama.cpp/LICENSE"),
            },
            ComponentText {
                license: "BSD-2-Clause",
                source: TextSource::CommentBlock {
                    file: "vendor/qwen3-tts/vendor/lz4.c",
                    begin: "LZ4 - Fast LZ compression algorithm",
                },
            },
        ],
        covers: QWEN_BINARIES,
        note: "Motor del proyecto qwen3-tts; las tablas de cuantización de ingot proceden de llama.cpp y LZ4 va en `vendor/lz4.c`.",
    },
    Component {
        name: "OpenBLAS enlazada en qwen_tts",
        platforms: &[WINDOWS, LINUX_X64, LINUX_ARM64],
        spdx: "BSD-3-Clause",
        texts: &[ComponentText {
            license: "BSD-3-Clause",
            source: TextSource::File("packaging/notices/openblas/LICENSE"),
        }],
        covers: QWEN_BINARIES,
        note: "Enlazada estáticamente; en macOS el motor usa Accelerate.",
    },
    Component {
        name: "Runtime de GCC enlazado en qwen_tts (libgfortran y libgcc)",
        platforms: &[WINDOWS, LINUX_X64, LINUX_ARM64],
        spdx: "GPL-3.0-or-later WITH GCC-exception-3.1",
        texts: &[GCC_EXCEPTION],
        covers: QWEN_BINARIES,
        note: "Runtime que usa el LAPACK de OpenBLAS.",
    },
    Component {
        name: "libquadmath enlazada en qwen_tts",
        platforms: &[LINUX_X64],
        spdx: "LGPL-2.1-or-later",
        texts: &[ComponentText {
            license: "LGPL-2.1",
            source: TextSource::File("packaging/notices/lgpl-2.1.txt"),
        }],
        covers: &["vendor/qwen3-tts/qwen_tts"],
        note: "Para reenlazar el motor con otra versión de libquadmath, reconstruye el motor con `cargo xtask build-engine` a partir de `vendor/qwen3-tts/`, cuyas fuentes ofrece SOURCE-OFFER.md.",
    },
    Component {
        name: "winpthreads enlazada en qwen_tts.exe",
        platforms: &[WINDOWS],
        spdx: "MIT AND BSD-3-Clause",
        texts: &[ComponentText {
            license: "MIT y BSD-3-Clause",
            source: TextSource::File("packaging/notices/winpthreads/COPYING"),
        }],
        covers: &["vendor/qwen3-tts/qwen_tts.exe"],
        note: "Runtime de hilos POSIX de MinGW-w64.",
    },
    Component {
        name: "CTranslate2 y oneDNN (fuentes incluidas en crates vendorizados)",
        platforms: ALL_TARGETS,
        spdx: "MIT AND Apache-2.0",
        texts: &[
            ComponentText {
                license: "MIT",
                source: TextSource::File("vendor/ct2rs-0.10.0/CTranslate2/LICENSE"),
            },
            ComponentText {
                license: "Apache-2.0",
                source: TextSource::File("vendor/onednn-src-0.1.15/oneDNN/LICENSE"),
            },
            ComponentText {
                license: "THIRD-PARTY-PROGRAMS de oneDNN",
                source: TextSource::File("vendor/onednn-src-0.1.15/oneDNN/THIRD-PARTY-PROGRAMS"),
            },
        ],
        covers: MAIN_BINARIES,
        note: "Enlazadas estáticamente en el ejecutable a través de `ct2rs` y `onednn-src`.",
    },
];

fn read_component_text(root: &Path, text: &ComponentText, ort: &str) -> Result<String> {
    match &text.source {
        TextSource::File(path) => {
            let path = root.join(path.replace("{ort}", ort));
            std::fs::read_to_string(&path)
                .map_err(|e| anyhow!("no se pudo leer {}: {}", path.display(), e))
        }
        TextSource::CommentBlock { file, begin } => {
            let path = root.join(file);
            let source = std::fs::read_to_string(&path)
                .map_err(|e| anyhow!("no se pudo leer {}: {}", path.display(), e))?;
            comment_block(&source, begin).map_err(|e| anyhow!("{}: {}", path.display(), e))
        }
    }
}

/// Contenido del comentario `/* ... */` que contiene `begin`, sin sangría común.
fn comment_block(source: &str, begin: &str) -> Result<String> {
    let at = source
        .find(begin)
        .ok_or_else(|| anyhow!("no contiene `{begin}`"))?;
    let open = source[..at]
        .rfind("/*")
        .ok_or_else(|| anyhow!("`{begin}` no está dentro de un comentario"))?;
    let close = source[at..]
        .find("*/")
        .map(|i| at + i)
        .ok_or_else(|| anyhow!("el comentario de `{begin}` no se cierra"))?;
    let inner = &source[open + 2..close];
    let indent = inner
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.len() - line.trim_start().len())
        .min()
        .unwrap_or(0);
    Ok(inner
        .lines()
        .map(|line| line.get(indent..).unwrap_or("").trim_end())
        .collect::<Vec<_>>()
        .join("\n"))
}

// ---------------------------------------------------------------------------------------
// Agrupación y render
// ---------------------------------------------------------------------------------------

/// Una línea que declara titulares de copyright (no un modelo con marcadores a rellenar).
fn is_copyright_line(trimmed: &str) -> bool {
    let lower = trimmed.to_lowercase();
    let declares = lower.starts_with("copyright")
        || lower.starts_with("(c)")
        || lower.starts_with("portions copyright")
        || lower.starts_with('©');
    let placeholder = [
        "yyyy",
        "<year>",
        "name of author",
        "name of copyright owner",
    ]
    .iter()
    .any(|marker| lower.contains(marker));
    declares && !placeholder
}

/// Separa un texto en su cuerpo, sin líneas de copyright ni espacios sobrantes, y sus
/// titulares.
fn split_notice(text: &str) -> (String, Vec<String>) {
    let mut holders = Vec::new();
    let mut body = String::new();
    let mut pending_blank = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if is_copyright_line(trimmed) {
            holders.push(trimmed.to_string());
            continue;
        }
        if trimmed.is_empty() {
            pending_blank = true;
            continue;
        }
        if !body.is_empty() {
            body.push('\n');
            if pending_blank {
                body.push('\n');
            }
        }
        pending_blank = false;
        body.push_str(line.trim_end());
    }
    (body, holders)
}

/// Valla de código más larga que cualquier racha de comillas invertidas del texto, para
/// que un texto que contenga vallas no cierre el bloque antes de tiempo.
fn fence_for(body: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for c in body.chars() {
        if c == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    "`".repeat(longest.max(2) + 1)
}

/// Un cuerpo de licencia distinto con sus titulares y quienes lo usan.
struct Group {
    body: String,
    licenses: BTreeSet<String>,
    holders: BTreeSet<String>,
    users: BTreeSet<String>,
}

#[derive(Default)]
struct Groups {
    by_body: BTreeMap<String, Group>,
}

impl Groups {
    fn add(&mut self, text: &str, licenses: &BTreeSet<String>, user: &str) {
        let (body, holders) = split_notice(text);
        let key = body.split_whitespace().collect::<Vec<_>>().join(" ");
        let group = self.by_body.entry(key).or_insert_with(|| Group {
            body,
            licenses: BTreeSet::new(),
            holders: BTreeSet::new(),
            users: BTreeSet::new(),
        });
        group.licenses.extend(licenses.iter().cloned());
        group.holders.extend(holders);
        group.users.insert(user.to_string());
    }
}

pub(crate) fn platform_name(triple: &str) -> &str {
    match triple {
        WINDOWS => "Windows x64",
        LINUX_X64 => "Linux x64",
        LINUX_ARM64 => "Linux ARM64",
        MACOS_ARM64 => "macOS ARM64",
        other => other,
    }
}

fn render_components_table() -> String {
    let mut table = String::from(
        "| Componente | Plataformas | Licencia | Archivos del paquete | Nota |\n\
         |------------|-------------|----------|----------------------|------|\n",
    );
    for component in COMPONENTS {
        let platforms = if component.platforms == ALL_TARGETS {
            "todas".to_string()
        } else {
            component
                .platforms
                .iter()
                .map(|triple| platform_name(triple))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let covers = component
            .covers
            .iter()
            .map(|file| format!("`{file}`"))
            .collect::<Vec<_>>()
            .join(", ");
        table.push_str(&format!(
            "| {} | {} | {} | {} | {} |\n",
            component.name, platforms, component.spdx, covers, component.note
        ));
    }
    table
}

/// Región de avisos: tabla de componentes que no vienen de Cargo y textos de licencia
/// agrupados por cuerpo, de los crates distribuidos y de esos componentes.
pub(crate) fn render_notices(root: &Path, packages: &[DistPackage], ort: &str) -> Result<String> {
    let mut groups = Groups::default();
    for package in packages {
        let expression = package.license.as_deref().ok_or_else(|| {
            anyhow!(
                "{} {} no declara `license` en su Cargo.toml: no hay licencia que registrar",
                package.name,
                package.version
            )
        })?;
        let required = required_licenses(expression)
            .map_err(|e| anyhow!("{} {}: {}", package.name, package.version, e))?;
        let texts = crate_texts(root, package)?;
        let user = format!("`{}` {}", package.name, package.version);
        for chosen in choose_texts(package, &texts, &required)? {
            let labels = if chosen.licenses.is_empty() {
                BTreeSet::from(["NOTICE".to_string()])
            } else {
                chosen.licenses.clone()
            };
            groups.add(&chosen.text.content, &labels, &user);
        }
    }
    for component in COMPONENTS {
        for text in component.texts {
            let content = read_component_text(root, text, ort)?;
            groups.add(
                &content,
                &BTreeSet::from([text.license.to_string()]),
                component.name,
            );
        }
    }

    let mut ordered: Vec<&Group> = groups.by_body.values().collect();
    ordered.sort_by(|a, b| {
        (&a.licenses, a.users.iter().next()).cmp(&(&b.licenses, b.users.iter().next()))
    });
    let mut out = format!(
        "Generado desde los archivos de licencia de los crates distribuidos y de los componentes \
         de la tabla siguiente: {} textos distintos para {} crates y {} componentes.\n\n\
         ### Componentes que no vienen de Cargo\n\n{}\n",
        ordered.len(),
        packages.len(),
        COMPONENTS.len(),
        render_components_table()
    );
    for (index, group) in ordered.iter().enumerate() {
        let fence = fence_for(&group.body);
        out.push_str(&format!(
            "### Texto {}: {}\n\n",
            index + 1,
            group
                .licenses
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        ));
        if !group.holders.is_empty() {
            out.push_str("Titulares:\n\n");
            for holder in &group.holders {
                out.push_str(&format!("- {holder}\n"));
            }
            out.push('\n');
        }
        out.push_str(&format!(
            "Aplica a: {}.\n\n{fence}text\n{}\n{fence}\n\n",
            group.users.iter().cloned().collect::<Vec<_>>().join(", "),
            group.body
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn ids(expression: &str) -> Vec<String> {
        required_licenses(expression)
            .unwrap()
            .into_iter()
            .map(|license| license.id)
            .collect()
    }

    #[test]
    fn and_requires_every_license() {
        assert_eq!(ids("MIT AND BSD-3-Clause"), ["MIT", "BSD-3-Clause"]);
        assert_eq!(ids("Apache-2.0 AND ISC"), ["Apache-2.0", "ISC"]);
    }

    #[test]
    fn or_prefers_mit_then_apache_then_the_rest() {
        assert_eq!(ids("Apache-2.0 OR MIT"), ["MIT"]);
        assert_eq!(ids("Zlib OR Apache-2.0"), ["Apache-2.0"]);
        assert_eq!(ids("Zlib OR ISC"), ["Zlib"]);
        assert_eq!(ids("Unlicense OR MIT OR Apache-2.0 OR CC0-1.0"), ["MIT"]);
        assert_eq!(
            ids("CC0-1.0 OR Apache-2.0 OR Apache-2.0 WITH LLVM-exception"),
            ["Apache-2.0"]
        );
    }

    #[test]
    fn the_legacy_slash_is_an_or() {
        assert_eq!(ids("MIT/Apache-2.0"), ["MIT"]);
        assert_eq!(ids("Apache-2.0 / MIT"), ["MIT"]);
        assert_eq!(ids("Unlicense/MIT"), ["MIT"]);
    }

    #[test]
    fn parentheses_group_before_and() {
        assert_eq!(
            ids("(Apache-2.0 OR MIT) AND BSD-3-Clause"),
            ["MIT", "BSD-3-Clause"]
        );
        assert_eq!(ids("ISC AND (Apache-2.0 OR ISC)"), ["ISC", "Apache-2.0"]);
    }

    #[test]
    fn with_keeps_the_base_license_and_its_exception() {
        let licenses = required_licenses("GPL-3.0-or-later WITH GCC-exception-3.1").unwrap();
        assert_eq!(licenses.len(), 1);
        assert_eq!(licenses[0].id, "GPL-3.0-or-later");
        assert_eq!(licenses[0].exception.as_deref(), Some("GCC-exception-3.1"));
    }

    #[test]
    fn malformed_expressions_fail() {
        for bad in ["", "MIT AND", "(MIT", "MIT OR OR Zlib", "MIT)", "WITH MIT"] {
            assert!(required_licenses(bad).is_err(), "`{bad}` debería fallar");
        }
    }

    fn package_in(dir: &Path, license: &str) -> DistPackage {
        DistPackage {
            name: "demo".to_string(),
            version: "1.0.0".to_string(),
            license: Some(license.to_string()),
            manifest_dir: dir.to_path_buf(),
        }
    }

    const MIT_TEXT: &str = "MIT License\n\nCopyright (c) 2020 Ana\n\nPermission is hereby granted, free of charge, to any person obtaining a copy\nof this software.\n";

    #[test]
    fn a_crate_without_license_text_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\n").unwrap();
        let root = tempfile::tempdir().unwrap();
        let package = package_in(dir.path(), "MIT");
        let err = render_notices(root.path(), &[package], "1.0.0").unwrap_err();
        assert!(err.to_string().contains("demo 1.0.0"), "{err}");
    }

    #[test]
    fn a_versioned_copy_covers_a_crate_without_license_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let copies = root.path().join("packaging/notices/crates");
        std::fs::create_dir_all(&copies).unwrap();
        std::fs::write(copies.join("demo-1.0.0.txt"), MIT_TEXT).unwrap();
        let package = package_in(dir.path(), "MIT");
        let texts = crate_texts(root.path(), &package).unwrap();
        let required = required_licenses("MIT").unwrap();
        assert_eq!(choose_texts(&package, &texts, &required).unwrap().len(), 1);
    }

    #[test]
    fn a_required_license_missing_from_the_texts_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("LICENSE"), MIT_TEXT).unwrap();
        let package = package_in(dir.path(), "MIT AND BSD-3-Clause");
        let texts = crate_texts(Path::new("."), &package).unwrap();
        let required = required_licenses("MIT AND BSD-3-Clause").unwrap();
        let err = choose_texts(&package, &texts, &required)
            .err()
            .expect("falta el texto BSD-3-Clause");
        assert!(err.to_string().contains("BSD-3-Clause"), "{err}");
    }

    #[test]
    fn a_dual_licensed_crate_reproduces_only_the_preferred_text() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("LICENSE-MIT"), MIT_TEXT).unwrap();
        std::fs::write(
            dir.path().join("LICENSE-APACHE"),
            "Apache License\nVersion 2.0, January 2004\n",
        )
        .unwrap();
        let package = package_in(dir.path(), "Apache-2.0 OR MIT");
        let texts = crate_texts(Path::new("."), &package).unwrap();
        let required = required_licenses("Apache-2.0 OR MIT").unwrap();
        let chosen = choose_texts(&package, &texts, &required).unwrap();
        assert_eq!(chosen.len(), 1);
        assert_eq!(chosen[0].text.name, "LICENSE-MIT");
    }

    #[test]
    fn notice_files_are_always_reproduced() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("LICENSE"),
            "Apache License\nVersion 2.0, January 2004\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("NOTICE"), "Este producto incluye X.\n").unwrap();
        let package = package_in(dir.path(), "Apache-2.0");
        let texts = crate_texts(Path::new("."), &package).unwrap();
        let required = required_licenses("Apache-2.0").unwrap();
        assert_eq!(choose_texts(&package, &texts, &required).unwrap().len(), 2);
    }

    #[test]
    fn identical_texts_group_into_one_body_with_every_holder() {
        let mut groups = Groups::default();
        let mit = BTreeSet::from(["MIT".to_string()]);
        groups.add(MIT_TEXT, &mit, "`a` 1.0.0");
        groups.add(
            &MIT_TEXT.replace("Copyright (c) 2020 Ana", "Copyright (c) 2021 Beto"),
            &mit,
            "`b` 2.0.0",
        );
        groups.add("Otra licencia distinta\n", &mit, "`c` 3.0.0");
        assert_eq!(groups.by_body.len(), 2);
        let shared = groups
            .by_body
            .values()
            .find(|group| group.users.len() == 2)
            .expect("un grupo compartido");
        assert_eq!(shared.holders.len(), 2);
        assert!(!shared.body.contains("Copyright"));
    }

    #[test]
    fn copyright_templates_are_not_holders() {
        let (body, holders) = split_notice("Copyright [yyyy] [name of copyright owner]\nTexto\n");
        assert!(holders.is_empty());
        assert!(body.contains("Copyright [yyyy]"));
    }

    #[test]
    fn the_fence_is_longer_than_any_backtick_run_in_the_text() {
        assert_eq!(fence_for("sin vallas"), "```");
        assert_eq!(fence_for("uno ` y ``` dentro"), "````");
    }

    #[test]
    fn a_comment_block_is_extracted_without_indentation() {
        let source =
            "int x;\n/*\n   LZ4 - Fast\n   Copyright (C) 2011, Yann\n\n     * item\n*/\nint y;\n";
        let block = comment_block(source, "LZ4 - Fast").unwrap();
        assert!(block.contains("LZ4 - Fast\nCopyright (C) 2011, Yann"));
        assert!(block.contains("  * item"));
        assert!(!block.contains("int"));
    }

    fn repo_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
    }

    #[test]
    fn every_component_expression_parses() {
        for component in COMPONENTS {
            assert!(
                required_licenses(component.spdx).is_ok(),
                "{}: `{}` no es una expresión SPDX válida",
                component.name,
                component.spdx
            );
        }
    }

    /// Toda entrada no documental de `required` de cada target tiene un componente que la
    /// atribuye en esa plataforma.
    #[test]
    fn every_non_document_package_file_has_a_component() {
        let text = std::fs::read_to_string(repo_root().join(crate::package::MANIFEST_REL)).unwrap();
        let targets = avi_shared::manifest::parse_manifest_text(&text).unwrap();
        let documents = [
            "LICENSE",
            "NOTICE",
            "THIRD-PARTY-LICENSES.md",
            "SOURCE-OFFER.md",
            "README.md",
        ];
        for (triple, target) in &targets {
            for file in &target.required {
                if documents.contains(&file.as_str()) {
                    continue;
                }
                assert!(
                    COMPONENTS.iter().any(|component| {
                        component.platforms.contains(&triple.as_str())
                            && component.covers.contains(&file.as_str())
                    }),
                    "`{file}` de {triple} no tiene componente en COMPONENTS"
                );
            }
        }
    }

    #[test]
    fn every_component_platform_and_file_exists_in_the_manifest() {
        let text = std::fs::read_to_string(repo_root().join(crate::package::MANIFEST_REL)).unwrap();
        let targets = avi_shared::manifest::parse_manifest_text(&text).unwrap();
        for component in COMPONENTS {
            for triple in component.platforms {
                assert!(
                    targets.contains_key(*triple),
                    "{}: `{triple}` no es un target del manifiesto",
                    component.name
                );
            }
            for file in component.covers {
                assert!(
                    component.platforms.iter().any(|triple| targets[*triple]
                        .required
                        .iter()
                        .any(|required| required == file)),
                    "{}: `{file}` no figura en `required` de ninguna de sus plataformas",
                    component.name
                );
            }
        }
    }

    #[test]
    fn the_onnx_runtime_notices_match_the_pinned_version() {
        let pins = avi_shared::pins::load_from_root(&repo_root()).unwrap();
        let directory = repo_root()
            .join("packaging")
            .join("notices")
            .join(format!("onnxruntime-{}", pins.ort));
        assert!(
            directory.join("LICENSE").is_file()
                && directory.join("ThirdPartyNotices.txt").is_file(),
            "faltan los avisos de ONNX Runtime {} en {}",
            pins.ort,
            directory.display()
        );
    }
}
