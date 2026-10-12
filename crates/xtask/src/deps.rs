//! Auditoría de dependencias del bundle: ningún ejecutable o librería que se
//! publica puede importar algo que el sistema del usuario no trae.
//!
//! Cada target del manifiesto declara su contrato: `system_libs` (librerías del
//! sistema base que se pueden importar; una entrada que termina en `/` o en `*`
//! admite todo lo que empieza por ese prefijo), `symbol_ceilings` (versión
//! máxima de cada familia de símbolos versionados de ELF, como `GLIBC_2.28`) y
//! `min_os` (versión mínima de macOS que deben declarar los binarios). Además,
//! un binario puede importar cualquier otra pieza del propio bundle. Los
//! binarios se leen con `object`, igual en local que en la CI, y la auditoría
//! reporta todas las violaciones juntas.

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{bail, Context, Result};
use avi_shared::manifest::{parse_manifest_text, BundleTarget};
use object::read::elf::{Dyn, ElfFile64};
use object::read::macho::{LoadCommandVariant, MachOFile64};
use object::read::pe::PeFile64;
use object::{elf, Endianness, FileKind};

/// Lo que un binario declara necesitar del sistema.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Imports {
    /// Librerías importadas: `DT_NEEDED` en ELF, `LC_LOAD_DYLIB` y afines en
    /// Mach-O, tabla de importación en PE.
    pub libs: Vec<String>,
    /// Versiones de símbolo requeridas (`GLIBC_2.17`); solo ELF.
    pub symbol_versions: Vec<String>,
    /// Versión mínima de macOS declarada (`26.0`); solo Mach-O.
    pub min_os: Option<String>,
}

/// Lee las importaciones de un binario. `Ok(None)` si los bytes no son un
/// ejecutable ni una librería de 64 bits de un formato auditado (documentos,
/// scripts...).
pub(crate) fn read_imports(data: &[u8]) -> Result<Option<Imports>> {
    let Ok(kind) = FileKind::parse(data) else {
        return Ok(None);
    };
    match kind {
        FileKind::Elf64 => elf_imports(data).map(Some),
        FileKind::MachO64 => macho_imports(data).map(Some),
        FileKind::Pe64 => pe_imports(data).map(Some),
        _ => Ok(None),
    }
}

/// ELF: `DT_NEEDED` de la sección dinámica y versiones de `SHT_GNU_VERNEED`.
fn elf_imports(data: &[u8]) -> Result<Imports> {
    let file = ElfFile64::<Endianness>::parse(data)?;
    let endian = file.endian();
    let sections = file.elf_section_table();
    let mut imports = Imports::default();
    if let Some((entries, link)) = sections.dynamic(endian, data)? {
        let strings = sections.strings(endian, data, link)?;
        for entry in entries {
            if entry.tag(endian) == elf::DT_NEEDED {
                let name = entry.string(endian, strings)?;
                imports
                    .libs
                    .push(String::from_utf8_lossy(name).into_owned());
            }
        }
    }
    if let Some((mut needs, link)) = sections.gnu_verneed(endian, data)? {
        let strings = sections.strings(endian, data, link)?;
        while let Some((_need, mut auxes)) = needs.next()? {
            while let Some(aux) = auxes.next()? {
                let name = aux.name(endian, strings)?;
                imports
                    .symbol_versions
                    .push(String::from_utf8_lossy(name).into_owned());
            }
        }
    }
    Ok(imports)
}

/// Mach-O: librerías de `LC_LOAD_DYLIB` (y sus variantes débil, reexportada y
/// ascendente) y `minos` de `LC_BUILD_VERSION`.
fn macho_imports(data: &[u8]) -> Result<Imports> {
    let file = MachOFile64::<Endianness>::parse(data)?;
    let endian = file.endian();
    let mut imports = Imports::default();
    let mut commands = file.macho_load_commands()?;
    while let Some(command) = commands.next()? {
        match command.variant()? {
            LoadCommandVariant::Dylib(dylib) => {
                let name = command.string(endian, dylib.dylib.name)?;
                imports
                    .libs
                    .push(String::from_utf8_lossy(name).into_owned());
            }
            LoadCommandVariant::BuildVersion(build, _) => {
                imports.min_os = Some(decode_macho_version(build.minos.get(endian).0));
            }
            _ => {}
        }
    }
    Ok(imports)
}

/// Versión de Mach-O codificada como `xxxx.yy.zz` en nibbles: `0x001A0000` es
/// `26.0`. El parche solo se escribe si no es cero.
fn decode_macho_version(raw: u32) -> String {
    let (major, minor, patch) = (raw >> 16, (raw >> 8) & 0xff, raw & 0xff);
    if patch == 0 {
        format!("{major}.{minor}")
    } else {
        format!("{major}.{minor}.{patch}")
    }
}

/// PE: nombres de las DLL de la tabla de importación.
fn pe_imports(data: &[u8]) -> Result<Imports> {
    let file = PeFile64::parse(data)?;
    let mut imports = Imports::default();
    if let Some(table) = file.import_table()? {
        let mut descriptors = table.descriptors()?;
        while let Some(descriptor) = descriptors.next()? {
            let name = table.name(descriptor.name.get(object::LittleEndian))?;
            imports
                .libs
                .push(String::from_utf8_lossy(name).into_owned());
        }
    }
    Ok(imports)
}

/// Último componente de una ruta con `/` (o el nombre entero si no la tiene).
fn base_name(lib: &str) -> &str {
    lib.rsplit('/').next().unwrap_or(lib)
}

/// `lib` está permitida por una entrada de `system_libs`: igual sin distinguir
/// mayúsculas, o con el prefijo de una entrada que termina en `/` o en `*`.
fn allowed_by_system(lib: &str, system_libs: &[String]) -> bool {
    let lib = lib.to_ascii_lowercase();
    system_libs.iter().any(|entry| {
        let entry = entry.to_ascii_lowercase();
        if let Some(prefix) = entry.strip_suffix('*') {
            lib.starts_with(prefix)
        } else if entry.ends_with('/') {
            lib.starts_with(&entry)
        } else {
            lib == entry
        }
    })
}

/// Componentes numéricos de una versión con puntos (`3.4.21`); `None` si alguno
/// no es un número.
fn parse_version(text: &str) -> Option<Vec<u64>> {
    text.split('.').map(|part| part.parse().ok()).collect()
}

/// `a > b` comparando componente a componente y rellenando con ceros.
fn version_greater(a: &[u64], b: &[u64]) -> bool {
    let len = a.len().max(b.len());
    for i in 0..len {
        let (x, y) = (
            a.get(i).copied().unwrap_or(0),
            b.get(i).copied().unwrap_or(0),
        );
        if x != y {
            return x > y;
        }
    }
    false
}

/// Violaciones del contrato de `section` en el binario `file` (ruta relativa
/// al bundle, para el mensaje). `bundled` son los nombres de fichero del
/// bundle, en minúsculas: importar una pieza del propio bundle está permitido.
pub(crate) fn violations(
    file: &str,
    imports: &Imports,
    bundled: &BTreeSet<String>,
    section: &BundleTarget,
) -> Vec<String> {
    let mut found = Vec::new();
    for lib in &imports.libs {
        let in_bundle = bundled.contains(&base_name(lib).to_ascii_lowercase());
        if !in_bundle && !allowed_by_system(lib, &section.system_libs) {
            found.push(format!(
                "{file}: importa {lib}, que no es del sistema base ni del bundle"
            ));
        }
    }
    for required in &imports.symbol_versions {
        let Some((family, version)) = required.split_once('_') else {
            continue;
        };
        let Some(ceiling) = section
            .symbol_ceilings
            .iter()
            .find_map(|c| c.split_once('_').filter(|(f, _)| *f == family))
        else {
            continue;
        };
        let exceeds = match (parse_version(version), parse_version(ceiling.1)) {
            (Some(req), Some(max)) => version_greater(&req, &max),
            _ => true,
        };
        if exceeds {
            found.push(format!(
                "{file}: exige {required}, por encima del techo {}_{}",
                ceiling.0, ceiling.1
            ));
        }
    }
    if let Some(min_os) = &section.min_os {
        match &imports.min_os {
            None => found.push(format!(
                "{file}: no declara versión mínima de macOS (se exige {min_os} o menor)"
            )),
            Some(declared) => {
                let exceeds = match (parse_version(declared), parse_version(min_os)) {
                    (Some(d), Some(m)) => version_greater(&d, &m),
                    _ => true,
                };
                if exceeds {
                    found.push(format!(
                        "{file}: exige macOS {declared}, por encima del mínimo {min_os}"
                    ));
                }
            }
        }
    }
    found
}

/// Ficheros regulares bajo `dir`, como rutas relativas con `/`.
fn collect_files(dir: &Path, base: &Path, out: &mut Vec<String>) -> Result<()> {
    for entry in
        std::fs::read_dir(dir).with_context(|| format!("no se pudo listar {}", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        let kind = entry.file_type()?;
        if kind.is_dir() {
            collect_files(&path, base, out)?;
        } else if kind.is_file() {
            let relative = path.strip_prefix(base)?;
            let parts: Vec<String> = relative
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            out.push(parts.join("/"));
        }
    }
    Ok(())
}

/// Audita el árbol `dir` contra el contrato de `section` y devuelve todas las
/// violaciones, ordenadas.
pub(crate) fn audit_tree(dir: &Path, section: &BundleTarget) -> Result<Vec<String>> {
    let mut files = Vec::new();
    collect_files(dir, dir, &mut files)?;
    files.sort();
    let bundled: BTreeSet<String> = files
        .iter()
        .map(|f| base_name(f).to_ascii_lowercase())
        .collect();
    let mut found = Vec::new();
    for file in &files {
        let data =
            std::fs::read(dir.join(file)).with_context(|| format!("no se pudo leer {file}"))?;
        let imports = read_imports(&data).with_context(|| format!("no se pudo analizar {file}"))?;
        if let Some(imports) = imports {
            found.extend(violations(file, &imports, &bundled, section));
        }
    }
    Ok(found)
}

/// Falla con todas las violaciones juntas si el árbol no cumple el contrato.
pub(crate) fn check_tree(dir: &Path, section: &BundleTarget) -> Result<()> {
    let found = audit_tree(dir, section)?;
    if found.is_empty() {
        println!(
            "dependencias en contrato: {} cumple el sistema base de {}",
            dir.display(),
            section.triple
        );
        return Ok(());
    }
    bail!(
        "el bundle depende de algo fuera del contrato de {}:\n  {}",
        section.triple,
        found.join("\n  ")
    )
}

/// Punto de entrada de `xtask deps --check --dir <ruta>`: audita un árbol ya
/// montado o extraído con el contrato del target del host.
pub(crate) fn run(check: bool, dir: &Path) -> Result<()> {
    if !check {
        bail!("usa `cargo xtask deps --check --dir <ruta>`");
    }
    let root = std::env::current_dir()?;
    let text = std::fs::read_to_string(root.join(super::package::MANIFEST_REL))
        .context("ejecuta `cargo xtask deps` desde la raíz del repositorio")?;
    let sections = parse_manifest_text(&text).map_err(|e| anyhow::anyhow!("{e}"))?;
    let triple = super::package::host_triple()?;
    let section = sections.get(triple).ok_or_else(|| {
        anyhow::anyhow!("el manifiesto de bundle no tiene sección para el target {triple}")
    })?;
    check_tree(dir, section)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Contrato de fixture con los campos que cada prueba necesita.
    fn section(system_libs: &[&str], ceilings: &[&str], min_os: Option<&str>) -> BundleTarget {
        BundleTarget {
            triple: "fixture".to_string(),
            executable: "bin".to_string(),
            required: vec!["bin".to_string()],
            system_libs: system_libs.iter().map(|s| (*s).to_string()).collect(),
            symbol_ceilings: ceilings.iter().map(|s| (*s).to_string()).collect(),
            min_os: min_os.map(str::to_string),
        }
    }

    /// Conjunto de ficheros del bundle en minúsculas, como lo construye `audit_tree`.
    fn bundled(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|n| (*n).to_ascii_lowercase()).collect()
    }

    /// Importaciones de fixture con solo librerías.
    fn libs(names: &[&str]) -> Imports {
        Imports {
            libs: names.iter().map(|n| (*n).to_string()).collect(),
            symbol_versions: Vec::new(),
            min_os: None,
        }
    }

    /// Importaciones de fixture con solo versiones de símbolo.
    fn symbols(names: &[&str]) -> Imports {
        Imports {
            libs: Vec::new(),
            symbol_versions: names.iter().map(|n| (*n).to_string()).collect(),
            min_os: None,
        }
    }

    /// Una librería fuera del sistema base y del bundle es una violación que
    /// nombra el fichero y la librería.
    #[test]
    fn unknown_lib_reports_file_and_lib() {
        let section = section(&[], &[], None);
        let found = violations("bin/malo", &libs(&["libfoo.so"]), &bundled(&[]), &section);
        assert_eq!(found.len(), 1);
        assert!(found[0].contains("bin/malo"));
        assert!(found[0].contains("libfoo.so"));
    }

    /// Una librería que es otro fichero del bundle no es violación.
    #[test]
    fn bundled_lib_is_allowed() {
        let section = section(&[], &[], None);
        let found = violations(
            "bin",
            &libs(&["libonnxruntime.so"]),
            &bundled(&["bin", "libonnxruntime.so"]),
            &section,
        );
        assert!(found.is_empty());
    }

    /// Una librería del sistema base no es violación, sin distinguir
    /// mayúsculas.
    #[test]
    fn system_lib_case_insensitive() {
        let section = section(&["kernel32.dll"], &[], None);
        let found = violations("a.exe", &libs(&["KERNEL32.dll"]), &bundled(&[]), &section);
        assert!(found.is_empty());
    }

    /// Las entradas con `/` o `*` final admiten el prefijo; otra raíz no.
    #[test]
    fn prefix_entries_cover_framework_and_api_set() {
        let section = section(
            &["/System/Library/Frameworks/", "api-ms-win-*", "/usr/lib/"],
            &[],
            None,
        );
        let ok = libs(&[
            "/System/Library/Frameworks/CoreAudio.framework/Versions/A/CoreAudio",
            "api-ms-win-crt-runtime-l1-1-0.dll",
        ]);
        assert!(violations("f", &ok, &bundled(&[]), &section).is_empty());
        let bad = libs(&["/opt/homebrew/lib/libomp.dylib"]);
        let found = violations("f", &bad, &bundled(&[]), &section);
        assert_eq!(found.len(), 1);
    }

    /// Con techo `GLIBC_2.28`, lo igual o menor pasa y lo mayor viola.
    #[test]
    fn glibc_ceiling() {
        let section = section(&[], &["GLIBC_2.28"], None);
        assert!(violations("f", &symbols(&["GLIBC_2.17"]), &bundled(&[]), &section).is_empty());
        assert!(violations("f", &symbols(&["GLIBC_2.28"]), &bundled(&[]), &section).is_empty());
        let found = violations("f", &symbols(&["GLIBC_2.29"]), &bundled(&[]), &section);
        assert_eq!(found.len(), 1);
        assert!(found[0].contains("GLIBC_2.29"));
    }

    /// La comparación de techos es numérica, no de texto (`3.4.9` < `3.4.21`).
    #[test]
    fn glibcxx_ceiling_compares_numerically() {
        let section = section(&[], &["GLIBCXX_3.4.21"], None);
        assert!(violations("f", &symbols(&["GLIBCXX_3.4.9"]), &bundled(&[]), &section).is_empty());
        assert_eq!(
            violations("f", &symbols(&["GLIBCXX_3.4.22"]), &bundled(&[]), &section).len(),
            1
        );
    }

    /// Sin techo para la familia no hay violación; lo no numérico con techo sí.
    #[test]
    fn family_without_ceiling_is_allowed() {
        let section = section(&[], &["GLIBC_2.28"], None);
        assert!(violations(
            "f",
            &symbols(&["GCC_3.0", "CXXABI_1.3.9"]),
            &bundled(&[]),
            &section
        )
        .is_empty());
        assert_eq!(
            violations("f", &symbols(&["GLIBC_PRIVATE"]), &bundled(&[]), &section).len(),
            1
        );
    }

    /// Con `min_os` `26.0`, lo igual o menor pasa; lo mayor o ausente viola.
    #[test]
    fn min_os_ceiling() {
        let section = section(&[], &[], Some("26.0"));
        for declared in ["14.0", "26.0"] {
            let imports = Imports {
                min_os: Some(declared.to_string()),
                ..Imports::default()
            };
            assert!(
                violations("f", &imports, &bundled(&[]), &section).is_empty(),
                "{declared} debe pasar"
            );
        }
        let imports = Imports {
            min_os: Some("26.1".to_string()),
            ..Imports::default()
        };
        assert_eq!(violations("f", &imports, &bundled(&[]), &section).len(), 1);
        assert_eq!(
            violations("f", &Imports::default(), &bundled(&[]), &section).len(),
            1
        );
    }

    /// El decodificador de versiones de Mach-O escribe el parche solo si no es
    /// cero.
    #[test]
    fn decodes_macho_version() {
        assert_eq!(decode_macho_version(0x001A_0000), "26.0");
        assert_eq!(decode_macho_version(0x000E_0201), "14.2.1");
    }

    /// Los bytes de un fichero de texto no son un binario auditado.
    #[test]
    fn text_bytes_are_not_a_binary() {
        let text = include_str!("../Cargo.toml");
        assert!(read_imports(text.as_bytes()).unwrap().is_none());
    }

    /// El ejecutable de la propia prueba se lee con el lector real del host y
    /// declara al menos una librería.
    #[test]
    fn current_exe_imports_at_least_one_lib() {
        let exe = std::env::current_exe().unwrap();
        let data = std::fs::read(&exe).unwrap();
        let imports = read_imports(&data)
            .unwrap()
            .expect("el ejecutable es auditable");
        assert!(!imports.libs.is_empty());
        if cfg!(windows) {
            assert!(
                imports
                    .libs
                    .iter()
                    .any(|l| l.to_ascii_lowercase().ends_with(".dll")),
                "en Windows alguna importación termina en .dll"
            );
        }
    }

    /// El manifiesto real declara el contrato de los cuatro targets.
    #[test]
    fn real_manifest_declares_contract() {
        let text = include_str!("../../../packaging/bundle-manifest.json");
        let sections = parse_manifest_text(text).expect("el manifiesto es válido");
        assert_eq!(sections.len(), 4);
        for section in sections.values() {
            assert!(!section.system_libs.is_empty(), "{}", section.triple);
        }
        for triple in ["x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu"] {
            assert_eq!(
                sections[triple].symbol_ceilings,
                ["GLIBC_2.28", "GLIBCXX_3.4.21"]
            );
            assert_eq!(sections[triple].min_os, None);
        }
        assert_eq!(
            sections["aarch64-apple-darwin"].min_os.as_deref(),
            Some("26.0")
        );
        assert_eq!(sections["x86_64-pc-windows-msvc"].min_os, None);
    }
}
