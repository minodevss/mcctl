use std::collections::BTreeMap;

use crate::commands::{Ctx, for_each_server};
use crate::config::{self, ServerConfig};
use crate::error::Error;
use crate::http;
use crate::install;
use crate::lock;
use crate::names::ServerName;
use crate::output;
use crate::paths::Paths;
use crate::status_file::{self, StatusFile};
use crate::systemd;

pub(crate) fn start(ctx: &Ctx<'_>, names: &[ServerName]) -> Result<(), Error> {
    let _lock = lock::acquire(ctx.paths)?;
    let configs = load_all(ctx.paths, names)?;
    write_dropins(ctx.paths, &configs)?;
    let agent = http::agent();
    for_each_server(names, |name| {
        start_one(ctx.paths, &agent, name)?;
        output::info(format!("started {name}"));
        Ok(())
    })
}

pub(crate) fn stop(ctx: &Ctx<'_>, names: &[ServerName]) -> Result<(), Error> {
    let _lock = lock::acquire(ctx.paths)?;
    load_all(ctx.paths, names)?;
    let published = status_file::read(ctx.paths);
    for_each_server(names, |name| {
        if !confirm_dropping_players(published.as_ref(), name, "Stop", ctx.yes)? {
            output::info(format!("kept {name} running"));
            return Ok(());
        }
        systemd::disable_now(&name.unit())?;
        output::info(format!("stopped {name}"));
        Ok(())
    })
}

pub(crate) fn restart(ctx: &Ctx<'_>, names: &[ServerName]) -> Result<(), Error> {
    let _lock = lock::acquire(ctx.paths)?;
    let configs = load_all(ctx.paths, names)?;
    write_dropins(ctx.paths, &configs)?;
    let published = status_file::read(ctx.paths);
    let agent = http::agent();
    for_each_server(names, |name| {
        if !confirm_dropping_players(published.as_ref(), name, "Restart", ctx.yes)? {
            output::info(format!("kept {name} running"));
            return Ok(());
        }
        prepare_java(ctx.paths, &agent, name)?;
        let unit = name.unit();
        systemd::reset_failed(&unit).ok();
        systemd::enable(&unit)?;
        systemd::restart(&unit)?;
        output::info(format!("restarted {name}"));
        Ok(())
    })
}

/// Makes sure the server's Java is present, then enables and starts the unit.
pub(crate) fn start_one(
    paths: &Paths,
    agent: &ureq::Agent,
    name: &ServerName,
) -> Result<(), Error> {
    prepare_java(paths, agent, name)?;
    let unit = name.unit();
    systemd::reset_failed(&unit).ok();
    systemd::enable_now(&unit)
}

fn prepare_java(paths: &Paths, agent: &ureq::Agent, name: &ServerName) -> Result<(), Error> {
    let detected = mcctl_platform::detect(&paths.server_dir(name))
        .map_err(Error::platform(format!("cannot start {name}")))?;
    install::refresh_java(agent, paths, detected.java)
}

/// Asks before cutting off players; `action` completes "<n> connections on <name>. <action> now?".
pub(crate) fn confirm_dropping_players(
    published: Option<&StatusFile>,
    name: &ServerName,
    action: &str,
    assume_yes: bool,
) -> Result<bool, Error> {
    match published.map_or(0, |published| published.connections_to(name.as_str())) {
        0 => Ok(true),
        count => output::confirm(
            format!(
                "{} on {name}. {action} now?",
                status_file::counted(count, "connection")
            ),
            assume_yes,
        ),
    }
}

fn load_all(
    paths: &Paths,
    names: &[ServerName],
) -> Result<BTreeMap<ServerName, ServerConfig>, Error> {
    names
        .iter()
        .map(|name| Ok((name.clone(), config::load_server(paths, name)?)))
        .collect()
}

/// Writes every memory drop-in and reloads systemd once if any changed.
pub(crate) fn write_dropins(
    paths: &Paths,
    configs: &BTreeMap<ServerName, ServerConfig>,
) -> Result<(), Error> {
    let mut changed = false;
    for (name, config) in configs {
        changed |= systemd::write_dropin(paths, name, config.memory)?;
    }
    if changed {
        systemd::daemon_reload()?;
    }
    Ok(())
}
