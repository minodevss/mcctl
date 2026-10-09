use crate::commands::Ctx;
use crate::config::{self, GlobalConfig};
use crate::error::Error;
use crate::files;
use crate::lock;
use crate::names::{Address, Hostname, ServerName};
use crate::output;
use crate::paths::Paths;
use crate::systemd;
use crate::users;

pub(crate) fn delete(ctx: &Ctx<'_>, name: &ServerName) -> Result<(), Error> {
    let _lock = lock::acquire(ctx.paths)?;
    let config = config::find_server(ctx.paths, name).or_else(|err| {
        if matches!(err, Error::ConfigSyntax { .. }) {
            output::warn(format!("{err}; deleting anyway"));
            Ok(None)
        } else {
            Err(err)
        }
    })?;
    let config_exists = files::exists_nofollow(&ctx.paths.server_config(name))?;
    let server_dir = ctx.paths.server_dir(name);
    let user = name.user();
    let user_exists = users::lookup(&user)?.is_some();
    if !config_exists && !files::exists_nofollow(&server_dir)? && !user_exists {
        return Err(Error::NoSuchServer { name: name.clone() });
    }
    let warning = format!(
        "This deletes {name}, its world and everything in {}.",
        server_dir.display()
    );
    if !output::confirm_typed(warning, name.as_str(), ctx.yes)? {
        return Err(Error::Cancelled);
    }
    remove_unit(ctx.paths, name)?;
    if let Some(Some(Address::Host(host))) = config.as_ref().map(|config| &config.address) {
        remove_dns_record(ctx.paths, host);
    }
    files::remove_file_if_exists(&ctx.paths.server_config(name))?;
    files::remove_tree(&server_dir)?;
    if user_exists {
        users::delete_user(&user)?;
    }
    output::info(format!("deleted {name}"));
    Ok(())
}

pub(crate) fn remove_unit(paths: &Paths, name: &ServerName) -> Result<(), Error> {
    if let Err(err) = systemd::disable_now(&name.unit()) {
        output::warn(err);
    }
    let dropins = paths.dropin_dir(name);
    if files::exists_nofollow(&dropins)? {
        files::remove_tree(&dropins)?;
        systemd::daemon_reload()?;
    }
    Ok(())
}

/// Best effort: a failure only warns, since the server is going away either way.
fn remove_dns_record(paths: &Paths, host: &Hostname) {
    let removed = config::load_global(paths).and_then(|global: GlobalConfig| {
        if global.dns.is_off() {
            return Ok(false);
        }
        let dns = global.dns.with_token(&config::read_token(paths)?)?;
        let Some(provider) = mcctl_dns::provider(&dns, mcctl_dns::agent()) else {
            return Ok(false);
        };
        mcctl_dns::remove(provider.as_ref(), host.as_str()).map_err(Error::dns(format!(
            "cannot remove the dns record for {host}"
        )))
    });
    match removed {
        Ok(true) => output::info(format!("removed the dns record for {host}")),
        Ok(false) => {}
        Err(err) => output::warn(err),
    }
}
