// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! Borrado diferido de rutas en Windows, sin copiar el ejecutable propio.
//!
//! Quien espera y borra es el Windows PowerShell 5.1 del sistema, que existe en
//! toda instalación de Windows y no depende del binario que programa.
//!
//! El script es una constante estática: nada del entorno se interpola en su
//! texto. El destino y la identidad esperada viajan por variables de entorno del
//! proceso lanzado, así que ninguna ruta, por rara que sea, se interpreta como
//! código.
//!
//! La espera es por identidad, no por PID: el script compara el instante de
//! creación del proceso (`StartTime`, expresado como FILETIME) con el del token.
//! Un PID reutilizado por otro proceso tiene otro inicio y cuenta como muerto.
//! Tras morir el proceso esperado, el script borra el destino con un número fijo
//! de reintentos y, solo si fallan todos, escribe `<destino>.removal-failed.log`
//! junto al destino. Un borrado correcto no deja ningún registro.
//!
//! El lanzamiento intenta separar el proceso del Job Object del padre, para que
//! sobreviva a su muerte. Si el Job no lo permite, se lanza sin separar y el
//! resultado lo dice ([`Scheduling::Attached`]); este módulo no escribe nada por
//! stdout ni stderr. Sin separación, el limpiador sobrevive al padre solo si el
//! Job no mata a sus procesos al cerrarse; cada consumidor decide qué decirle al
//! usuario.

use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use avi_shared::OwnerToken;

use crate::{Scheduling, CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW};

/// Bandera de creación que separa el proceso del Job Object del padre.
const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;

/// Código de Windows para acceso denegado: es el que devuelve `CreateProcess`
/// cuando el Job Object del padre no permite separar al hijo.
const ERROR_ACCESS_DENIED: i32 = 5;

/// Espera fija tras lanzar el script antes de darlo por programado. Si en ese
/// plazo ya terminó con código distinto de cero, programar no sirvió.
const START_GRACE: Duration = Duration::from_millis(1000);

/// Intervalo de sondeo del arranque del script.
const START_POLL: Duration = Duration::from_millis(25);

/// Script del limpiador. Estático: recibe `AVI_REMOVAL_TARGET`,
/// `AVI_REMOVAL_PID` y `AVI_REMOVAL_START` (FILETIME decimal) por entorno.
///
/// Sin comillas dobles ni saltos de línea significativos, para que la línea de
/// comandos de `-Command` no necesite escapes.
const REMOVAL_SCRIPT: &str = "\
$ErrorActionPreference = 'Stop'; \
$target = $env:AVI_REMOVAL_TARGET; \
$waitPid = [int]$env:AVI_REMOVAL_PID; \
$waitStart = [long]$env:AVI_REMOVAL_START; \
while ($true) { \
  $p = Get-Process -Id $waitPid -ErrorAction SilentlyContinue; \
  if ($null -eq $p) { break }; \
  try { \
    if ($p.HasExited -or $p.StartTime.ToFileTimeUtc() -ne $waitStart) { break } \
  } catch { }; \
  Start-Sleep -Milliseconds 250 \
}; \
$failure = 'la ruta sigue existiendo'; \
for ($i = 0; $i -lt 10; $i++) { \
  try { \
    if (Test-Path -LiteralPath $target) { Remove-Item -LiteralPath $target -Recurse -Force }; \
    if (-not (Test-Path -LiteralPath $target)) { $failure = $null; break } \
  } catch { $failure = $_.Exception.Message }; \
  Start-Sleep -Milliseconds 500 \
}; \
if ($null -ne $failure) { \
  Set-Content -LiteralPath ($target + '.removal-failed.log') -Encoding UTF8 -Value ('no se pudo retirar ' + $target + ' tras 10 intentos: ' + $failure); \
  exit 1 \
}";

/// Ruta de `powershell.exe` bajo `%SystemRoot%`, absoluta: nunca se busca en el
/// `PATH` ni en el directorio actual.
fn system_powershell() -> anyhow::Result<PathBuf> {
    let root = std::env::var_os("SystemRoot")
        .ok_or_else(|| anyhow::anyhow!("la variable SystemRoot no está definida"))?;
    Ok(PathBuf::from(root)
        .join("System32")
        .join("WindowsPowerShell")
        .join("v1.0")
        .join("powershell.exe"))
}

/// Programa el borrado de `target` para cuando muera el proceso `wait`, con el
/// PowerShell del sistema.
///
/// Devuelve `Ok` cuando el script está en marcha, y dice cómo quedó el
/// limpiador. Si no se puede lanzar (`powershell.exe` ausente, bloqueo de
/// directiva como AppLocker) o termina con fallo durante la espera de arranque,
/// devuelve error sin haber tocado el destino.
pub fn schedule_removal(target: &Path, wait: &OwnerToken) -> anyhow::Result<Scheduling> {
    schedule_removal_with(&system_powershell()?, target, wait)
}

/// Igual que [`schedule_removal`] con la ruta de `powershell.exe` inyectada, para
/// probar el fallo de lanzamiento. `Ok` dice cómo quedó el limpiador.
pub fn schedule_removal_with(
    powershell: &Path,
    target: &Path,
    wait: &OwnerToken,
) -> anyhow::Result<Scheduling> {
    if target.parent().is_none_or(|p| p.as_os_str().is_empty()) {
        anyhow::bail!(
            "el destino del borrado diferido no tiene directorio padre: {}",
            target.display()
        );
    }
    let start: u64 = wait.start().parse().map_err(|_| {
        anyhow::anyhow!(
            "el inicio del token {} no es un FILETIME decimal",
            wait.format()
        )
    })?;

    let launch = |flags: u32| {
        Command::new(powershell)
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                REMOVAL_SCRIPT,
            ])
            .env("AVI_REMOVAL_TARGET", target)
            .env("AVI_REMOVAL_PID", wait.pid().to_string())
            .env("AVI_REMOVAL_START", start.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP | flags)
            .spawn()
    };
    let (mut child, scheduling) =
        launch_detached_or_attached(launch).map_err(|e| launch_error(powershell, &e))?;

    // Espera de arranque fija: un script que muere enseguida con fallo no
    // programó nada.
    let deadline = Instant::now() + START_GRACE;
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait()? {
            if !status.success() {
                anyhow::bail!("el borrado diferido terminó con fallo al arrancar ({status})");
            }
            break;
        }
        std::thread::sleep(START_POLL);
    }
    Ok(scheduling)
}

/// Lanza con `launch` pidiendo separarse del Job del padre. Si el Job lo
/// rechaza con acceso denegado, reintenta una vez sin la bandera y lo informa
/// como [`Scheduling::Attached`]; cualquier otro error se devuelve tal cual.
fn launch_detached_or_attached(
    launch: impl Fn(u32) -> std::io::Result<Child>,
) -> std::io::Result<(Child, Scheduling)> {
    match launch(CREATE_BREAKAWAY_FROM_JOB) {
        Ok(child) => Ok((child, Scheduling::Detached)),
        Err(e) if e.raw_os_error() == Some(ERROR_ACCESS_DENIED) => {
            launch(0).map(|child| (child, Scheduling::Attached))
        }
        Err(e) => Err(e),
    }
}

fn launch_error(powershell: &Path, e: &std::io::Error) -> anyhow::Error {
    anyhow::anyhow!(
        "no se pudo lanzar el borrado diferido con {}: {e}",
        powershell.display()
    )
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    /// Lanza un proceso inocuo que termina enseguida.
    fn harmless_child() -> std::io::Result<Child> {
        Command::new("cmd")
            .args(["/C", "exit 0"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
    }

    #[test]
    fn accepted_breakaway_is_detached() {
        let calls = Cell::new(0);
        let (mut child, scheduling) = launch_detached_or_attached(|flags| {
            calls.set(calls.get() + 1);
            assert_eq!(flags, CREATE_BREAKAWAY_FROM_JOB);
            harmless_child()
        })
        .unwrap();
        child.wait().unwrap();
        assert_eq!(scheduling, Scheduling::Detached);
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn access_denied_retries_without_flag_and_is_attached() {
        let calls = Cell::new(0);
        let (mut child, scheduling) = launch_detached_or_attached(|flags| {
            calls.set(calls.get() + 1);
            if flags == CREATE_BREAKAWAY_FROM_JOB {
                Err(std::io::Error::from_raw_os_error(ERROR_ACCESS_DENIED))
            } else {
                harmless_child()
            }
        })
        .unwrap();
        child.wait().unwrap();
        assert_eq!(scheduling, Scheduling::Attached);
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn other_error_fails_without_retry() {
        let calls = Cell::new(0);
        let result = launch_detached_or_attached(|_| {
            calls.set(calls.get() + 1);
            Err(std::io::Error::from_raw_os_error(2))
        });
        assert!(result.is_err());
        assert_eq!(calls.get(), 1);
    }
}
