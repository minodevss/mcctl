use std::fs;
use std::os::unix::process::CommandExt;
use std::process::{Command, Output, Stdio};

use crate::error::Error;
use crate::files::Owner;

/// Program and arguments, for error messages.
pub(crate) fn describe(command: &Command) -> String {
    std::iter::once(command.get_program())
        .chain(command.get_args())
        .map(|part| part.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

fn output(command: &mut Command) -> Result<Output, Error> {
    command
        .stdin(Stdio::null())
        .output()
        .map_err(|source| Error::Spawn {
            program: command.get_program().to_string_lossy().into_owned(),
            source,
        })
}

/// Runs to completion and returns stdout; a non-zero exit becomes an error carrying stderr.
pub(crate) fn run(command: &mut Command) -> Result<String, Error> {
    let output = output(command)?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(failed(command, &output))
    }
}

/// Runs to completion and returns the exit code with stdout; only a failure to start is an error.
pub(crate) fn probe(command: &mut Command) -> Result<(Option<i32>, String), Error> {
    let output = output(command)?;
    Ok((
        output.status.code(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
    ))
}

pub(crate) fn failed(command: &Command, output: &Output) -> Error {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let detail = last_line(&stderr)
        .or_else(|| last_line(&stdout))
        .map_or_else(|| output.status.to_string(), str::to_owned);
    Error::CommandFailed {
        command: describe(command),
        detail,
    }
}

fn last_line(text: &str) -> Option<&str> {
    text.lines().map(str::trim).rfind(|line| !line.is_empty())
}

pub(crate) fn as_owner(command: &mut Command, owner: Owner) -> &mut Command {
    command.uid(owner.uid).gid(owner.gid)
}

/// Whether the effective user is root, read from `/proc/self/status`; false where that is missing.
pub(crate) fn is_root() -> bool {
    fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| effective_uid(&status))
        == Some(0)
}

/// The effective uid: the second number on the `Uid:` line.
pub(crate) fn effective_uid(proc_status: &str) -> Option<u32> {
    proc_status
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

/// Joins arguments for a shell, quoting the ones a shell would split or expand.
pub(crate) fn shell_words<I, S>(args: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    args.into_iter()
        .map(|arg| quote(arg.as_ref()))
        .collect::<Vec<_>>()
        .join(" ")
}

fn quote(arg: &str) -> String {
    let plain = !arg.is_empty()
        && arg.chars().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | ':' | '=' | '@' | ',')
        });
    if plain {
        arg.to_owned()
    } else {
        format!("'{}'", arg.replace('\'', r"'\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_uid_from_proc_status() {
        let status = "Name:\tmcctl\nUmask:\t0022\nState:\tR (running)\nUid:\t1000\t0\t0\t0\nGid:\t1000\t1000\t1000\t1000\n";
        assert_eq!(effective_uid(status), Some(0));
        let user = "Name:\tmcctl\nUid:\t1000\t1000\t1000\t1000\n";
        assert_eq!(effective_uid(user), Some(1000));
        assert_eq!(effective_uid("Name:\tmcctl\n"), None);
        assert_eq!(effective_uid("Uid:\t1000\n"), None);
        assert_eq!(effective_uid("Uid:\tx\ty\n"), None);
    }

    #[test]
    fn quotes_shell_words_only_when_needed() {
        assert_eq!(
            shell_words(["new", "survival", "--address", "a.example.com"]),
            "new survival --address a.example.com"
        );
        assert_eq!(shell_words(["console", "say hi"]), "console 'say hi'");
        assert_eq!(shell_words(["it's"]), r"'it'\''s'");
        assert_eq!(shell_words([""]), "''");
    }
}
