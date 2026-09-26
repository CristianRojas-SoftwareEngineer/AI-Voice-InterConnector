//! Confirmación y simulación de las operaciones de ciclo de vida (§9.1).
//!
//! La tabla de §9.1 tiene cuatro celdas, y las cuatro importan porque el defecto
//! que corrigen es que **sin terminal se borra en silencio**: hoy la
//! confirmación solo se pide cuando hay TTY, así que un `cleanup` sin terminal
//! borra sin preguntar. Aquí la ausencia de terminal es un dato explícito, no una
//! señal de que se pueda pasar, y el predicado se inyecta para que las pruebas
//! puedan ejercitar las cuatro celdas sin un terminal real.
//!
//! | Tipo | Con terminal | Sin terminal |
//! |---|---|---|
//! | No destructiva | Resumen y `[S/n]`; `--yes` la omite | Procede sin preguntar |
//! | Destructiva | Rutas con tamaños y `[s/N]`; `--yes` la omite | Exige `--yes`; sin él, `confirmation_required` y 2 |
//!
//! **El plan se imprime siempre** —en las cuatro celdas— porque sin terminal la
//! operación no destructiva también necesita que el usuario sepa qué va a pasar,
//! y `--dry-run` es exactamente "imprime el plan sin modificar el disco".
//!
//! **Lo que el usuario no confirma no es un error de contrato.** El producto
//! actual responde `Cancelado.` y sale con éxito, y así sigue: `Cancelled` es
//! una decisión, no un `reason`. Los `reason` que sí son contrato están en §9.1
//! y son `confirmation_required` y `usage_error`.
//!
//! La lectura de la respuesta y la escritura del prompt son parámetros, no
//! `std::io` directo: el prompt va a **stderr** (§9.1) y la respuesta se lee de
//! **stdin**, que es justo lo que el bootstrap redirige a la terminal de control
//! para que `curl | sh` siga siendo interactivo.

use crate::LifecycleError;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

/// Tipo de operación según la tabla de §9.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `self install`, `self update`, `setup`: no borran nada por sí solas.
    NonDestructive,
    /// `self uninstall`, `cleanup`, `setup --force-update`, degradar de versión,
    /// `xtask clean`.
    Destructive,
}

impl Kind {
    /// Literal del prompt: el valor por defecto de la celda de la tabla.
    pub fn prompt_literal(self) -> &'static str {
        match self {
            Self::NonDestructive => "[S/n]",
            Self::Destructive => "[s/N]",
        }
    }

    /// Prefijo de la línea de cierre, distinto porque la consecuencia no es la
    /// misma: una no destructiva solo dice lo que va a hacer, la destructiva
    /// recuerda que borra.
    fn question(self) -> &'static str {
        match self {
            Self::NonDestructive => "¿Continuar?",
            Self::Destructive => "Esto eliminará lo indicado. ¿Continuar?",
        }
    }

    /// `true` si la operación borra o reemplaza algo.
    pub fn is_destructive(self) -> bool {
        matches!(self, Self::Destructive)
    }
}

/// Una ruta del plan con su tamaño, para el listado que §9.1 exige en las
/// operaciones destructivas. El tamaño es `None` cuando la ruta no existe, que es
/// el caso de `self uninstall` antes de haber instalado nada.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanEntry {
    pub path: PathBuf,
    pub size: Option<u64>,
}

impl PlanEntry {
    /// Construye la entrada leyendo el tamaño actual de `path`.
    pub fn of(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref().to_path_buf();
        let size = std::fs::metadata(&path).map(|m| m.len()).ok();
        Self { path, size }
    }
}

/// Petición de confirmación: el plan que se muestra y las banderas que lo
/// gobiernan.
#[derive(Debug)]
pub struct Confirmation<'a> {
    pub kind: Kind,
    /// Resumen de cambios de la operación no destructiva (versión, programa,
    /// `PATH`, modelos). Se imprime siempre.
    pub summary: &'a [String],
    /// Rutas que la operación va a tocar. En las destructivas es la lista con
    /// tamaños que §9.1 exige; en las no destructivas puede estar vacía.
    pub entries: &'a [PlanEntry],
    /// `--yes`: omite la pregunta.
    pub assume_yes: bool,
    /// `--dry-run`: imprime el plan y no modifica el disco.
    pub dry_run: bool,
    /// Si stdin es una TTY. Se inyecta (`std::io::IsTerminal` en producción) para
    /// que las pruebas alcancen las cuatro celdas de la tabla.
    pub stdin_is_terminal: bool,
}

/// Qué hacer después de imprimir el plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// El usuario confirmó (o no hacía falta preguntar): seguir.
    Proceed,
    /// `--dry-run`: el plan está impreso y no se toca el disco.
    DryRun,
    /// El usuario dijo que no. No es un error (§9.1 no lo lista entre los
    /// `reason`): el producto responde `Cancelado.` y sale con éxito.
    Cancelled,
}

/// Aplica la tabla de §9.1 y devuelve la decisión.
///
/// `input` es stdin y `out` es stderr, para que el prompt no contamine la salida
/// que el contrato del CLI declara. Ninguna rama escribe en el disco: el módulo
/// solo decide, y quien borra lo hace después de obtener `Proceed`.
pub fn confirm<R: BufRead>(
    request: &Confirmation<'_>,
    input: &mut R,
    out: &mut dyn Write,
) -> anyhow::Result<Decision> {
    print_plan(request, out)?;

    if request.dry_run {
        writeln!(
            out,
            "Simulación: se muestra lo que {} haría, no se ha modificado nada.",
            crate::APP_NAME
        )?;
        return Ok(Decision::DryRun);
    }

    if request.assume_yes {
        return Ok(Decision::Proceed);
    }

    if !request.stdin_is_terminal {
        return match request.kind {
            // Sin terminal, lo no destructivo procede (§9.1).
            Kind::NonDestructive => Ok(Decision::Proceed),
            // Sin terminal, lo destructivo exige `--yes`.
            Kind::Destructive => Err(LifecycleError::confirmation_required(format!(
                "operación destructiva sin terminal y sin `--yes`: {}",
                crate::APP_NAME
            ))
            .into()),
        };
    }

    write!(
        out,
        "{} {}",
        request.kind.question(),
        request.kind.prompt_literal()
    )?;
    out.flush()?;
    let mut line = String::new();
    input.read_line(&mut line)?;
    if affirmative(&line, request.kind) {
        Ok(Decision::Proceed)
    } else {
        writeln!(out, "Cancelado.")?;
        Ok(Decision::Cancelled)
    }
}

/// Interpreta la respuesta. El valor por defecto de cada celda manda cuando la
/// respuesta es solo un salto de línea: `Enter` acepta lo no destructivo y
/// rechaza lo destructivo. Cualquier respuesta que no sea un sí explícito es un
/// no, también en la celda no destructiva: una errata nunca debe autorizar un
/// borrado.
fn affirmative(line: &str, kind: Kind) -> bool {
    match line.trim().to_ascii_lowercase().as_str() {
        "" => !kind.is_destructive(),
        "y" | "yes" | "s" | "si" | "sí" | "dale" | "ok" => true,
        _ => false,
    }
}

/// Imprime el plan: el resumen de cambios y, si los hay, las rutas con tamaños.
fn print_plan(request: &Confirmation<'_>, out: &mut dyn Write) -> anyhow::Result<()> {
    for line in request.summary {
        writeln!(out, "{line}")?;
    }
    for entry in request.entries {
        match entry.size {
            Some(size) => writeln!(out, "  {} ({})", entry.path.display(), human_size(size))?,
            None => writeln!(out, "  {}", entry.path.display())?,
        }
    }
    Ok(())
}

/// Tamaño legible, con la misma escala que usa el resto del producto (MB y GB,
/// un decimal).
fn human_size(bytes: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    const GB: f64 = MB * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.1} GB", b / GB)
    } else {
        format!("{:.1} MB", b / MB)
    }
}

/// Categorías que acepta `cleanup` (§9.6). La puerta de uso está aquí porque es la
/// misma regla transversal de §9.1: sin categoría, `usage_error` y 2, sin borrar
/// nada.
pub const CLEANUP_CATEGORIES: [&str; 4] = ["model", "voices", "synthetic-speech", "all"];

/// Exige que `cleanup` reciba al menos una categoría. Sin ella, `usage_error` con
/// código 2 y sin tocar el disco.
pub fn require_cleanup_category(selected: &[&str]) -> anyhow::Result<()> {
    if !selected.is_empty() {
        return Ok(());
    }
    Err(LifecycleError::usage_error(format!(
        "cleanup necesita una categoría: {}",
        CLEANUP_CATEGORIES.join(", ")
    ))
    .into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{scratch, snapshot, write_file};
    use std::io::Cursor;

    fn request<'a>(
        kind: Kind,
        summary: &'a [String],
        entries: &'a [PlanEntry],
        assume_yes: bool,
        dry_run: bool,
        stdin_is_terminal: bool,
    ) -> Confirmation<'a> {
        Confirmation {
            kind,
            summary,
            entries,
            assume_yes,
            dry_run,
            stdin_is_terminal,
        }
    }

    /// Las cuatro celdas de la tabla de §9.1 con el predicado de terminal
    /// inyectado, y en cada celda el prompt que le corresponde.
    #[test]
    fn confirmation_matrix_covers_four_cells() {
        let dir = scratch("confirm-matriz");
        let dest = dir.join("ai-voice-interconnector");
        write_file(&dest, "contenido");
        let entries = vec![PlanEntry::of(&dest)];
        let summary = vec!["Se instalará ai-voice-interconnector 0.24.0".to_string()];

        // Columna "con terminal", fila no destructiva: se pregunta con `[S/n]` y
        // `Enter` acepta.
        let mut out = Vec::new();
        let decision = confirm(
            &request(Kind::NonDestructive, &summary, &[], false, false, true),
            &mut Cursor::new("\n"),
            &mut out,
        )
        .unwrap();
        assert_eq!(decision, Decision::Proceed);
        let out = String::from_utf8(out).unwrap();
        assert!(out.contains("[S/n]"), "literal de la celda: {out}");
        assert!(!out.contains("[s/N]"), "no es la celda destructiva: {out}");

        // Columna "con terminal", fila destructiva: se pregunta con `[s/N]`,
        // `Enter` rechaza y una `s` explícita acepta.
        let mut out = Vec::new();
        let decision = confirm(
            &request(Kind::Destructive, &summary, &entries, false, false, true),
            &mut Cursor::new("\n"),
            &mut out,
        )
        .unwrap();
        assert_eq!(decision, Decision::Cancelled, "el valor por defecto es no");
        let out = String::from_utf8(out).unwrap();
        assert!(out.contains("[s/N]"), "literal de la celda: {out}");
        assert!(out.contains("Cancelado."), "cancela sin error: {out}");
        assert!(
            out.contains(&dest.display().to_string()) && out.contains("MB"),
            "la celda destructiva lista rutas con tamaños: {out}"
        );

        let mut out = Vec::new();
        let decision = confirm(
            &request(Kind::Destructive, &summary, &entries, false, false, true),
            &mut Cursor::new("s\n"),
            &mut out,
        )
        .unwrap();
        assert_eq!(decision, Decision::Proceed, "una `s` explícita acepta");

        // Columna "sin terminal", fila no destructiva: procede sin preguntar, y
        // no lee stdin.
        let mut out = Vec::new();
        let decision = confirm(
            &request(Kind::NonDestructive, &summary, &[], false, false, false),
            &mut Cursor::new(""),
            &mut out,
        )
        .unwrap();
        assert_eq!(decision, Decision::Proceed);
        let out = String::from_utf8(out).unwrap();
        assert!(!out.contains('?'), "no se pregunta: {out}");
        assert!(out.contains("0.24.0"), "el plan se imprime igual: {out}");

        // Columna "sin terminal", fila destructiva: sin `--yes` es
        // `confirmation_required`, y con `--yes` procede. Es la cuarta celda de la
        // matriz y la razón de que el predicado sea inyectable.
        let mut out = Vec::new();
        let err = confirm(
            &request(Kind::Destructive, &summary, &entries, false, false, false),
            &mut Cursor::new(""),
            &mut out,
        )
        .unwrap_err();
        let failure = err
            .downcast_ref::<LifecycleError>()
            .expect("reason de contrato");
        assert_eq!(failure.reason, "confirmation_required");
        assert_eq!(failure.exit_code, 2);
        let mut out = Vec::new();
        assert_eq!(
            confirm(
                &request(Kind::Destructive, &summary, &entries, true, false, false),
                &mut Cursor::new(""),
                &mut out,
            )
            .unwrap(),
            Decision::Proceed,
            "`--yes` omite la pregunta incluso sin terminal"
        );
        assert!(
            !String::from_utf8(out).unwrap().contains('?'),
            "con `--yes` no se pregunta"
        );

        // La puerta de uso de `cleanup` es la otra mitad de la acción: sin
        // categoría, `usage_error` y 2.
        assert!(require_cleanup_category(&["model"]).is_ok());
        assert!(require_cleanup_category(&CLEANUP_CATEGORIES).is_ok());
        let err = require_cleanup_category(&[]).unwrap_err();
        let failure = err.downcast_ref::<LifecycleError>().unwrap();
        assert_eq!(failure.reason, "usage_error");
        assert_eq!(failure.exit_code, 2);
        assert!(failure.message.contains("model"), "{}", failure.message);

        // Lectura de respuestas: `Enter` sigue el valor por defecto de cada
        // celda, y una respuesta que no sea un no cuenta como sí.
        assert!(affirmative("\n", Kind::NonDestructive));
        assert!(!affirmative("\n", Kind::Destructive));
        assert!(affirmative("S\n", Kind::Destructive));
        assert!(affirmative("si\n", Kind::Destructive));
        assert!(affirmative("sí\n", Kind::Destructive));
        assert!(!affirmative("n\n", Kind::NonDestructive));
        assert!(!affirmative("no\n", Kind::NonDestructive));
        assert!(!affirmative("cancelar\n", Kind::NonDestructive));
        assert!(Kind::Destructive.is_destructive() && !Kind::NonDestructive.is_destructive());

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Destructiva sin terminal y sin `--yes`: `confirmation_required` con
    /// código 2, y nada borrado — que es el criterio 19 de §15.
    #[test]
    fn destructive_without_tty_and_without_yes_exits_2() {
        let dir = scratch("confirm-sin-tty");
        let data = dir.join("datos");
        let model = dir.join("modelos");
        write_file(&data.join("voz.wav"), "voz sintetizada");
        write_file(&model.join("revision.bin"), &"pesado".repeat(1024));
        let before = snapshot(&dir);

        let entries = vec![PlanEntry::of(&data), PlanEntry::of(&model)];
        let mut out = Vec::new();
        let err = confirm(
            &Confirmation {
                kind: Kind::Destructive,
                summary: &["cleanup borrará modelos y habla sintetizada".to_string()],
                entries: &entries,
                assume_yes: false,
                dry_run: false,
                stdin_is_terminal: false,
            },
            &mut Cursor::new("y\n"),
            &mut out,
        )
        .unwrap_err();

        let failure = err
            .downcast_ref::<LifecycleError>()
            .expect("reason de contrato");
        assert_eq!(failure.reason, "confirmation_required");
        assert_eq!(failure.exit_code, 2, "error de uso, no un error genérico");
        // Que la respuesta fuera `y` no cambia nada: sin terminal no hay a quién
        // preguntarle, y el criterio es exigir `--yes`.
        assert_eq!(snapshot(&dir), before, "no se borra nada sin `--yes`");
        let printed = String::from_utf8(out.clone()).unwrap();
        assert!(printed.contains("datos"), "el plan se imprime: {printed}");

        // Con `--yes` la misma operación procede, y tampoco borra: el borrado lo
        // hace quien ejecuta, después de la decisión.
        assert_eq!(
            confirm(
                &Confirmation {
                    kind: Kind::Destructive,
                    summary: &[],
                    entries: &entries,
                    assume_yes: true,
                    dry_run: false,
                    stdin_is_terminal: false,
                },
                &mut Cursor::new(""),
                &mut out,
            )
            .unwrap(),
            Decision::Proceed
        );
        assert_eq!(snapshot(&dir), before);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// `--dry-run` imprime el plan y no modifica el disco: mismo árbol, mismos
    /// tamaños, antes y después.
    #[test]
    fn dry_run_does_not_touch_disk() {
        let dir = scratch("confirm-dry-run");
        let dest = dir.join("ai-voice-interconnector");
        let model = dir.join("models").join("revision.bin");
        write_file(&dest, "contenido");
        write_file(&model, &"x".repeat(2048));
        let before = snapshot(&dir);

        let entries = vec![PlanEntry::of(&dest), PlanEntry::of(&model)];
        let summary = vec!["Se eliminará lo siguiente:".to_string()];
        let mut out = Vec::new();
        let decision = confirm(
            &Confirmation {
                kind: Kind::Destructive,
                summary: &summary,
                entries: &entries,
                assume_yes: false,
                dry_run: true,
                stdin_is_terminal: true,
            },
            &mut Cursor::new(""),
            &mut out,
        )
        .unwrap();
        assert_eq!(decision, Decision::DryRun);
        assert_eq!(snapshot(&dir), before, "el disco no se toca");

        let printed = String::from_utf8(out.clone()).unwrap();
        assert!(
            printed.contains("Simulación"),
            "se anuncia la simulación: {printed}"
        );
        assert!(
            printed.contains(&model.display().to_string()) && printed.contains("0.0 MB"),
            "el plan se imprime con rutas y tamaños: {printed}"
        );
        assert!(
            !printed.contains('?'),
            "en simulación no se pregunta, no hay nada que confirmar: {printed}"
        );

        // La simulación gana a `--yes` y a la terminal: en las cuatro celdas el
        // resultado es el mismo, porque no se toca el disco en ninguna.
        for (yes, terminal) in [(true, true), (false, false), (true, false)] {
            assert_eq!(
                confirm(
                    &Confirmation {
                        kind: Kind::Destructive,
                        summary: &summary,
                        entries: &entries,
                        assume_yes: yes,
                        dry_run: true,
                        stdin_is_terminal: terminal,
                    },
                    &mut Cursor::new(""),
                    &mut out,
                )
                .unwrap(),
                Decision::DryRun
            );
        }
        assert_eq!(
            snapshot(&dir),
            before,
            "tampoco al combinarla con otras banderas"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
