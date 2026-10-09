use std::fs::{File, OpenOptions};
use std::os::unix::fs::FileTypeExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use mcctl_platform::Launch;

use crate::config;
use crate::error::Error;
use crate::files;
use crate::install;
use crate::names::ServerName;
use crate::output;
use crate::paths::Paths;
use crate::process;

const CONSOLE_FIFO: &str = "stdin";

/// Replaces this process with the server's Java; returns only when that fails.
pub(crate) fn run(paths: &Paths, name: &ServerName) -> Result<(), Error> {
    output::use_service_style();
    let config = config::load_server(paths, name)?;
    let dir = paths.server_dir(name);
    let detected =
        mcctl_platform::detect(&dir).map_err(Error::platform(format!("cannot start {name}")))?;
    let java = mcctl_platform::java_path(&paths.java_root(), detected.java);
    if !java.exists() {
        return Err(Error::JavaMissing {
            major: detected.java,
            name: name.clone(),
        });
    }
    let fifo_path = console_fifo(paths, name);
    let console = open_console(&fifo_path)?;
    let argv = mcctl_platform::command_line(
        &detected,
        &Launch {
            java,
            memory_mib: config.memory.mib(),
            port: config.internal_port.get(),
        },
    );
    let Some((program, arguments)) = argv.split_first() else {
        return Ok(());
    };
    output::info(format!(
        "starting {} with java {} on 127.0.0.1:{}",
        install::detected_version(&detected),
        detected.java,
        config.internal_port
    ));
    let err = Command::new(program)
        .args(arguments)
        .current_dir(&dir)
        .stdin(console)
        .exec();
    Err(Error::Spawn {
        program: program.to_string_lossy().into_owned(),
        source: err,
    })
}

fn console_fifo(paths: &Paths, name: &ServerName) -> PathBuf {
    std::env::var_os("RUNTIME_DIRECTORY").map_or_else(
        || paths.console_fifo(name),
        |dir| PathBuf::from(dir).join(CONSOLE_FIFO),
    )
}

// Holding the write end too means the server never reads EOF when a console writer leaves.
fn open_console(path: &Path) -> Result<File, Error> {
    match files::lstat(path)? {
        Some(metadata) if metadata.file_type().is_fifo() => {}
        Some(_) => {
            files::remove_file_if_exists(path)?;
            make_fifo(path)?;
        }
        None => make_fifo(path)?,
    }
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(Error::io(path))
}

fn make_fifo(path: &Path) -> Result<(), Error> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(Error::io(parent))?;
    }
    process::run(Command::new("mkfifo").args(["-m", "0600"]).arg(path)).map(drop)
}
