use std::process::{Command, Stdio};

/// Lanza el daemon en segundo plano desacoplado del proceso padre.
///
/// Técnica A exacta: `Stdio::null` + Windows `DETACHED_PROCESS|CREATE_NEW_PROCESS_GROUP`
/// + Unix `pre_exec(setsid)` con `libc`.
pub fn spawn_background() -> anyhow::Result<u32> {
    let exe = std::env::current_exe()?;
    let mut cmd = Command::new(exe);
    cmd.arg("daemon")
        .arg("serve")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x00000008 | 0x00000200);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }
    let child = cmd.spawn()?;
    Ok(child.id())
}
