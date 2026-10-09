use std::ffi::OsString;
use std::io::{self, BufRead, Read, Write};
use std::os::unix::fs::FileTypeExt;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::mpsc;

use crate::commands::Ctx;
use crate::error::Error;
use crate::files::{self, Owner};
use crate::names::ServerName;
use crate::output;
use crate::process;
use crate::systemd;
use crate::users;

/// Writes up to this size are atomic on a pipe, so a line never interleaves with another writer.
const PIPE_BUF: usize = 4096;
const CONSOLE_HISTORY_LINES: u32 = 30;
const DELIVERY_TIMEOUT: Duration = Duration::from_secs(5);
const DELIVERY_POLL: Duration = Duration::from_millis(50);
const LOG_LINES: u32 = 200;

pub(crate) fn console(ctx: &Ctx<'_>, name: &ServerName, words: &[String]) -> Result<(), Error> {
    let fifo = ctx.paths.console_fifo(name);
    let is_fifo = files::lstat(&fifo)?.is_some_and(|metadata| metadata.file_type().is_fifo());
    if !is_fifo || !systemd::is_active(&name.unit()) {
        return Err(Error::NotRunning { name: name.clone() });
    }
    let owner =
        users::lookup(&name.user())?.ok_or_else(|| Error::UnknownUser { user: name.user() })?;
    let line = (!words.is_empty())
        .then(|| command_line(words))
        .transpose()?;
    let mut writer = Writer::spawn(name, &fifo, owner)?;
    let sent = match line {
        Some(line) => writer.send(&line),
        None => interactive(name, &mut writer),
    };
    let finished = writer.finish();
    sent.and(finished)
}

pub(crate) fn logs(name: &ServerName, follow: bool) -> Result<(), Error> {
    let err = systemd::journal(&name.unit(), LOG_LINES, follow).exec();
    Err(Error::Spawn {
        program: "journalctl".to_owned(),
        source: err,
    })
}

/// Joins words into one console line with a trailing newline.
pub(crate) fn command_line(words: &[String]) -> Result<String, Error> {
    let line = format!("{}\n", words.join(" "));
    if words.iter().any(|word| word.contains(['\n', '\r'])) {
        return Err(Error::MultilineCommand);
    }
    if line.len() > PIPE_BUF {
        return Err(Error::CommandTooLong {
            len: line.len(),
            max: PIPE_BUF,
        });
    }
    Ok(line)
}

// The runtime folder belongs to the server user, so root must not open whatever that user put there.
/// Feeds console lines into the FIFO through `dd`, running as the server user.
struct Writer {
    name: ServerName,
    command: String,
    child: Child,
    stdin: Option<ChildStdin>,
}

impl Writer {
    fn spawn(name: &ServerName, fifo: &Path, owner: Owner) -> Result<Self, Error> {
        let mut target = OsString::from("of=");
        target.push(fifo);
        let mut command = Command::new("dd");
        command
            .arg(target)
            .args(["bs=4096", "status=none"])
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let description = process::describe(&command);
        let mut child = process::as_owner(&mut command, owner)
            .spawn()
            .map_err(|source| Error::Spawn {
                program: "dd".to_owned(),
                source,
            })?;
        let stdin = child.stdin.take();
        Ok(Self {
            name: name.clone(),
            command: description,
            child,
            stdin,
        })
    }

    fn send(&mut self, line: &str) -> Result<(), Error> {
        let broken = || Error::CommandFailed {
            command: self.command.clone(),
            detail: "the server stopped reading its console".to_owned(),
        };
        let stdin = self.stdin.as_mut().ok_or_else(broken)?;
        match stdin.write(line.as_bytes()) {
            Ok(written) if written == line.len() => Ok(()),
            Ok(_) | Err(_) => Err(broken()),
        }
    }

    /// Closes the input and waits for `dd`; a server that never opens its console counts as not running.
    fn finish(mut self) -> Result<(), Error> {
        drop(self.stdin.take());
        let Some(status) = self.wait_briefly()? else {
            self.child.kill().ok();
            self.child.wait().ok();
            return Err(Error::NotRunning { name: self.name });
        };
        let mut stderr = String::new();
        if let Some(mut pipe) = self.child.stderr.take() {
            pipe.read_to_string(&mut stderr).ok();
        }
        if status.success() {
            return Ok(());
        }
        Err(Error::CommandFailed {
            command: self.command,
            detail: stderr
                .lines()
                .rfind(|line| !line.trim().is_empty())
                .map_or_else(|| status.to_string(), str::to_owned),
        })
    }

    fn wait_briefly(&mut self) -> Result<Option<ExitStatus>, Error> {
        let started = Instant::now();
        loop {
            let exited = self.child.try_wait().map_err(|source| Error::Spawn {
                program: "dd".to_owned(),
                source,
            })?;
            if exited.is_some() || started.elapsed() >= DELIVERY_TIMEOUT {
                return Ok(exited);
            }
            std::thread::sleep(DELIVERY_POLL);
        }
    }
}

fn interactive(name: &ServerName, writer: &mut Writer) -> Result<(), Error> {
    let mut journal = systemd::journal(&name.unit(), CONSOLE_HISTORY_LINES, true)
        .stdin(Stdio::null())
        .spawn()
        .map_err(|source| Error::Spawn {
            program: "journalctl".to_owned(),
            source,
        })?;
    output::info("type server commands; Ctrl-D to leave");
    let result = forward_lines(writer);
    journal.kill().ok();
    journal.wait().ok();
    result
}

fn forward_lines(writer: &mut Writer) -> Result<(), Error> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(Error::Runtime)?;
    runtime.block_on(async {
        let mut interrupt = signal(SignalKind::interrupt()).map_err(Error::Runtime)?;
        let (lines_tx, mut lines_rx) = mpsc::channel::<io::Result<String>>(16);
        std::thread::spawn(move || {
            for line in io::stdin().lock().lines() {
                if lines_tx.blocking_send(line).is_err() {
                    break;
                }
            }
        });
        loop {
            tokio::select! {
                _ = interrupt.recv() => return Ok(()),
                line = lines_rx.recv() => match line {
                    None => return Ok(()),
                    Some(Err(err)) => return Err(Error::Terminal(err)),
                    Some(Ok(line)) if line.trim().is_empty() => {}
                    Some(Ok(line)) => writer.send(&command_line(&[line])?)?,
                },
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_one_console_line() {
        let words = ["say".to_owned(), "hello".to_owned(), "world".to_owned()];
        assert_eq!(command_line(&words).unwrap(), "say hello world\n");
        assert!(matches!(
            command_line(&["say\nop me".to_owned()]),
            Err(Error::MultilineCommand)
        ));
        assert!(matches!(
            command_line(&["x".repeat(PIPE_BUF)]),
            Err(Error::CommandTooLong { .. })
        ));
    }
}
