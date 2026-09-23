//! On-demand lifecycle for the WSL2 SearXNG stack.
//!
//! The MCP boots the stack only when a `search` hits a connection error
//! (zero overhead when it is already running) and arms an idle watchdog
//! only after successful searches, so a stack the MCP never touched is
//! never terminated.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

use crate::config::Config;
use crate::error::ArgosError;

/// Convert an absolute Windows path (`D:\a\b`) to its WSL form (`/mnt/d/a/b`).
/// Unix-style paths pass through unchanged; anything else yields `None`.
pub fn win_to_wsl(path: &str) -> Option<String> {
    let path = path.trim();
    if path.starts_with('/') {
        return Some(path.to_string());
    }
    let bytes = path.as_bytes();
    let has_drive = bytes.len() > 2 && bytes[1] == b':';
    let is_abs = has_drive && (bytes[2] == b'\\' || bytes[2] == b'/');
    if is_abs {
        let drive = char::from(bytes[0]).to_ascii_lowercase();
        let rest = path[2..].replace('\\', "/");
        return Some(format!("/mnt/{drive}{rest}"));
    }
    None
}

fn last_used() -> &'static Mutex<Option<Instant>> {
    static LAST_USED: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();
    LAST_USED.get_or_init(|| Mutex::new(None))
}

/// Boot the stack on demand: `wsl-setup.sh --up` (start daemon + compose up + wait for the API).
///
/// On timeout the child keeps running detached so the boot completes in the
/// background; the caller surfaces [`ArgosError::StackStarting`] and the next
/// search is expected to succeed.
pub async fn ensure_up(config: &Config) -> Result<(), ArgosError> {
    let script = config.stack_script.to_string_lossy().to_string();
    let unix_script = win_to_wsl(&script).ok_or_else(|| ArgosError::StackBoot {
        detail: format!("cannot convert stack script path to WSL: {script}"),
    })?;

    let mut child = tokio::process::Command::new("wsl")
        .args([
            "-d",
            config.wsl_distro.as_str(),
            "-u",
            "root",
            "-e",
            "bash",
            unix_script.as_str(),
            "--up",
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|ex| ArgosError::StackBoot {
            detail: format!("cannot spawn wsl: {ex}"),
        })?;

    match tokio::time::timeout(config.boot_timeout, child.wait()).await {
        Ok(Ok(status)) if status.success() => Ok(()),
        Ok(Ok(status)) => Err(ArgosError::StackBoot {
            detail: format!("exited with code {}", status.code().unwrap_or(-1)),
        }),
        Ok(Err(ex)) => Err(ArgosError::StackBoot {
            detail: format!("wait failed: {ex}"),
        }),
        Err(_) => Err(ArgosError::StackStarting {
            secs: config.boot_timeout.as_secs(),
        }),
    }
}

/// Record successful usage and arm the one-shot idle watchdog.
pub async fn note_usage(config: &Config) {
    if config.idle_stop.is_zero() {
        return;
    }
    *last_used().lock().await = Some(Instant::now());

    static WATCHDOG: OnceLock<()> = OnceLock::new();
    if WATCHDOG.set(()).is_ok() {
        tokio::spawn(watchdog(config.wsl_distro.clone(), config.idle_stop));
    }
}

async fn watchdog(distro: String, idle_stop: Duration) {
    loop {
        tokio::time::sleep(Duration::from_secs(30)).await;
        let mut stamp = last_used().lock().await;
        if let Some(used_at) = *stamp {
            if used_at.elapsed() >= idle_stop {
                // Clear the stamp first: only one terminate per usage cycle.
                *stamp = None;
                drop(stamp);
                terminate(&distro).await;
            }
        }
    }
}

/// Terminate the distro (stops its docker daemon and containers too).
pub async fn terminate(distro: &str) {
    let _ = tokio::process::Command::new("wsl")
        .args(["--terminate", distro])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_windows_paths_with_spaces() {
        assert_eq!(
            win_to_wsl(r"D:\Mis Juegos\ClaudeMCPs\argos-engine\searxng\wsl-setup.sh"),
            Some("/mnt/d/Mis Juegos/ClaudeMCPs/argos-engine/searxng/wsl-setup.sh".to_string())
        );
    }

    #[test]
    fn converts_forward_slash_windows_paths() {
        assert_eq!(
            win_to_wsl("C:/repo/searxng/wsl-setup.sh"),
            Some("/mnt/c/repo/searxng/wsl-setup.sh".to_string())
        );
    }

    #[test]
    fn upper_case_drive_is_lowercased() {
        assert_eq!(
            win_to_wsl(r"E:\data\script.sh"),
            Some("/mnt/e/data/script.sh".to_string())
        );
    }

    #[test]
    fn unix_paths_pass_through() {
        assert_eq!(
            win_to_wsl("/mnt/d/whatever.sh"),
            Some("/mnt/d/whatever.sh".to_string())
        );
    }

    #[test]
    fn relative_paths_are_rejected() {
        assert_eq!(win_to_wsl("searxng/wsl-setup.sh"), None);
        assert_eq!(win_to_wsl(""), None);
    }
}
