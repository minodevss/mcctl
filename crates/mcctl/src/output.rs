#![expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "output.rs is the one place that writes to the terminal and the journal"
)]

use std::fmt::Display;
use std::io::{self, BufRead, IsTerminal, Write};
use std::process::ExitCode;
use std::sync::OnceLock;

use crate::error::Error;

static STYLE: OnceLock<Style> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Style {
    Cli,
    Plain,
    Journal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Level {
    Info,
    Warn,
    Error,
}

/// Switches stderr lines to service style: no `mcctl:` prefix, syslog levels when journald reads them.
pub(crate) fn use_service_style() {
    let style = if std::env::var_os("JOURNAL_STREAM").is_some() {
        Style::Journal
    } else {
        Style::Plain
    };
    STYLE.set(style).ok();
}

fn style() -> Style {
    STYLE.get().copied().unwrap_or(Style::Cli)
}

fn format_line(style: Style, level: Level, message: &str) -> String {
    match (style, level) {
        (Style::Cli, Level::Info | Level::Error) => format!("mcctl: {message}"),
        (Style::Cli, Level::Warn) => format!("mcctl: warning: {message}"),
        (Style::Plain, Level::Info) => message.to_owned(),
        (Style::Plain, Level::Warn) => format!("warning: {message}"),
        (Style::Plain, Level::Error) => format!("error: {message}"),
        (Style::Journal, Level::Info) => format!("<6>{message}"),
        (Style::Journal, Level::Warn) => format!("<4>{message}"),
        (Style::Journal, Level::Error) => format!("<3>{message}"),
    }
}

pub(crate) fn data(line: impl Display) {
    println!("{line}");
}

pub(crate) fn log(level: Level, message: impl Display) {
    eprintln!("{}", format_line(style(), level, &message.to_string()));
}

pub(crate) fn info(message: impl Display) {
    log(Level::Info, message);
}

pub(crate) fn warn(message: impl Display) {
    log(Level::Warn, message);
}

pub(crate) fn error(err: &Error) {
    match style() {
        Style::Cli => {
            log(Level::Error, format_args!("error: {err}"));
            if let Some(hint) = err.hint() {
                log(Level::Error, format_args!("hint: {hint}"));
            }
        }
        Style::Plain | Style::Journal => log(Level::Error, err),
    }
}

/// Prints clap's help, version or usage error and returns its exit code (0 or 2).
pub(crate) fn clap_exit(err: &clap::Error) -> ExitCode {
    err.print().ok();
    match err.exit_code() {
        0 => ExitCode::SUCCESS,
        _ => ExitCode::from(2),
    }
}

/// Asks a yes/no question on the terminal; `assume_yes` answers it without asking.
pub(crate) fn confirm(question: impl Display, assume_yes: bool) -> Result<bool, Error> {
    if assume_yes {
        return Ok(true);
    }
    let answer = ask(&format!("{question} [y/N] "))?;
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

/// Asks the user to type `expected` exactly; `assume_yes` skips the question.
pub(crate) fn confirm_typed(
    warning: impl Display,
    expected: &str,
    assume_yes: bool,
) -> Result<bool, Error> {
    if assume_yes {
        return Ok(true);
    }
    if !io::stdin().is_terminal() {
        return Err(Error::ConfirmationNeeded);
    }
    eprintln!("{warning}");
    let answer = ask(&format!("Type {expected} to confirm: "))?;
    Ok(answer.trim() == expected)
}

fn ask(prompt: &str) -> Result<String, Error> {
    let stdin = io::stdin();
    if !stdin.is_terminal() {
        return Err(Error::ConfirmationNeeded);
    }
    eprint!("{prompt}");
    io::stderr().flush().map_err(Error::Terminal)?;
    let mut answer = String::new();
    stdin
        .lock()
        .read_line(&mut answer)
        .map_err(Error::Terminal)?;
    Ok(answer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixes_lines_by_style_and_level() {
        let cases = [
            (Style::Cli, Level::Info, "mcctl: started survival"),
            (Style::Cli, Level::Warn, "mcctl: warning: started survival"),
            (Style::Plain, Level::Warn, "warning: started survival"),
            (Style::Plain, Level::Error, "error: started survival"),
            (Style::Journal, Level::Info, "<6>started survival"),
            (Style::Journal, Level::Warn, "<4>started survival"),
            (Style::Journal, Level::Error, "<3>started survival"),
        ];
        for (style, level, expected) in cases {
            assert_eq!(format_line(style, level, "started survival"), expected);
        }
    }
}
