mod console;
mod delete;
mod doctor;
mod lifecycle;
mod new;
mod self_update;
mod status;
mod uninstall;
mod update;

use crate::cli::{Cli, Command};
use crate::error::Error;
use crate::names::ServerName;
use crate::output;
use crate::paths::Paths;
use crate::process;
use crate::{run, serve};

pub(crate) struct Ctx<'a> {
    pub(crate) paths: &'a Paths,
    pub(crate) yes: bool,
}

pub(crate) fn dispatch(cli: Cli, paths: &Paths) -> Result<(), Error> {
    let command = cli.command.unwrap_or(Command::Status);
    if command.needs_root() && !process::is_root() {
        return Err(Error::NotRoot {
            args: process::shell_words(
                std::env::args_os()
                    .skip(1)
                    .map(|arg| arg.to_string_lossy().into_owned()),
            ),
        });
    }
    let ctx = Ctx {
        paths,
        yes: cli.yes,
    };
    match command {
        Command::Status => status::status(&ctx),
        Command::New(args) => new::new(&ctx, args),
        Command::Delete { server } => delete::delete(&ctx, &server),
        Command::Start { servers } => lifecycle::start(&ctx, &servers),
        Command::Stop { servers } => lifecycle::stop(&ctx, &servers),
        Command::Restart { servers } => lifecycle::restart(&ctx, &servers),
        Command::Update {
            server: Some(server),
            version,
            force,
        } => update::update(&ctx, &server, version.as_deref(), force),
        Command::Update { server: None, .. } => self_update::self_update(&ctx),
        Command::Console { server, command } => console::console(&ctx, &server, &command),
        Command::Logs { server, follow } => console::logs(&server, follow),
        Command::Doctor => doctor::doctor(&ctx),
        Command::Uninstall { purge } => uninstall::uninstall(&ctx, purge),
        Command::Serve => serve::serve(paths),
        Command::Run { server } => run::run(paths, &server),
    }
}

/// Runs `action` for every server, reporting each failure and carrying on with the rest.
pub(crate) fn for_each_server(
    servers: &[ServerName],
    mut action: impl FnMut(&ServerName) -> Result<(), Error>,
) -> Result<(), Error> {
    if let [only] = servers {
        return action(only);
    }
    let mut failed = 0;
    for server in servers {
        if let Err(err) = action(server) {
            output::error(&err);
            failed += 1;
        }
    }
    if failed == 0 {
        Ok(())
    } else {
        Err(Error::SomeFailed {
            failed,
            total: servers.len(),
        })
    }
}
