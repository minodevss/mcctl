use std::collections::BTreeSet;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use crate::remembered::Writer;
use crate::{Desired, Error};

pub(crate) const TOKEN_ENV: &str = "MCCTL_DNS_TOKEN";
const TIMEOUT: Duration = Duration::from_secs(30);
const POLL: Duration = Duration::from_millis(50);
const STDERR_GRACE: Duration = Duration::from_secs(1);
const STDERR_TAIL_BYTES: usize = 8 * 1024;
const STDERR_LINE_CHARS: usize = 300;

/// Runs `<command> upsert <name> <ip>` and `<command> delete <name>`; the token travels only in the environment.
pub(crate) struct Exec {
    command: PathBuf,
    token: Option<String>,
}

impl Exec {
    pub(crate) fn new(command: &Path, token: Option<&str>) -> Self {
        Self {
            command: command.to_owned(),
            token: token.map(str::to_owned),
        }
    }

    fn run(&self, args: &[&str]) -> Result<(), Error> {
        let mut command = Command::new(&self.command);
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        if let Some(token) = &self.token {
            command.env(TOKEN_ENV, token);
        }
        let mut child = command.spawn().map_err(|err| self.io_error(&err))?;
        let stderr = read_stderr(&mut child);
        match wait_until(&mut child, TIMEOUT).map_err(|err| self.io_error(&err))? {
            None => Err(Error::HookTimeout {
                command: self.command.clone(),
                timeout: TIMEOUT,
            }),
            Some(status) if status.success() => Ok(()),
            Some(status) => Err(Error::HookFailed {
                command: self.command.clone(),
                status: status.to_string(),
                stderr: self.last_line(&stderr.recv_timeout(STDERR_GRACE).unwrap_or_default()),
            }),
        }
    }

    fn io_error(&self, err: &io::Error) -> Error {
        Error::HookIo {
            command: self.command.clone(),
            reason: err.to_string(),
        }
    }

    fn last_line(&self, stderr: &str) -> String {
        let line = stderr
            .lines()
            .map(str::trim)
            .rfind(|line| !line.is_empty())
            .unwrap_or("no output");
        let line: String = line.chars().take(STDERR_LINE_CHARS).collect();
        match &self.token {
            Some(token) if !token.is_empty() => line.replace(token.as_str(), "***"),
            _ => line,
        }
    }
}

impl Writer for Exec {
    fn write(&self, record: &Desired) -> Result<(), Error> {
        self.run(&["upsert", &record.name, &record.ip.to_string()])
    }

    fn erase(&self, name: &str, _remaining: &BTreeSet<String>) -> Result<(), Error> {
        self.run(&["delete", name])
    }
}

/// Kills the child once `timeout` passes and returns `None`.
fn wait_until(child: &mut Child, timeout: Duration) -> io::Result<Option<ExitStatus>> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if Instant::now() >= deadline {
            child.kill()?;
            child.wait()?;
            return Ok(None);
        }
        thread::sleep(POLL);
    }
}

// A thread drains stderr so a chatty hook never blocks on a full pipe before it exits.
fn read_stderr(child: &mut Child) -> Receiver<String> {
    let (sender, receiver) = mpsc::channel();
    if let Some(stderr) = child.stderr.take() {
        thread::spawn(move || sender.send(read_tail(stderr)));
    }
    receiver
}

fn read_tail(mut reader: impl Read) -> String {
    let mut tail = Vec::new();
    let mut chunk = [0_u8; 4096];
    while let Ok(read @ 1..) = reader.read(&mut chunk) {
        tail.extend_from_slice(&chunk[..read]);
        let excess = tail.len().saturating_sub(STDERR_TAIL_BYTES);
        tail.drain(..excess);
    }
    String::from_utf8_lossy(&tail).into_owned()
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::{Mutex, MutexGuard};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    static SPAWNING: Mutex<()> = Mutex::new(());

    // Serialized so no test forks while another still holds its script open for writing (ETXTBSY).
    fn spawn_lock() -> MutexGuard<'static, ()> {
        SPAWNING
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn scratch_dir(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("mcctl-dns-{tag}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn exec_passes_token_in_env_not_argv() {
        let _spawning = spawn_lock();
        let dir = scratch_dir("exec");
        let script = dir.join("hook.sh");
        let script_body = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" >> '{args}'\nprintf '%s' \"${TOKEN_ENV}\" > '{env}'\n",
            args = dir.join("args").display(),
            env = dir.join("env").display(),
        );
        fs::write(&script, script_body).unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();

        let hook = Exec::new(&script, Some("hook-secret"));
        hook.write(&Desired {
            name: "survival.example.com".into(),
            ip: [1, 2, 3, 4].into(),
        })
        .unwrap();
        hook.erase("survival.example.com", &BTreeSet::new())
            .unwrap();

        let args = fs::read_to_string(dir.join("args")).unwrap();
        let env = fs::read_to_string(dir.join("env")).unwrap();
        fs::remove_dir_all(&dir).unwrap();
        assert_eq!(
            args,
            "upsert\nsurvival.example.com\n1.2.3.4\ndelete\nsurvival.example.com\n"
        );
        assert_eq!(env, "hook-secret");
    }

    #[test]
    fn exec_reports_failure_without_token() {
        let _spawning = spawn_lock();
        let dir = scratch_dir("exec-fail");
        let script = dir.join("hook.sh");
        fs::write(
            &script,
            format!("#!/bin/sh\necho \"bad token ${TOKEN_ENV}\" >&2\nexit 3\n"),
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();

        let err = Exec::new(&script, Some("hook-secret")).write(&Desired {
            name: "a.example.com".into(),
            ip: [1, 2, 3, 4].into(),
        });
        fs::remove_dir_all(&dir).unwrap();
        let Err(Error::HookFailed { stderr, .. }) = err else {
            panic!("expected a hook failure, got {err:?}");
        };
        assert_eq!(stderr, "bad token ***");
    }

    #[test]
    fn exec_kills_hook_after_timeout() {
        let _spawning = spawn_lock();
        let mut child = Command::new("sleep").arg("5").spawn().unwrap();
        let started = Instant::now();
        assert_eq!(
            wait_until(&mut child, Duration::from_millis(100)).unwrap(),
            None
        );
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn keeps_only_the_stderr_tail() {
        let noisy = format!("{}\nlast words\n", "x".repeat(STDERR_TAIL_BYTES * 3));
        let tail = read_tail(noisy.as_bytes());
        assert_eq!(tail.len(), STDERR_TAIL_BYTES);
        assert!(tail.ends_with("last words\n"));
    }
}
