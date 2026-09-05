//! Relaunching the application with administrative rights.
//!
//! Reading a disk or a USB stick sector by sector is reserved to
//! administrators by the operating system, so a user who started PhoinixDR
//! normally sees devices as "not accessible". Rather than asking them to
//! find the *Run as administrator* menu, the interface offers a button that
//! starts an elevated copy of this executable through the platform's own
//! prompt: UAC on Windows (`Start-Process -Verb RunAs`), polkit on Linux
//! (`pkexec`). Nothing here needs `unsafe`; both mechanisms are reached
//! through ordinary child processes.

use std::process::Command;

/// Whether this process already runs with administrative rights.
pub fn is_elevated() -> bool {
    platform::is_elevated()
}

/// Starts an elevated copy of this executable after the operating system's
/// consent prompt.
///
/// Returns `Ok(true)` when the elevated copy has been started and this
/// instance should exit (Windows: `Start-Process` returns once the new
/// process exists), `Ok(false)` when the prompt has been opened but this
/// instance should stay (Linux: `pkexec` keeps running as the parent of the
/// elevated copy, so success cannot be told apart from a pending prompt
/// without waiting for the user), and `Err` with a message for the user when
/// they declined or the mechanism is unavailable.
pub fn relaunch_elevated() -> Result<bool, String> {
    let exe = std::env::current_exe()
        .map_err(|e| format!("cannot determine the path of the executable: {e}"))?;
    platform::relaunch(Command::new(platform::LAUNCHER), &exe)
}

#[cfg(windows)]
mod platform {
    use std::os::windows::process::CommandExt;
    use std::path::Path;
    use std::process::{Command, Stdio};

    /// Do not open a console window for the helper processes.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    pub const LAUNCHER: &str = "powershell.exe";

    /// `fltmc.exe` (the filter manager control tool, present on every
    /// supported Windows) refuses to run without administrative rights, so
    /// its exit status tells whether this process is elevated.
    pub fn is_elevated() -> bool {
        Command::new("fltmc.exe")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// `Start-Process -Verb RunAs` raises the UAC prompt and returns once
    /// the elevated process exists; it fails with "canceled by the user"
    /// when the prompt is declined.
    pub fn relaunch(mut powershell: Command, exe: &Path) -> Result<bool, String> {
        let path = exe.to_string_lossy().replace('\'', "''");
        let script = format!("Start-Process -FilePath '{path}' -Verb RunAs");
        let output = powershell
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-WindowStyle",
                "Hidden",
                "-Command",
                &script,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .map_err(|e| format!("cannot start PowerShell to request elevation: {e}"))?;
        if output.status.success() {
            return Ok(true);
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.to_ascii_lowercase().contains("cancel") {
            return Err("The request for administrator rights was cancelled.".to_owned());
        }
        let detail = stderr
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .trim();
        Err(format!("Windows did not start an elevated copy: {detail}"))
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use std::path::Path;
    use std::process::{Command, Stdio};

    pub const LAUNCHER: &str = "pkexec";

    /// The effective user id from `/proc/self/status` (the second field of
    /// the `Uid:` line); root is 0.
    pub fn is_elevated() -> bool {
        std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|status| {
                status
                    .lines()
                    .find_map(|line| line.strip_prefix("Uid:"))
                    .and_then(|ids| ids.split_whitespace().nth(1).map(|euid| euid == "0"))
            })
            .unwrap_or(false)
    }

    /// `pkexec` shows the polkit password dialog and then runs the program
    /// as root. It scrubs the environment, so the variables the display
    /// server and WebKitGTK need are passed through `env` explicitly.
    pub fn relaunch(mut pkexec: Command, exe: &Path) -> Result<bool, String> {
        pkexec.arg("env");
        for var in [
            "DISPLAY",
            "XAUTHORITY",
            "WAYLAND_DISPLAY",
            "XDG_RUNTIME_DIR",
            "XDG_SESSION_TYPE",
            "DBUS_SESSION_BUS_ADDRESS",
            "GDK_BACKEND",
            "WEBKIT_DISABLE_COMPOSITING_MODE",
            "WEBKIT_DISABLE_DMABUF_RENDERER",
        ] {
            if let Ok(value) = std::env::var(var) {
                pkexec.arg(format!("{var}={value}"));
            }
        }
        pkexec
            .arg(exe)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("cannot start pkexec (is polkit installed?): {e}"))?;
        Ok(false)
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod platform {
    use std::path::Path;
    use std::process::Command;

    pub const LAUNCHER: &str = "";

    pub fn is_elevated() -> bool {
        false
    }

    pub fn relaunch(_launcher: Command, _exe: &Path) -> Result<bool, String> {
        Err("Restarting with administrator rights is not supported on this platform; start PhoinixDR with sudo.".to_owned())
    }
}
