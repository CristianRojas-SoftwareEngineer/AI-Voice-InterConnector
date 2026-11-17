// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! Consola del proceso: la propiedad de la línea de fase de stderr y los
//! avisos.
//!
//! La implementación vive en `avi_term::line`; este módulo solo la reexporta
//! para los crates que ya dependían de `avi_shared::console`.

pub use avi_term::line::{phase_line_open, set_phase_line_open, take_phase_line, warn};
