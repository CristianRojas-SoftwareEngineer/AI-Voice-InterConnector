// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! Conexiones TCP a procesos locales con fallo rápido para las sondas.
//!
//! Windows no da por rechazada una conexión al primer RST: reintenta el SYN
//! durante unos 2 s antes de devolver `ConnectionRefused`, tanto contra un
//! puerto sin oyente como contra un oyente con la cola llena. Una sonda que
//! pregunta si un proceso local responde agota así su plazo entero cuando la
//! respuesta es «no». Con [`Connection::Probe`] el socket se configura antes de
//! conectar para no reintentar el SYN, y el rechazo llega al instante.
//!
//! El fallo rápido es solo para sondas. Ese reintento es también lo que deja
//! entrar a una conexión en una cola que se vacía en menos de 2 s, así que una
//! petición de trabajo usa [`Connection::Work`] y conecta como siempre. Una
//! sonda que falla contra una cola llena solo da un falso «no responde», que
//! quien sondea ya tolera.
//!
//! En Unix el rechazo ya es inmediato y ambas conexiones son iguales.

use std::io;
use std::net::{SocketAddr, TcpStream};
#[cfg(unix)]
use std::os::fd::AsFd;
#[cfg(windows)]
use std::os::windows::io::AsSocket;
use std::time::Duration;

use socket2::{Domain, Protocol, Socket, Type};

/// Uso de una conexión a un proceso local.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Connection {
    /// Pregunta si el proceso responde: un rechazo falla al instante.
    Probe,
    /// Petición de trabajo: conserva los reintentos de conexión del sistema.
    Work,
}

/// Prepara un socket todavía sin conectar para el uso `connection`.
///
/// En Windows, con [`Connection::Probe`], desactiva los reintentos de SYN del
/// socket y devuelve el error del sistema si no puede; nunca recurre a una
/// conexión normal. Con [`Connection::Work`] no hace nada.
#[cfg(windows)]
pub fn prepare<S: AsSocket>(socket: &S, connection: Connection) -> io::Result<()> {
    use std::os::windows::io::AsRawSocket;
    use windows_sys::Win32::Networking::WinSock::{
        WSAIoctl, SIO_TCP_INITIAL_RTO, TCP_INITIAL_RTO_NO_SYN_RETRANSMISSIONS,
        TCP_INITIAL_RTO_PARAMETERS,
    };

    if connection == Connection::Work {
        return Ok(());
    }
    let params = TCP_INITIAL_RTO_PARAMETERS {
        // `TCP_INITIAL_RTO_UNSPECIFIED_RTT`: conserva el RTO inicial del sistema.
        Rtt: u16::MAX,
        // `windows-sys` declara la constante como `u16` (0xFFFE), pero el campo
        // es `u8`: el truncado da 0xFE, el `(UCHAR)-2` del header de Windows.
        MaxSynRetransmissions: TCP_INITIAL_RTO_NO_SYN_RETRANSMISSIONS as u8,
    };
    let mut returned = 0u32;
    // SAFETY: el socket es válido mientras dure el préstamo, el búfer de
    // entrada apunta a `params` con su tamaño exacto, no hay búfer de salida y
    // la llamada es síncrona (sin `OVERLAPPED` ni rutina de finalización).
    let status = unsafe {
        WSAIoctl(
            socket.as_socket().as_raw_socket() as usize,
            SIO_TCP_INITIAL_RTO,
            std::ptr::addr_of!(params).cast(),
            std::mem::size_of::<TCP_INITIAL_RTO_PARAMETERS>() as u32,
            std::ptr::null_mut(),
            0,
            &mut returned,
            std::ptr::null_mut(),
            None,
        )
    };
    if status != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Prepara un socket todavía sin conectar para el uso `connection`. En Unix el
/// rechazo de una conexión ya es inmediato y no hay nada que preparar.
#[cfg(unix)]
pub fn prepare<S: AsFd>(_socket: &S, _connection: Connection) -> io::Result<()> {
    Ok(())
}

/// Conecta a `addr` con un plazo de conexión `timeout`, preparando antes el
/// socket para el uso `connection`.
pub fn connect_timeout(
    addr: &SocketAddr,
    timeout: Duration,
    connection: Connection,
) -> io::Result<TcpStream> {
    let socket = Socket::new(
        Domain::for_address(*addr),
        Type::STREAM,
        Some(Protocol::TCP),
    )?;
    prepare(&socket, connection)?;
    socket.connect_timeout(&(*addr).into(), timeout)?;
    Ok(socket.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::time::Instant;

    /// Cota de un rechazo inmediato; sin fallo rápido, Windows tarda ~2 s.
    const FAST: Duration = Duration::from_millis(200);

    #[test]
    fn probe_to_freed_port_is_refused_fast() {
        let addr = TcpListener::bind("127.0.0.1:0")
            .and_then(|listener| listener.local_addr())
            .expect("reservar un puerto local");
        let started = Instant::now();
        let error = connect_timeout(&addr, Duration::from_secs(1), Connection::Probe)
            .expect_err("un puerto liberado no tiene oyente");
        let elapsed = started.elapsed();
        assert_eq!(error.kind(), io::ErrorKind::ConnectionRefused, "{error}");
        assert!(elapsed < FAST, "el rechazo tardó {elapsed:?}");
    }

    #[test]
    fn probe_to_listening_port_connects() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("abrir un oyente local");
        let addr = listener.local_addr().expect("dirección del oyente");
        connect_timeout(&addr, Duration::from_secs(1), Connection::Probe)
            .expect("un oyente activo acepta la sonda");
    }

    /// Windows también rechaza con RST cuando la cola del oyente está llena, y
    /// sin fallo rápido reintenta el SYN hasta que la cola tiene sitio. Por eso
    /// el fallo rápido es solo para sondas: una petición de trabajo debe poder
    /// entrar en una cola que se vacía.
    #[cfg(windows)]
    #[test]
    fn probe_to_full_backlog_is_refused_fast() {
        let listener = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))
            .expect("crear el socket del oyente");
        let bind: SocketAddr = "127.0.0.1:0".parse().expect("dirección de loopback");
        listener.bind(&bind.into()).expect("enlazar el oyente");
        listener.listen(1).expect("escuchar con cola de 1");
        let addr = listener
            .local_addr()
            .ok()
            .and_then(|addr| addr.as_socket())
            .expect("dirección del oyente");

        // El oyente nunca acepta: las sondas llenan la cola hasta que una se
        // rechaza.
        let mut queued = Vec::new();
        for _ in 0..16 {
            let started = Instant::now();
            match connect_timeout(&addr, Duration::from_secs(5), Connection::Probe) {
                Ok(stream) => queued.push(stream),
                Err(error) => {
                    let elapsed = started.elapsed();
                    assert_eq!(error.kind(), io::ErrorKind::ConnectionRefused, "{error}");
                    assert!(elapsed < FAST, "el rechazo tardó {elapsed:?}");
                    return;
                }
            }
        }
        panic!("la cola admitió {} conexiones sin rechazar", queued.len());
    }
}
