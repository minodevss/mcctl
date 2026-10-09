use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::error::Error;
use crate::files;
use crate::names::{Memory, ServerName};
use crate::paths::Paths;
use crate::process;

pub(crate) const ROUTER_UNIT: &str = "mcctl.service";
const MIN_HEADROOM_MIB: u32 = 1024;

fn systemctl(args: &[&str]) -> Result<(), Error> {
    process::run(Command::new("systemctl").args(args)).map(drop)
}

pub(crate) fn enable_now(unit: &str) -> Result<(), Error> {
    systemctl(&["enable", "--now", unit])
}

pub(crate) fn enable(unit: &str) -> Result<(), Error> {
    systemctl(&["enable", unit])
}

// disable --now so a stopped server stays stopped after reboot.
pub(crate) fn disable_now(unit: &str) -> Result<(), Error> {
    systemctl(&["disable", "--now", unit])
}

pub(crate) fn stop(unit: &str) -> Result<(), Error> {
    systemctl(&["stop", unit])
}

pub(crate) fn restart(unit: &str) -> Result<(), Error> {
    systemctl(&["restart", unit])
}

pub(crate) fn reset_failed(unit: &str) -> Result<(), Error> {
    systemctl(&["reset-failed", unit])
}

pub(crate) fn daemon_reload() -> Result<(), Error> {
    systemctl(&["daemon-reload"])
}

/// False also when systemctl is missing.
pub(crate) fn is_active(unit: &str) -> bool {
    process::probe(Command::new("systemctl").args(["is-active", "--quiet", unit]))
        .is_ok_and(|(code, _)| code == Some(0))
}

pub(crate) fn show(unit: &str) -> Result<UnitStatus, Error> {
    let stdout = process::run(Command::new("systemctl").args([
        "show",
        "-p",
        "ActiveState,SubState,UnitFileState,NRestarts",
        unit,
    ]))?;
    Ok(parse_show(&stdout))
}

/// The unit properties `systemctl show` reports, as text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct UnitStatus {
    pub(crate) active: String,
    pub(crate) sub: String,
    pub(crate) file_state: String,
    pub(crate) restarts: u32,
}

impl UnitStatus {
    pub(crate) fn enabled(&self) -> bool {
        self.file_state == "enabled"
    }
}

pub(crate) fn parse_show(text: &str) -> UnitStatus {
    let mut status = UnitStatus::default();
    for (key, value) in text.lines().filter_map(|line| line.split_once('=')) {
        match key {
            "ActiveState" => value.clone_into(&mut status.active),
            "SubState" => value.clone_into(&mut status.sub),
            "UnitFileState" => value.clone_into(&mut status.file_state),
            "NRestarts" => status.restarts = value.trim().parse().unwrap_or(0),
            _ => {}
        }
    }
    status
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ServerState {
    Running,
    Starting,
    Stopping,
    Stopped,
    Failed,
    Restarting,
    Unknown,
}

impl ServerState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Starting => "starting",
            Self::Stopping => "stopping",
            Self::Stopped => "stopped",
            Self::Failed => "failed",
            Self::Restarting => "restarting",
            Self::Unknown => "unknown",
        }
    }
}

impl fmt::Display for ServerState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

pub(crate) fn server_state(status: &UnitStatus) -> ServerState {
    if status.sub.starts_with("auto-restart") {
        return ServerState::Restarting;
    }
    match status.active.as_str() {
        "active" | "reloading" | "refreshing" => ServerState::Running,
        "activating" => ServerState::Starting,
        "deactivating" => ServerState::Stopping,
        "inactive" | "maintenance" => ServerState::Stopped,
        "failed" => ServerState::Failed,
        _ => ServerState::Unknown,
    }
}

/// Heap plus room for the JVM itself: half the heap, at least 1 GiB.
pub(crate) fn memory_dropin(memory: Memory) -> String {
    let heap = memory.mib();
    let limit = heap.saturating_add((heap / 2).max(MIN_HEADROOM_MIB));
    format!("[Service]\nMemoryMax={limit}M\n")
}

/// Writes the memory drop-in for `name`; returns whether it changed and needs a daemon-reload.
pub(crate) fn write_dropin(
    paths: &Paths,
    name: &ServerName,
    memory: Memory,
) -> Result<bool, Error> {
    let path = paths.dropin_file(name);
    let wanted = memory_dropin(memory);
    if files::read_optional(&path)?.as_deref() == Some(wanted.as_str()) {
        return Ok(false);
    }
    let dir = paths.dropin_dir(name);
    fs::create_dir_all(&dir).map_err(Error::io(&dir))?;
    files::write_atomic(&path, wanted.as_bytes(), 0o644)?;
    Ok(true)
}

/// Argv that runs `program` once as `user` in a sandbox that may only write to `dir`.
pub(crate) fn sandboxed_run_argv(user: &str, dir: &Path, program: &[OsString]) -> Vec<OsString> {
    let mut argv: Vec<OsString> = ["systemd-run", "--wait", "--pipe", "--collect", "--quiet"]
        .into_iter()
        .map(OsString::from)
        .collect();
    argv.push(format!("--uid={user}").into());
    argv.push(format!("--gid={user}").into());
    for property in [
        prefixed("WorkingDirectory=", dir),
        "ProtectSystem=strict".into(),
        prefixed("ReadWritePaths=", dir),
        "PrivateTmp=yes".into(),
        "NoNewPrivileges=yes".into(),
        "UMask=0007".into(),
    ] {
        argv.push("-p".into());
        argv.push(property);
    }
    argv.push("--".into());
    argv.extend(program.iter().cloned());
    argv
}

fn prefixed(prefix: &str, path: &Path) -> OsString {
    let mut value = OsString::from(prefix);
    value.push(path);
    value
}

pub(crate) fn run_sandboxed(user: &str, dir: &Path, program: &[OsString]) -> Result<(), Error> {
    let argv = sandboxed_run_argv(user, dir, program);
    let Some((first, rest)) = argv.split_first() else {
        return Ok(());
    };
    let mut command = Command::new(first);
    command.args(rest).stdin(Stdio::null());
    process::run(&mut command).map(drop)
}

pub(crate) fn journal(unit: &str, lines: u32, follow: bool) -> Command {
    let mut command = Command::new("journalctl");
    command.args(["-u", unit, "-o", "cat", "-n"]);
    command.arg(lines.to_string());
    if follow {
        command.arg("-f");
    }
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(active: &str, sub: &str, file_state: &str) -> UnitStatus {
        UnitStatus {
            active: active.to_owned(),
            sub: sub.to_owned(),
            file_state: file_state.to_owned(),
            restarts: 0,
        }
    }

    #[test]
    fn maps_systemctl_show_to_state() {
        let cases = [
            (unit("active", "running", "enabled"), ServerState::Running),
            (
                unit("activating", "start", "enabled"),
                ServerState::Starting,
            ),
            (
                unit("deactivating", "stop-sigterm", "enabled"),
                ServerState::Stopping,
            ),
            (unit("inactive", "dead", "disabled"), ServerState::Stopped),
            (unit("inactive", "dead", "enabled"), ServerState::Stopped),
            (unit("failed", "failed", "enabled"), ServerState::Failed),
            (
                unit("activating", "auto-restart", "enabled"),
                ServerState::Restarting,
            ),
            (
                unit("activating", "auto-restart-queued", "enabled"),
                ServerState::Restarting,
            ),
            (unit("", "", ""), ServerState::Unknown),
        ];
        for (status, expected) in cases {
            assert_eq!(server_state(&status), expected, "{status:?}");
        }
        let shown = parse_show(
            "ActiveState=activating\nSubState=auto-restart\nUnitFileState=enabled\nNRestarts=3\n",
        );
        assert_eq!(
            shown,
            UnitStatus {
                restarts: 3,
                ..unit("activating", "auto-restart", "enabled")
            }
        );
        assert!(shown.enabled());
        assert_eq!(server_state(&shown), ServerState::Restarting);
    }

    #[test]
    fn memory_dropin_has_heap_plus_headroom() {
        let cases = [
            ("1G", "[Service]\nMemoryMax=2048M\n"),
            ("2G", "[Service]\nMemoryMax=3072M\n"),
            ("4G", "[Service]\nMemoryMax=6144M\n"),
            ("8G", "[Service]\nMemoryMax=12288M\n"),
            ("1536M", "[Service]\nMemoryMax=2560M\n"),
        ];
        for (memory, expected) in cases {
            assert_eq!(memory_dropin(memory.parse().unwrap()), expected, "{memory}");
        }
    }

    #[test]
    fn writes_dropin_only_when_it_changes() {
        let root = tempfile::tempdir().unwrap();
        let paths = Paths::under(root.path());
        let name: ServerName = "survival".parse().unwrap();
        let memory: Memory = "8G".parse().unwrap();
        assert!(write_dropin(&paths, &name, memory).unwrap());
        assert!(!write_dropin(&paths, &name, memory).unwrap());
        assert!(write_dropin(&paths, &name, "4G".parse().unwrap()).unwrap());
    }

    #[test]
    fn sandboxes_installer_to_the_server_folder() {
        let argv = sandboxed_run_argv(
            "mc-survival",
            Path::new("/var/lib/mcctl/servers/.staging-survival-7"),
            &["/java".into(), "-jar".into(), "installer.jar".into()],
        );
        let argv: Vec<&str> = argv.iter().map(|arg| arg.to_str().unwrap()).collect();
        assert_eq!(
            argv,
            [
                "systemd-run",
                "--wait",
                "--pipe",
                "--collect",
                "--quiet",
                "--uid=mc-survival",
                "--gid=mc-survival",
                "-p",
                "WorkingDirectory=/var/lib/mcctl/servers/.staging-survival-7",
                "-p",
                "ProtectSystem=strict",
                "-p",
                "ReadWritePaths=/var/lib/mcctl/servers/.staging-survival-7",
                "-p",
                "PrivateTmp=yes",
                "-p",
                "NoNewPrivileges=yes",
                "-p",
                "UMask=0007",
                "--",
                "/java",
                "-jar",
                "installer.jar",
            ]
        );
    }
}
