// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! Direcciones de red de las pruebas. Lo declaran con
//! `#[path = "support/net.rs"] mod net;`.

/// Puerto efímero que se enlaza y se suelta: garantiza que no hay nada escuchando,
/// sin depender de que el puerto por defecto esté libre en la máquina que ejecuta.
pub fn dead_port() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("se enlaza un puerto");
    let addr = listener.local_addr().expect("dirección local");
    drop(listener);
    addr.to_string()
}

/// URL `http://127.0.0.1:<puerto>` de un oyente que acepta cada conexión y la
/// cierra sin responder, durante toda la prueba: el cliente HTTP falla al
/// instante con un error de transporte. No se usa un puerto cerrado porque
/// Windows tarda ~2 s en rechazar cada intento de conexión, ni uno retenido sin
/// oyente porque en macOS la conexión cuelga hasta el timeout TCP.
#[allow(dead_code)]
pub fn closing_endpoint() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("se enlaza un puerto");
    let addr = listener.local_addr().expect("dirección local");
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            drop(stream);
        }
    });
    format!("http://{addr}")
}
