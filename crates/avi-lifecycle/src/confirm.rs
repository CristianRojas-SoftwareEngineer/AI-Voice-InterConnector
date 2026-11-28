// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! Confirmación de las operaciones de ciclo de vida.
//!
//! La tabla de confirmaciones tiene cuatro celdas, y las cuatro importan porque
//! **sin terminal no se borra en silencio**: una operación destructiva sin
//! terminal exige `--yes`. La ausencia de terminal es un dato explícito, no una
//! señal de que se pueda pasar, y el predicado se inyecta para que las pruebas
//! puedan ejercitar las cuatro celdas sin un terminal real.
//!
//! | Tipo | Con terminal | Sin terminal |
//! |---|---|---|
//! | No destructiva | Resumen y `[S/n]`; `--yes` la omite | Procede sin preguntar |
//! | Destructiva | Rutas con tamaños y `[s/N]`; `--yes` la omite | Exige `--yes`; sin él, `confirmation_required` y 2 |
//!
//! **El plan se imprime siempre** —en las cuatro celdas— porque sin terminal la
//! operación no destructiva también necesita que el usuario sepa qué va a pasar.
//! La simulación (`--dry-run`) no pasa por la confirmación: quien la ofrece
//! imprime su plan y se detiene antes de preguntar.
//!
//! **Lo que el usuario no confirma no es un error de contrato.** `Cancelled` es
//! una decisión, no un `reason`: el módulo no escribe el texto de cancelación,
//! lo pinta el llamador detrás del separador con [`Kind::cancelled_closing`] y
//! la operación sale con éxito. Los `reason` que sí son contrato son
//! `confirmation_required` y `usage_error`.
//!
//! La lectura de la respuesta y la escritura del prompt son parámetros, no
//! `std::io` directo: el prompt va a **stderr** y la respuesta se lee de
//! **stdin**, que es justo lo que el bootstrap redirige a la terminal de control
//! para que `curl | sh` siga siendo interactivo.

use crate::LifecycleError;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

/// Tipo de operación según la tabla de confirmaciones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `self install`, `self update`, `setup`: no borran nada por sí solas.
    NonDestructive,
    /// `self uninstall`, `cleanup`, `setup --force-update`, degradar de versión,
    /// `xtask clean` y `voice remove` de una voz con locuciones.
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

    /// `true` si la operación borra o reemplaza algo.
    pub fn is_destructive(self) -> bool {
        matches!(self, Self::Destructive)
    }

    /// Cierre que pinta el llamador detrás del separador al cancelar: una
    /// operación destructiva aclara que no se ha borrado nada; la que no borra
    /// dice solo `Cancelado.`.
    pub fn cancelled_closing(self) -> avi_term::line::SpecialClosing {
        match self {
            Self::NonDestructive => avi_term::line::SpecialClosing::CancelledNonDestructive,
            Self::Destructive => avi_term::line::SpecialClosing::Cancelled,
        }
    }
}

/// Una ruta del plan con su tamaño, para el listado que hay que hacer en las
/// operaciones destructivas. El tamaño es `None` cuando la ruta no existe, que es
/// el caso de `self uninstall` antes de haber instalado nada.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanEntry {
    pub path: PathBuf,
    pub size: Option<u64>,
}

impl PlanEntry {
    /// Construye la entrada leyendo el tamaño actual de `path`: el total recursivo si
    /// es un directorio, no el de su entrada en el sistema de archivos.
    pub fn of(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref().to_path_buf();
        let size = std::fs::symlink_metadata(&path)
            .is_ok()
            .then(|| crate::cleanup::path_size(&path));
        Self { path, size }
    }
}

/// Petición de confirmación: el plan que se muestra y las banderas que lo
/// gobiernan.
#[derive(Debug)]
pub struct Confirmation<'a> {
    pub kind: Kind,
    /// Pregunta concreta de la operación, sin el sufijo `[s/N]` ni `[S/n]`, que
    /// pone `confirm` según el tipo. Dice qué se va a hacer o a borrar.
    pub question: &'a str,
    /// Resumen de cambios de la operación no destructiva (versión, programa,
    /// `PATH`, modelos). Se imprime siempre.
    pub summary: &'a [String],
    /// Rutas que la operación va a tocar. En las destructivas es la lista con
    /// tamaños que la tabla exige; en las no destructivas puede estar vacía.
    pub entries: &'a [PlanEntry],
    /// `--yes`: omite la pregunta.
    pub assume_yes: bool,
    /// Si stdin es una TTY. Se inyecta (`std::io::IsTerminal` en producción) para
    /// que las pruebas alcancen las cuatro celdas de la tabla.
    pub stdin_is_terminal: bool,
}

/// Qué hacer después de imprimir el plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// El usuario confirmó (o no hacía falta preguntar): seguir.
    Proceed,
    /// El usuario dijo que no. No es un error (la tabla de `reason` no lo
    /// lista): el llamador pinta el cierre de cancelación y sale con éxito.
    Cancelled,
}

/// Aplica la tabla de confirmaciones y devuelve la decisión. Al cancelar no
/// escribe el texto de cancelación: lo pinta el llamador detrás del separador,
/// con el cierre especial de cancelación del indicador de etapas.
///
/// `input` es stdin y `out` es stderr, para que el prompt no contamine la salida
/// que el contrato del CLI declara. Ninguna rama escribe en el disco: el módulo
/// solo decide, y quien borra lo hace después de obtener `Proceed`.
pub fn confirm_without_cancel_notice<R: BufRead>(
    request: &Confirmation<'_>,
    input: &mut R,
    out: &mut dyn Write,
) -> anyhow::Result<Decision> {
    print_plan(request, out)?;

    if request.assume_yes {
        return Ok(Decision::Proceed);
    }

    if !request.stdin_is_terminal {
        return match request.kind {
            // Sin terminal, lo no destructivo procede.
            Kind::NonDestructive => Ok(Decision::Proceed),
            // Sin terminal, lo destructivo exige `--yes`.
            Kind::Destructive => Err(LifecycleError::confirmation_required(
                "Esta orden necesita tu confirmación, pero no hay una terminal para preguntarte.",
            )
            .into()),
        };
    }

    // El espacio final separa la respuesta que el usuario teclea del prompt.
    write!(
        out,
        "{} {} ",
        request.question,
        request.kind.prompt_literal()
    )?;
    out.flush()?;
    let mut line = String::new();
    input.read_line(&mut line)?;
    if affirmative(&line, request.kind) {
        Ok(Decision::Proceed)
    } else {
        Ok(Decision::Cancelled)
    }
}

/// Pregunta fuera del reloj del comando: abre la pausa del registro, aplica la
/// tabla de confirmaciones sin escribir el texto de cancelación y cierra la
/// pausa; si se confirmó, el reloj arranca al responder. Con error, cierra la
/// pausa sin confirmar y lo propaga.
pub fn ask<R: BufRead>(
    request: &Confirmation<'_>,
    input: &mut R,
    out: &mut dyn Write,
) -> anyhow::Result<Decision> {
    avi_term::report::begin_question();
    let decision = confirm_without_cancel_notice(request, input, out);
    avi_term::report::end_question(matches!(decision, Ok(Decision::Proceed)));
    decision
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
            Some(size) => writeln!(
                out,
                "  {} ({})",
                entry.path.display(),
                avi_term::fmt::bytes(size)
            )?,
            None => writeln!(out, "  {}", entry.path.display())?,
        }
    }
    Ok(())
}

/// Categorías que acepta `cleanup`. La puerta de uso está aquí porque es la
/// misma regla transversal: sin categoría, `usage_error` y 2, sin borrar
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
    use avi_core::exit_codes::ExitCode;
    use std::io::Cursor;

    fn request<'a>(
        kind: Kind,
        summary: &'a [String],
        entries: &'a [PlanEntry],
        assume_yes: bool,
        stdin_is_terminal: bool,
    ) -> Confirmation<'a> {
        Confirmation {
            kind,
            question: "¿Hacer la prueba?",
            summary,
            entries,
            assume_yes,
            stdin_is_terminal,
        }
    }

    /// El tamaño de un directorio es el total de lo que contiene, no el de su entrada
    /// en el sistema de archivos; una ruta ausente no tiene tamaño.
    #[test]
    fn plan_entry_sizes_a_directory_recursively() {
        let dir = scratch("plan-entry-recursive");
        write_file(&dir.join("a"), "12345");
        write_file(&dir.join("sub").join("b"), "123");

        assert_eq!(PlanEntry::of(&dir).size, Some(8));
        assert_eq!(PlanEntry::of(dir.join("a")).size, Some(5));
        assert_eq!(PlanEntry::of(dir.join("no_existe")).size, None);
    }

    /// Sin aviso de cancelación, el prompt queda como último texto escrito y la
    /// decisión sigue siendo `Cancelled`.
    #[test]
    fn cancelling_without_notice_leaves_only_the_prompt() {
        let summary = vec!["Se borrará algo:".to_string()];
        let mut out = Vec::new();
        let decision = confirm_without_cancel_notice(
            &request(Kind::Destructive, &summary, &[], false, true),
            &mut Cursor::new("n\n"),
            &mut out,
        )
        .unwrap();
        assert_eq!(decision, Decision::Cancelled);
        let out = String::from_utf8(out).unwrap();
        assert!(out.ends_with("¿Hacer la prueba? [s/N] "), "{out:?}");
        assert!(!out.contains("Cancelado"), "{out:?}");
    }

    /// `ask` cierra la pausa de la pregunta al responder, también al cancelar, y
    /// no escribe el texto de cancelación.
    #[test]
    fn ask_closes_the_question_and_does_not_announce_cancellation() {
        let _guard = avi_term::report::test_guard();
        let summary: Vec<String> = Vec::new();
        for (answer, expected) in [("s\n", Decision::Proceed), ("n\n", Decision::Cancelled)] {
            let mut out = Vec::new();
            let decision = ask(
                &request(Kind::Destructive, &summary, &[], false, true),
                &mut Cursor::new(answer),
                &mut out,
            )
            .unwrap();
            assert_eq!(decision, expected);
            assert!(
                !avi_term::report::snapshot().question_active,
                "la pausa de la pregunta sigue abierta tras {answer:?}"
            );
            let out = String::from_utf8(out).unwrap();
            assert!(!out.contains("Cancelado"), "{out:?}");
        }
    }

    /// El cierre de cancelación depende de si la operación borra algo.
    #[test]
    fn cancelled_closing_maps_each_kind() {
        use avi_term::line::SpecialClosing;
        assert_eq!(
            Kind::Destructive.cancelled_closing(),
            SpecialClosing::Cancelled
        );
        assert_eq!(
            Kind::NonDestructive.cancelled_closing(),
            SpecialClosing::CancelledNonDestructive
        );
    }

    /// Las cuatro celdas de la tabla de confirmaciones con el predicado de terminal
    /// inyectado, y en cada celda el prompt que le corresponde.
    #[test]
    fn confirmation_matrix_covers_four_cells() {
        let dir = scratch("confirm-matrix");
        let dest = dir.join("ai-voice-interconnector");
        write_file(&dest, "contenido");
        let entries = vec![PlanEntry::of(&dest)];
        let summary = vec!["Se instalará ai-voice-interconnector 0.24.0".to_string()];

        // Columna "con terminal", fila no destructiva: se pregunta con `[S/n]` y
        // `Enter` acepta.
        let mut out = Vec::new();
        let decision = confirm_without_cancel_notice(
            &request(Kind::NonDestructive, &summary, &[], false, true),
            &mut Cursor::new("\n"),
            &mut out,
        )
        .unwrap();
        assert_eq!(decision, Decision::Proceed);
        let out = String::from_utf8(out).unwrap();
        assert!(out.contains("[S/n]"), "literal de la celda: {out}");
        assert!(
            out.ends_with("¿Hacer la prueba? [S/n] "),
            "el prompt lleva la pregunta concreta y termina con espacio: {out:?}"
        );
        assert!(!out.contains("[s/N]"), "no es la celda destructiva: {out}");

        // Columna "con terminal", fila destructiva: se pregunta con `[s/N]`,
        // `Enter` rechaza y una `s` explícita acepta.
        let mut out = Vec::new();
        let decision = confirm_without_cancel_notice(
            &request(Kind::Destructive, &summary, &entries, false, true),
            &mut Cursor::new("\n"),
            &mut out,
        )
        .unwrap();
        assert_eq!(decision, Decision::Cancelled, "el valor por defecto es no");
        let out = String::from_utf8(out).unwrap();
        assert!(out.contains("[s/N]"), "literal de la celda: {out}");
        assert!(
            out.ends_with("¿Hacer la prueba? [s/N] "),
            "el prompt termina con espacio y no escribe la cancelación: {out:?}"
        );
        // La cancelación de una operación no destructiva tampoco se escribe aquí.
        let mut out = Vec::new();
        let decision = confirm_without_cancel_notice(
            &request(Kind::NonDestructive, &summary, &[], false, true),
            &mut Cursor::new("n\n"),
            &mut out,
        )
        .unwrap();
        assert_eq!(decision, Decision::Cancelled);
        assert!(String::from_utf8(out).unwrap().ends_with("[S/n] "));
        let mut out = Vec::new();
        confirm_without_cancel_notice(
            &request(Kind::Destructive, &summary, &entries, false, true),
            &mut Cursor::new("\n"),
            &mut out,
        )
        .unwrap();
        let out = String::from_utf8(out).unwrap();
        assert!(
            out.contains(&dest.display().to_string()) && out.contains("9 B)"),
            "la celda destructiva lista rutas con tamaños: {out}"
        );

        let mut out = Vec::new();
        let decision = confirm_without_cancel_notice(
            &request(Kind::Destructive, &summary, &entries, false, true),
            &mut Cursor::new("s\n"),
            &mut out,
        )
        .unwrap();
        assert_eq!(decision, Decision::Proceed, "una `s` explícita acepta");

        // Columna "sin terminal", fila no destructiva: procede sin preguntar, y
        // no lee stdin.
        let mut out = Vec::new();
        let decision = confirm_without_cancel_notice(
            &request(Kind::NonDestructive, &summary, &[], false, false),
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
        let err = confirm_without_cancel_notice(
            &request(Kind::Destructive, &summary, &entries, false, false),
            &mut Cursor::new(""),
            &mut out,
        )
        .unwrap_err();
        let failure = err
            .downcast_ref::<LifecycleError>()
            .expect("reason de contrato");
        assert_eq!(failure.reason, "confirmation_required");
        assert_eq!(ExitCode::from_reason(failure.reason).code(), 2);
        let mut out = Vec::new();
        assert_eq!(
            confirm_without_cancel_notice(
                &request(Kind::Destructive, &summary, &entries, true, false),
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
        assert_eq!(ExitCode::from_reason(failure.reason).code(), 2);
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
    }

    /// Destructiva sin terminal y sin `--yes`: `confirmation_required` con
    /// código 2, y nada borrado.
    #[test]
    fn destructive_without_tty_and_without_yes_exits_2() {
        let dir = scratch("confirm-no-tty");
        let data = dir.join("data");
        let model = dir.join("models");
        write_file(&data.join("voice.wav"), "voz sintetizada");
        write_file(&model.join("revision.bin"), &"pesado".repeat(1024));
        let before = snapshot(&dir);

        let entries = vec![PlanEntry::of(&data), PlanEntry::of(&model)];
        let mut out = Vec::new();
        let err = confirm_without_cancel_notice(
            &Confirmation {
                kind: Kind::Destructive,
                question: "¿Borrar lo indicado?",
                summary: &["cleanup borrará modelos y audios sintetizados".to_string()],
                entries: &entries,
                assume_yes: false,
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
        assert_eq!(
            ExitCode::from_reason(failure.reason).code(),
            2,
            "error de uso, no un error genérico"
        );
        assert_eq!(
            failure.message,
            "Esta orden necesita tu confirmación, pero no hay una terminal para preguntarte.",
            "el mensaje no lleva el nombre de la aplicación: la solución completa la orden"
        );
        // Que la respuesta fuera `y` no cambia nada: sin terminal no hay a quién
        // preguntarle, y el criterio es exigir `--yes`.
        assert_eq!(snapshot(&dir), before, "no se borra nada sin `--yes`");
        let printed = String::from_utf8(out.clone()).unwrap();
        assert!(printed.contains("data"), "el plan se imprime: {printed}");

        // Con `--yes` la misma operación procede, y tampoco borra: el borrado lo
        // hace quien ejecuta, después de la decisión.
        assert_eq!(
            confirm_without_cancel_notice(
                &Confirmation {
                    kind: Kind::Destructive,
                    question: "¿Borrar lo indicado?",
                    summary: &[],
                    entries: &entries,
                    assume_yes: true,
                    stdin_is_terminal: false,
                },
                &mut Cursor::new(""),
                &mut out,
            )
            .unwrap(),
            Decision::Proceed
        );
        assert_eq!(snapshot(&dir), before);
    }
}
