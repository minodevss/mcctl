use std::path::Path;
use std::process::Command;

use crate::error::Error;
use crate::files::Owner;
use crate::process;

const GETENT_NOT_FOUND: i32 = 2;

/// Reads `name:x:uid:gid:...` from `getent passwd`.
pub(crate) fn parse_passwd_line(line: &str) -> Option<Owner> {
    let mut fields = line.trim().split(':');
    let _name = fields.next()?;
    let _password = fields.next()?;
    let uid = fields.next()?.parse().ok()?;
    let gid = fields.next()?.parse().ok()?;
    Some(Owner { uid, gid })
}

pub(crate) fn lookup(user: &str) -> Result<Option<Owner>, Error> {
    let mut command = Command::new("getent");
    command.args(["passwd", user]);
    match process::probe(&mut command)? {
        (Some(0), stdout) => Ok(stdout.lines().next().and_then(parse_passwd_line)),
        (Some(GETENT_NOT_FOUND), _) => Ok(None),
        (code, _) => Err(Error::CommandFailed {
            command: process::describe(&command),
            detail: code.map_or_else(|| "killed".to_owned(), |code| format!("exit code {code}")),
        }),
    }
}

pub(crate) fn group_exists(group: &str) -> Result<bool, Error> {
    let mut command = Command::new("getent");
    command.args(["group", group]);
    Ok(process::probe(&mut command)?.0 == Some(0))
}

pub(crate) fn create_server_user(user: &str, home: &Path) -> Result<Owner, Error> {
    process::run(
        Command::new("useradd")
            .args(["--system", "--user-group", "--home-dir"])
            .arg(home)
            .args(["--no-create-home", "--shell", "/usr/sbin/nologin", user]),
    )?;
    lookup(user)?.ok_or_else(|| Error::UnknownUser {
        user: user.to_owned(),
    })
}

pub(crate) fn delete_user(user: &str) -> Result<(), Error> {
    process::run(Command::new("userdel").arg(user)).map(drop)
}

pub(crate) fn delete_group(group: &str) -> Result<(), Error> {
    process::run(Command::new("groupdel").arg(group)).map(drop)
}

pub(crate) fn add_to_group(user: &str, group: &str) -> Result<(), Error> {
    process::run(Command::new("usermod").args(["-a", "-G", group, user])).map(drop)
}

/// The account that ran sudo, unless it was root itself.
pub(crate) fn sudo_user() -> Option<String> {
    std::env::var("SUDO_USER")
        .ok()
        .filter(|user| !user.is_empty() && user != "root")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_ids_from_passwd_line() {
        assert_eq!(
            parse_passwd_line(
                "mc-survival:x:998:997::/var/lib/mcctl/servers/survival:/usr/sbin/nologin\n"
            ),
            Some(Owner { uid: 998, gid: 997 })
        );
        assert_eq!(
            parse_passwd_line("mc-survival:x:abc:997::/:/bin/false"),
            None
        );
        assert_eq!(parse_passwd_line("mc-survival"), None);
    }
}
