use std::collections::BTreeSet;
use std::fs;

use crate::commands::Ctx;
use crate::commands::delete::remove_unit;
use crate::config;
use crate::error::Error;
use crate::files;
use crate::lock;
use crate::names::ServerName;
use crate::output;
use crate::paths::Paths;
use crate::systemd::{self, ROUTER_UNIT};
use crate::users;

const SERVICE_ACCOUNT: &str = "mcctl";
const UNIT_FILES: [&str; 2] = ["mcctl.service", "mc@.service"];

pub(crate) fn uninstall(ctx: &Ctx<'_>, purge: bool) -> Result<(), Error> {
    let _lock = lock::acquire(ctx.paths)?;
    let servers = known_servers(ctx.paths)?;
    let (warning, word) = if purge {
        (
            format!(
                "This removes mcctl and deletes every server, world and setting in {} and {}.",
                ctx.paths.config_dir().display(),
                ctx.paths.data_dir().display()
            ),
            "purge",
        )
    } else {
        (
            "This stops every server and removes mcctl; worlds and settings stay on disk."
                .to_owned(),
            "uninstall",
        )
    };
    if !output::confirm_typed(warning, word, ctx.yes)? {
        return Err(Error::Cancelled);
    }
    for name in &servers {
        remove_unit(ctx.paths, name)?;
    }
    if let Err(err) = systemd::disable_now(ROUTER_UNIT) {
        output::warn(err);
    }
    for unit in UNIT_FILES {
        files::remove_file_if_exists(&ctx.paths.unit_dir().join(unit))?;
    }
    files::remove_file_if_exists(&ctx.paths.binary())?;
    systemd::daemon_reload()?;
    if purge {
        purge_everything(ctx.paths, &servers)?;
        output::info("removed mcctl, all servers and all settings");
    } else {
        output::info(format!(
            "removed mcctl; servers are kept in {} and {}",
            ctx.paths.servers_dir().display(),
            ctx.paths.servers_config_dir().display()
        ));
    }
    Ok(())
}

/// Servers with a config or a data folder, so half-created ones are cleaned up too.
fn known_servers(paths: &Paths) -> Result<BTreeSet<ServerName>, Error> {
    let mut names: BTreeSet<ServerName> = config::server_files(paths)?
        .into_iter()
        .filter_map(|(name, _)| name.ok())
        .collect();
    if let Ok(entries) = fs::read_dir(paths.servers_dir()) {
        names.extend(
            entries
                .filter_map(Result::ok)
                .filter_map(|entry| entry.file_name().to_str()?.parse().ok()),
        );
    }
    Ok(names)
}

fn purge_everything(paths: &Paths, servers: &BTreeSet<ServerName>) -> Result<(), Error> {
    files::remove_tree(&paths.config_dir())?;
    files::remove_tree(&paths.data_dir())?;
    for name in servers {
        delete_user_if_present(&name.user())?;
    }
    delete_user_if_present(SERVICE_ACCOUNT)?;
    if users::group_exists(SERVICE_ACCOUNT)? {
        users::delete_group(SERVICE_ACCOUNT)?;
    }
    Ok(())
}

fn delete_user_if_present(user: &str) -> Result<(), Error> {
    if users::lookup(user)?.is_some() {
        users::delete_user(user)?;
    }
    Ok(())
}
