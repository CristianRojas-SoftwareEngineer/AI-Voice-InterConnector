//! Integración de `PATH` en Unix: enlace simbólico y bloque delimitado de
//! perfiles, por decisión cerrada D2.
//!
//! D2 es lo que hace este módulo necesario donde el instalador heredado no
//! tocaba nada: **en Unix el `PATH` persistente se modifica por defecto**, se
//! anuncia en el resumen y se revierte exactamente al desinstalar.
//! `--no-modify-path` lo desactiva, y entonces el comando se invoca por su ruta
//! completa.
//!
//! **El bloque de perfil se edita en cualquier plataforma.** El texto que se
//! escribe es texto, y la lógica de "no lo escribas dos veces" y de "reviértelo
//! byte a byte" es lo que hay que afirmar; dejar la edición entera tras
//! `#[cfg(unix)]` la volvería imposible de probar desde la puerta de Windows,
//! que es donde se compila y se ejecuta el lote. Solo el enlace simbólico es de
//! Unix, y esa parte sí está acotada.

use crate::LifecycleError;
use std::path::{Path, PathBuf};

/// Marca de apertura del bloque delimitado.
pub const BLOCK_BEGIN: &str = "# >>> ai-voice-interconnector >>>";
/// Marca de cierre del bloque delimitado.
pub const BLOCK_END: &str = "# <<< ai-voice-interconnector <<<";

/// Cuerpo del bloque, con la línea exacta que publica la
/// especificación.
///
/// `bin_dir` se interpola como `$HOME/.local/bin` cuando es la ruta convencional,
/// que es lo que hace portable el bloque entre máquinas del mismo usuario, y como
/// ruta absoluta cuando `AVI_BIN_DIR` la reubica. El resultado es byte a byte el
/// bloque de la especificación en el caso normal.
/// Forma en la que el bloque escribe la entrada del `PATH`: `$HOME/...`
/// cuando el directorio del enlace es `$HOME/.local/bin`, y la ruta absoluta en el
/// resto de los casos.
///
/// **Es la misma función que usa [`block_text`]**, y tiene que serlo: `needs_block`
/// decide si hace falta el bloque comparando contra lo que el bloque escribiría, y con
/// dos formas distintas la comparación falla justo en el caso que quiere cubrir —un
/// `PATH` que ya trae `$HOME/.local/bin`— y el bloque se duplica.
fn block_entry(bin_dir: &Path, home: &Path) -> String {
    match bin_dir.strip_prefix(home) {
        Ok(relative) if relative == Path::new(".local/bin") => "$HOME/.local/bin".to_string(),
        _ => bin_dir.display().to_string(),
    }
}

pub fn block_text(bin_dir: &Path, home: &Path) -> String {
    let entry = block_entry(bin_dir, home);
    format!(
        "{BLOCK_BEGIN}\ncase \":${{PATH}}:\" in *\":{entry}:\"*) ;; *) export \
         PATH=\"{entry}:$PATH\" ;; esac\n{BLOCK_END}"
    )
}

/// Los shells que la tabla nombra, en la forma en que se detectan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shell {
    Sh,
    Bash,
    Zsh,
    Fish,
}

impl Shell {
    /// Lee el shell del usuario de `$SHELL`, que es de lo que la tabla dice "el de
    /// `$SHELL`". Desconocido se trata como `sh`, que es el caso más restrictivo
    /// y del que los demás heredan.
    pub fn from_env(shell: Option<&str>) -> Self {
        let name = shell.and_then(|path| path.rsplit('/').next());
        match name {
            Some("zsh") => Self::Zsh,
            Some("fish") => Self::Fish,
            Some("bash") => Self::Bash,
            _ => Self::Sh,
        }
    }
}

/// Archivos de arranque a los que hay que añadir el bloque, según el shell
/// detectado y los que ya tengan archivo.
///
/// `home`, `zdotdir` y `exists` son parámetros —como la clave de registro en la
/// integración del `PATH`— para que la prueba no dependa del `HOME` de quien la
/// ejecuta ni de si esa máquina tiene bash. El orden es el de la tabla: `~/.profile`
/// siempre, `~/.bashrc` solo si existe, el `.zshrc` del shell de `$SHELL` y el
/// archivo propio de fish.
pub fn profile_targets<F>(
    home: &Path,
    zdotdir: Option<&Path>,
    shell: Shell,
    exists: F,
) -> Vec<PathBuf>
where
    F: Fn(&Path) -> bool,
{
    let mut out = vec![home.join(".profile")];
    let bashrc = home.join(".bashrc");
    if exists(&bashrc) {
        out.push(bashrc);
    }
    match shell {
        Shell::Zsh => out.push(zdotdir.unwrap_or(home).join(".zshrc")),
        Shell::Fish => out.push(home.join(".config/fish/conf.d/ai-voice-interconnector.fish")),
        Shell::Sh | Shell::Bash => {}
    }
    out
}

/// ¿Hace falta el bloque? Solo si el directorio del enlace no está ya en el
/// `PATH` de la sesión.
///
/// Se aceptan las dos formas de la misma entrada: la expandida y la que el bloque
/// escribe sin expandir (`$HOME/.local/bin`), porque un `PATH` con la segunda no
/// reconocería a la primera con una comparación literal y acabaría con un bloque
/// redundante.
///
/// El troceado es el de `split_paths`, que usa `:` en Unix y `;` en Windows, en vez
/// de `split(':')` a pelo: en Windows la letra de la unidad es una `C:`, y trocear
/// por `:` rompería cada entrada por la mitad.
pub fn needs_block(path_env: &str, bin_dir: &Path, home: &Path) -> bool {
    // Las dos formas admitidas: la que escribe el bloque (`$HOME/.local/bin` cuando el
    // directorio del enlace está bajo `$HOME`) y la expandida, que es lo que queda en el
    // `PATH` de una sesión donde alguien ya la expandió a mano.
    let block_form = block_entry(bin_dir, home);
    let literal_form = format!("{}/.local/bin", home.display());
    !std::env::split_paths(path_env)
        .filter(|entry| !entry.as_os_str().is_empty())
        .any(|entry| {
            let raw = entry.to_string_lossy().to_string();
            raw == block_form
                || raw == literal_form
                || crate::canonical_path_entry_matches(&entry, bin_dir)
        })
}

/// Añade el bloque a `path` de forma idempotente. Devuelve `true` si el archivo
/// cambió.
///
/// La idempotencia no es "comprobar si el texto está": es comprobar si el
/// **sufijo** que se añadiría ya está, porque un usuario puede tener su propio
/// bloque con los mismos delimitadores y ese no es el nuestro.
pub fn write_block(path: &Path, bin_dir: &Path, home: &Path) -> std::io::Result<bool> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    let block = block_text(bin_dir, home);
    if content.ends_with(&block) {
        return Ok(false);
    }
    let new = append(&content, &block);
    write(path, &new)?;
    Ok(true)
}

/// Quita el bloque de `path` y devuelve `true` si el archivo cambió.
///
/// **Solo trunca si el final del archivo es exactamente lo que se habría añadido**, y
/// se quita también el separador que lo acompaña. Un perfil en el que el usuario ha
/// editado dentro del bloque no se toca: preferimos dejar la integración a medias —que
/// el recibo permite revertir y que `doctor` puede señalar— antes que borrar
/// contenido suyo.
pub fn remove_block(path: &Path, bin_dir: &Path, home: &Path) -> std::io::Result<bool> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    let block = block_text(bin_dir, home);
    // `append` pone un separador antes del bloque salvo cuando el archivo estaba
    // vacío, así que el sufijo a quitar depende de si queda algo delante. Los dos
    // casos son excluyentes, y por eso basta con probar el largo primero.
    let with_separator = format!("\n{block}");
    let before = match content.strip_suffix(&with_separator) {
        Some(before) if !before.is_empty() => before,
        _ => match content.strip_suffix(&block) {
            Some(before) => before,
            None => return Ok(false),
        },
    };
    write(path, before)?;
    Ok(true)
}

/// Une contenido y bloque. El separador es **siempre** un salto de línea, y el bloque
/// no lo lleva al final.
///
/// Es lo que hace que `remove_block` pueda devolver el archivo byte a byte como
/// estaba: si el separador dependiera de si el contenido terminaba en salto de línea,
/// la reversión no podría saber cuál de los dos había que devolver. El coste es una
/// línea en blanco extra cuando el perfil ya acababa en salto de línea, que es
/// cosmetics y no de contrato.
fn append(content: &str, block: &str) -> String {
    if content.is_empty() {
        block.to_string()
    } else {
        format!("{content}\n{block}")
    }
}

fn write(path: &Path, content: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, content)
}

/// Qué hay en la ruta del enlace. La clasificación va aparte de la
/// creación para que la regla —"solo es conflicto lo que no es enlace nuestro"—
/// se pueda afirmar sin crear enlaces, que en Windows y dentro de contenedores
/// exigen privilegios que las pruebas no tienen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Existing {
    /// No hay nada en la ruta.
    Absent,
    /// Es un enlace simbólico que apunta al ejecutable del directorio de programa.
    Ours(PathBuf),
    /// Hay algo que no es un enlace nuestro: `path_conflict`.
    Foreign,
}

/// Regla como función pura, para que el `reason` y su código se
/// afirmación en cualquier plataforma.
pub fn decide_existing(
    existing: &Existing,
    link: &Path,
    force: bool,
) -> Result<(), LifecycleError> {
    match existing {
        Existing::Ours(_) => Ok(()),
        Existing::Foreign if !force => Err(LifecycleError::new(
            "path_conflict",
            format!(
                "ya hay algo en {} que no es el enlace de {}: quítalo o repite la \
                 operación con `--force`",
                link.display(),
                crate::APP_NAME
            ),
        )),
        Existing::Foreign | Existing::Absent => Ok(()),
    }
}

/// Clasifica lo que hay en `link` sin tocarlo.
#[cfg(unix)]
pub fn classify_existing(link: &Path, program_exe: &Path) -> Existing {
    match std::fs::read_link(link) {
        Ok(dest) => {
            // Se comparan las dos formas del destino: un enlace puede llevar la
            // ruta absoluta o la relativa al directorio del enlace, y ninguna de
            // las dos es culpa de quien lo creó.
            let absolute = if dest.is_absolute() {
                dest
            } else {
                link.parent().unwrap_or(Path::new(".")).join(dest)
            };
            if crate::canonical_path_entry_matches(&absolute, program_exe) {
                Existing::Ours(absolute)
            } else {
                Existing::Foreign
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if link.symlink_metadata().is_ok() {
                Existing::Foreign
            } else {
                Existing::Absent
            }
        }
        // `read_link` sobre algo que no es un enlace da `InvalidInput` en Unix y
        // también puede dar `PermissionDenied`: en los dos casos hay algo ahí que
        // no es un enlace nuestro, que es exactamente lo que la regla llama
        // conflicto.
        Err(_) => Existing::Foreign,
    }
}

/// Fuera de Unix la clasificación solo puede afirmar una cosa: si la ruta está
/// ocupada, lo que hay no es un enlace nuestro. La creación de enlaces no tiene
/// equivalente portable, y por eso `create_symlink` no existe fuera de Unix.
#[cfg(not(unix))]
pub fn classify_existing(link: &Path, _program_exe: &Path) -> Existing {
    if link.symlink_metadata().is_ok() {
        Existing::Foreign
    } else {
        Existing::Absent
    }
}

/// Crea el enlace simbólico de forma atómica: enlace temporal hermano y
/// renombrado. Devuelve `path_conflict` si en la ruta hay algo que no sea
/// un enlace propio, salvo `force`.
#[cfg(unix)]
pub fn create_symlink(link: &Path, program_exe: &Path, force: bool) -> Result<(), LifecycleError> {
    let existing = classify_existing(link, program_exe);
    decide_existing(&existing, link, force)?;
    if let Some(parent) = link.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            LifecycleError::new("path_conflict", format!("{}: {e}", parent.display()))
        })?;
    }
    let temp = link.with_extension(format!("avi-link-{}", std::process::id()));
    let _ = std::fs::remove_file(&temp);
    std::os::unix::fs::symlink(program_exe, &temp).map_err(|e| {
        LifecycleError::new(
            "path_conflict",
            format!(
                "no se pudo crear el enlace temporal {}: {e}",
                temp.display()
            ),
        )
    })?;
    // `force` con algo ajeno en medio: se retira solo lo que ya era un enlace, nunca
    // un fichero. Un `--force` que borrara el binario de otro programa sería peor
    // que el conflicto que evita.
    if let Ok(meta) = std::fs::symlink_metadata(link) {
        if meta.file_type().is_symlink() {
            let _ = std::fs::remove_file(link);
        }
    }
    match std::fs::rename(&temp, link) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&temp);
            Err(LifecycleError::new(
                "path_conflict",
                14,
                format!("no se pudo poner el enlace en {}: {e}", link.display()),
            ))
        }
    }
}

/// Revierte el enlace, **solo si apunta al directorio de programa** (paso 7
/// de la desinstalación). Un enlace a otra cosa no es de la instalación y no se toca.
#[cfg(unix)]
pub fn revert_symlink(link: &Path, program_exe: &Path) -> bool {
    match classify_existing(link, program_exe) {
        Existing::Ours(_) => std::fs::remove_file(link).is_ok(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::write_file;
    use avi_core::exit_codes::ExitCode;

    fn sandbox(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("path-unix-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// El bloque se escribe una vez y no se duplica por reintentarlo: es el
    /// criterio 2 aplicado al lado de Unix.
    ///
    /// Se afirma además que lo que se escribe es exactamente el bloque publicado
    /// cuando el directorio del enlace es el convencional, porque un bloque
    /// distinto sería un contrato distinto.
    #[test]
    fn profile_block_is_idempotent() {
        let home = sandbox("idempotente");
        let bin = home.join(".local/bin");
        let profile = home.join(".profile");
        let original = "# mi perfil\nexport LANG=es_ES.UTF-8\n";
        write_file(&profile, original);

        assert!(
            write_block(&profile, &bin, &home).unwrap(),
            "la primera escritura cambia el archivo"
        );
        let body = std::fs::read_to_string(&profile).unwrap();
        let block = block_text(&bin, &home);
        assert_eq!(
            body,
            format!("{original}\n{block}"),
            "el contenido es el original más el bloque, con `$HOME` sin expandir"
        );
        assert_eq!(
            block,
            format!(
                "{BLOCK_BEGIN}\ncase \":${{PATH}}:\" in *\":$HOME/.local/bin:\"*) ;; *) \
                 export PATH=\"$HOME/.local/bin:$PATH\" ;; esac\n{BLOCK_END}"
            ),
            "y el bloque es literalmente el publicado"
        );

        // Las dos formas de idempotencia: repetir y volver a pedir el bloque.
        assert!(
            !write_block(&profile, &bin, &home).unwrap(),
            "repetir no cambia el archivo"
        );
        assert!(
            !write_block(&profile, &bin, &home).unwrap(),
            "y una tercera vez tampoco"
        );
        assert_eq!(
            std::fs::read_to_string(&profile).unwrap(),
            body,
            "el contenido es idéntico"
        );
        assert_eq!(
            body.matches(BLOCK_BEGIN).count(),
            1,
            "un solo bloque, no uno por intento"
        );

        // Un bloque nuestro con la ruta reubicada lleva la ruta absoluta, no
        // `$HOME`: es la misma integración con otro destino.
        let relocated = sandbox("reubicada");
        let other = relocated.join("bin-personalizado");
        let block = block_text(&other, &relocated);
        assert!(
            block.contains(&other.display().to_string()),
            "con `AVI_BIN_DIR` el bloque lleva la ruta real: {block}"
        );
        assert!(!block.contains("$HOME/.local/bin"));
        std::fs::remove_dir_all(&home).ok();
        std::fs::remove_dir_all(&relocated).ok();
    }

    /// La reversión deja el perfil **exactamente** como estaba, incluido el caso
    /// en que el original no terminaba en salto de línea, que es donde un
    /// `trim_end` ingenuo añadiría uno de más.
    #[test]
    fn profile_block_revert_restores_exactly() {
        let home = sandbox("revierte");
        let bin = home.join(".local/bin");
        let profile = home.join(".profile");

        // Tres originales distintos, para los tres caminos del separador.
        for original in [
            "# mi perfil\nexport LANG=es_ES.UTF-8\n",
            "# sin salto final",
            "export PATH=/usr/bin",
            "",
        ] {
            write_file(&profile, original);
            write_block(&profile, &bin, &home).unwrap();
            assert!(
                remove_block(&profile, &bin, &home).unwrap(),
                "el bloque se quita: {original:?}"
            );
            assert_eq!(
                std::fs::read_to_string(&profile).unwrap(),
                original,
                "el perfil vuelve a ser byte a byte lo que era: {original:?}"
            );
            assert!(
                !remove_block(&profile, &bin, &home).unwrap(),
                "quitar un bloque ausente es un no-op"
            );
            assert_eq!(std::fs::read_to_string(&profile).unwrap(), original);
        }

        // Un perfil con un bloque ajeno no se toca: hay un `PATH` exportado a mano
        // que no es nuestro y borrarlo sería perder configuración del usuario.
        let bashrc = home.join(".bashrc");
        let foreign = "export PATH=\"$HOME/.local/bin:$PATH\"\n";
        write_file(&bashrc, foreign);
        assert!(
            !remove_block(&bashrc, &bin, &home).unwrap(),
            "un bloque ajeno no se quita"
        );
        assert_eq!(std::fs::read_to_string(&bashrc).unwrap(), foreign);
        std::fs::remove_dir_all(&home).ok();
    }

    /// En la ruta del enlace hay algo que no es enlace propio: `path_conflict` con
    /// su código de la tabla cerrada, y `--force` lo deja pasar.
    ///
    /// La regla se afirma por sus tres estados, porque la regla dice "solo es
    /// conflicto lo que no es enlace nuestro": repetir sobre un enlace nuestro
    /// **no** es conflicto, y ese es el caso que una implementación que mirara
    /// solo `exists()` rompería en cada reejecución.
    #[test]
    fn preexisting_foreign_path_is_conflict() {
        let home = sandbox("conflicto");
        let bin = home.join(".local/bin");
        std::fs::create_dir_all(&bin).unwrap();
        let link = bin.join(crate::APP_NAME);

        // Ausente: se puede seguir.
        assert_eq!(
            classify_existing(&link, &home.join("opt/ejecutable")),
            Existing::Absent
        );
        assert!(decide_existing(&Existing::Absent, &link, false).is_ok());

        // Enlace nuestro: repetir no es conflicto.
        let exe = home.join("opt/ai-voice-interconnector/ai-voice-interconnector");
        let our = Existing::Ours(exe.clone());
        assert!(decide_existing(&our, &link, false).is_ok());
        assert!(decide_existing(&our, &link, true).is_ok());

        // Algo ajeno: conflicto con `path_conflict` y el entero 14.
        let err = decide_existing(&Existing::Foreign, &link, false)
            .expect_err("una ruta ajena es conflicto");
        assert_eq!(err.reason, "path_conflict");
        assert_eq!(
            ExitCode::from_reason(err.reason).code(),
            14,
            "`PathConflict = 14` de la tabla única"
        );
        assert!(
            err.message.contains("--force"),
            "el mensaje dice cómo se resuelve"
        );
        assert!(
            decide_existing(&Existing::Foreign, &link, true).is_ok(),
            "`--force` deja seguir"
        );

        // Y la clasificación real: un fichero de texto donde debería ir el enlace
        // es exactamente el caso que la regla llama conflicto.
        write_file(&link, "no soy un enlace\n");
        assert_eq!(
            classify_existing(&link, &exe),
            Existing::Foreign,
            "un fichero común no es un enlace nuestro"
        );
        std::fs::remove_dir_all(&home).ok();
    }

    /// El mecanismo del enlace —atomicidad, reejecución y reversión verbatim— es
    /// de Unix: en Windows crear un enlace simbólico exige privilegios que la
    /// puerta de CI no tiene. La regla que decide sí se afirma en todas partes
    /// (`preexisting_foreign_path_is_conflict`).
    #[cfg(unix)]
    #[test]
    fn symlink_round_trip_and_verbatim_revert() {
        let home = sandbox("symlink");
        let bin = home.join(".local/bin");
        let exe = home.join("opt/ai-voice-interconnector/ai-voice-interconnector");
        write_file(&exe, "binario");
        std::fs::create_dir_all(&bin).unwrap();
        let link = bin.join(crate::APP_NAME);

        create_symlink(&link, &exe, false).expect("una ruta libre no es conflicto");
        assert_eq!(classify_existing(&link, &exe), Existing::Ours(exe.clone()));
        create_symlink(&link, &exe, false).expect("recrear el enlace propio no falla");

        // Enlace a otro sitio: conflicto, y con `--force` se sustituye.
        // El enlace propio que hay en `link` se quita antes: `symlink` no sobrescribe
        // un destino existente, y sin quitarlo la prueba falla con `AlreadyExists` antes
        // de llegar a la clasificación.
        let other = home.join("opt/otra-cosa/ai-voice-interconnector");
        write_file(&other, "otro binario");
        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(&other, &link).unwrap();
        assert_eq!(classify_existing(&link, &exe), Existing::Foreign);
        assert!(create_symlink(&link, &exe, false).is_err());
        assert_eq!(
            std::fs::read_link(&link).unwrap(),
            other,
            "sin `--force` el enlace ajeno no se toca"
        );
        create_symlink(&link, &exe, true).expect("`--force` sustituye el enlace ajeno");
        assert_eq!(classify_existing(&link, &exe), Existing::Ours(exe.clone()));

        // La reversión solo toca lo nuestro: un enlace a otra cosa sobrevive.
        assert!(revert_symlink(&link, &exe), "el enlace nuestro se quita");
        assert!(link.symlink_metadata().is_err(), "y desaparece");
        std::os::unix::fs::symlink(&other, &link).unwrap();
        assert!(
            !revert_symlink(&link, &exe),
            "un enlace ajeno no lo revierte la instalación"
        );
        assert!(link.symlink_metadata().is_ok());
        std::fs::remove_dir_all(&home).ok();
    }

    /// Los archivos de arranque que se tocan son los de la tabla, y
    /// solo se añade `.bashrc` si existe: escribirlo en una máquina sin bash sería
    /// crear un archivo que el usuario no pidió.
    #[test]
    fn profile_targets_follow_the_shell_table() {
        let home = sandbox("shells");
        let exists = |p: &Path| p == home.join(".bashrc");
        let zdotdir = home.join("cfg/zsh");
        let zdot = Some(zdotdir.as_path());

        assert_eq!(
            profile_targets(&home, zdot, Shell::Bash, exists),
            vec![home.join(".profile"), home.join(".bashrc")],
            "bash: `.profile` y el `.bashrc` que existe"
        );
        assert_eq!(
            profile_targets(&home, zdot, Shell::Sh, exists),
            vec![home.join(".profile"), home.join(".bashrc")],
            "sh hereda la fila de bash"
        );
        assert_eq!(
            profile_targets(&home, zdot, Shell::Zsh, exists),
            vec![
                home.join(".profile"),
                home.join(".bashrc"),
                zdotdir.join(".zshrc"),
            ],
            "zsh: `$ZDOTDIR` manda sobre `$HOME`"
        );
        assert_eq!(
            profile_targets(&home, None, Shell::Zsh, exists),
            vec![
                home.join(".profile"),
                home.join(".bashrc"),
                home.join(".zshrc")
            ],
            "sin `ZDOTDIR`, el `.zshrc` cae en `$HOME`"
        );
        assert_eq!(
            profile_targets(&home, zdot, Shell::Fish, exists),
            vec![
                home.join(".profile"),
                home.join(".bashrc"),
                home.join(".config/fish/conf.d/ai-voice-interconnector.fish"),
            ],
            "fish: el archivo propio de la tabla"
        );
        assert_eq!(
            profile_targets(&home, zdot, Shell::Sh, |_| false),
            vec![home.join(".profile")],
            "sin `.bashrc` no se crea uno"
        );
        assert_eq!(Shell::from_env(Some("/bin/zsh")), Shell::Zsh);
        assert_eq!(Shell::from_env(Some("/usr/bin/bash")), Shell::Bash);
        assert_eq!(Shell::from_env(Some("/usr/local/bin/fish")), Shell::Fish);
        assert_eq!(Shell::from_env(Some("/bin/dash")), Shell::Sh);
        assert_eq!(Shell::from_env(None), Shell::Sh);
        std::fs::remove_dir_all(&home).ok();
    }

    /// El bloque solo se añade cuando el directorio del enlace no está en el
    /// `PATH`, y se reconoce en las dos formas en que puede estar escrito.
    #[test]
    fn block_only_needed_when_bin_dir_absent_from_path() {
        let home = sandbox("necesita");
        let bin = home.join(".local/bin");
        // El `PATH` se compone con `join_paths`, que usa el separador de cada
        // plataforma: en Windows la letra de la unidad lleva `:` y trocear por `:`
        // partiría cada entrada en dos.
        let join_paths = |entries: &[PathBuf]| {
            std::env::join_paths(entries)
                .expect("se compone el PATH")
                .to_string_lossy()
                .to_string()
        };
        let path = |relative: &str| home.join(relative);

        assert!(
            needs_block(&join_paths(&[path("usr/bin"), path("bin")]), &bin, &home),
            "sin el directorio del enlace, hace falta"
        );
        assert!(
            !needs_block(
                &join_paths(&[path("usr/bin"), bin.clone(), path("bin")]),
                &bin,
                &home
            ),
            "ya está en el PATH, no hace falta"
        );
        assert!(
            needs_block(
                &join_paths(&[path("usr/bin"), path(".local/bin2"), path("bin")]),
                &bin,
                &home
            ),
            "un directorio vecino no es el directorio del enlace"
        );
        assert!(
            needs_block(
                &join_paths(&[path("usr/bin"), path("bin"), path("bin")]),
                &bin,
                &home
            ),
            "otro directorio de binarios tampoco"
        );
        assert!(
            needs_block("", &bin, &home),
            "un PATH vacío necesita el bloque"
        );
        std::fs::remove_dir_all(&home).ok();
    }

    /// La forma sin expandir que escribe el propio bloque —`$HOME/.local/bin`— se
    /// reconoce como la misma entrada. Sin esto, un `PATH` que ya traiga el bloque
    /// de una instalación anterior volvería a recibirlo.
    ///
    /// Solo tiene sentido en Unix: la línea es `export PATH="$HOME/.local/bin:$PATH"`
    /// de un perfil de shell, y en Windows el `PATH` se separa con `;` y no admite
    /// variables sin resolver en la línea de comandos.
    #[cfg(unix)]
    #[test]
    fn block_not_needed_when_path_has_the_unexpanded_form() {
        let home = sandbox("sin-expandir");
        let bin = home.join(".local/bin");
        assert!(
            !needs_block("/usr/bin:$HOME/.local/bin", &bin, &home),
            "la forma sin expandir cuenta como la misma entrada"
        );
        assert!(
            needs_block("/usr/bin:$HOME/.local/bin2", &bin, &home),
            "pero una variable distinta no"
        );
        assert!(
            needs_block("/usr/bin:$OTRA/.local/bin", &bin, &home),
            "ni una variable que no es `HOME`"
        );
        std::fs::remove_dir_all(&home).ok();
    }
}
