//! Control de procesos inerte. Lo declaran con `#[path = "support/inert.rs"] mod inert;`.

use avi_lifecycle::daemon_stop::ProcessControl;

/// Control de procesos inerte: no hay daemon en el sandbox, así que la parada es un
/// no-op. Es el mismo `ProcessControl` que `daemon_stop` espera y que el binario
/// alimenta con `avi-daemon` y `avi-tts`.
pub struct Inert;

impl ProcessControl for Inert {
    fn pid_alive(&self, _pid: u32) -> bool {
        false
    }
    fn kill_tree_by_pid(&self, _pid: u32) -> bool {
        false
    }
    fn resident_pid_alive(&self, _pid: u32) -> bool {
        false
    }
    fn kill_tree_resident_by_pid(&self, _pid: u32) -> bool {
        false
    }
}
