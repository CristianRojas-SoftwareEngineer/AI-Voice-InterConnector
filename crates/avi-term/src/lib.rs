// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! Núcleo de presentación en terminal de la CLI: el único sitio que da formato
//! a duraciones, tamaños y plurales, emite color, gestiona la línea de fase y
//! dibuja las etapas de trabajo.
//!
//! Crate hoja: no depende de ningún otro crate del workspace. Su única
//! dependencia de sistema es `windows-sys`, solo en Windows.
//!
//! - [`fmt`]: formateadores puros (duraciones, tamaños, plurales, medidor).
//! - [`style`]: paleta y reglas del color.
//! - [`line`]: estado global de la salida y líneas de contexto, aviso y error.
//! - [`stage`]: catálogo único de etapas.
//! - [`progress`]: el indicador de fases [`progress::PhaseProgress`].

#![cfg_attr(not(test), deny(clippy::let_underscore_must_use))]

pub mod fmt;
pub mod line;
pub mod progress;
pub mod stage;
pub mod style;
