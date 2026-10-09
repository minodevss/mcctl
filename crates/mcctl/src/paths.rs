use std::path::{Path, PathBuf};

use crate::names::ServerName;

/// Every file location mcctl uses, under one root so tests can use a temporary folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Paths {
    root: PathBuf,
}

impl Paths {
    pub(crate) fn system() -> Self {
        Self::under("/")
    }

    pub(crate) fn under(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn at(&self, rel: impl AsRef<Path>) -> PathBuf {
        self.root.join(rel)
    }

    pub(crate) fn config_dir(&self) -> PathBuf {
        self.at("etc/mcctl")
    }

    pub(crate) fn global_config(&self) -> PathBuf {
        self.config_dir().join("mcctl.toml")
    }

    pub(crate) fn servers_config_dir(&self) -> PathBuf {
        self.config_dir().join("servers")
    }

    pub(crate) fn server_config(&self, name: &ServerName) -> PathBuf {
        self.servers_config_dir().join(format!("{name}.toml"))
    }

    pub(crate) fn lock_file(&self) -> PathBuf {
        self.config_dir().join(".lock")
    }

    pub(crate) fn dns_token(&self) -> PathBuf {
        self.config_dir().join("dns-token")
    }

    pub(crate) fn data_dir(&self) -> PathBuf {
        self.at("var/lib/mcctl")
    }

    pub(crate) fn servers_dir(&self) -> PathBuf {
        self.data_dir().join("servers")
    }

    pub(crate) fn server_dir(&self, name: &ServerName) -> PathBuf {
        self.servers_dir().join(name.as_str())
    }

    pub(crate) fn staging_dir(&self, name: &ServerName) -> PathBuf {
        self.servers_dir()
            .join(format!(".staging-{name}-{}", std::process::id()))
    }

    pub(crate) fn java_root(&self) -> PathBuf {
        self.data_dir().join("java")
    }

    pub(crate) fn dropin_dir(&self, name: &ServerName) -> PathBuf {
        self.at("etc/systemd/system")
            .join(format!("{}.d", name.unit()))
    }

    pub(crate) fn dropin_file(&self, name: &ServerName) -> PathBuf {
        self.dropin_dir(name).join("mcctl.conf")
    }

    pub(crate) fn status_file(&self) -> PathBuf {
        self.at("run/mcctl/serve/status.json")
    }

    pub(crate) fn console_fifo(&self, name: &ServerName) -> PathBuf {
        self.at("run/mcctl/servers")
            .join(name.as_str())
            .join("stdin")
    }

    pub(crate) fn binary(&self) -> PathBuf {
        self.at("usr/local/bin/mcctl")
    }

    pub(crate) fn unit_dir(&self) -> PathBuf {
        self.at("usr/local/lib/systemd/system")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_paths_match_the_unit_files() {
        let paths = Paths::system();
        let name: ServerName = "survival".parse().unwrap();
        let cases = [
            (
                paths.server_config(&name),
                "/etc/mcctl/servers/survival.toml",
            ),
            (paths.server_dir(&name), "/var/lib/mcctl/servers/survival"),
            (
                paths.dropin_file(&name),
                "/etc/systemd/system/mc@survival.service.d/mcctl.conf",
            ),
            (
                paths.console_fifo(&name),
                "/run/mcctl/servers/survival/stdin",
            ),
            (paths.status_file(), "/run/mcctl/serve/status.json"),
            (paths.java_root(), "/var/lib/mcctl/java"),
            (paths.binary(), "/usr/local/bin/mcctl"),
            (paths.unit_dir(), "/usr/local/lib/systemd/system"),
        ];
        for (actual, expected) in cases {
            assert_eq!(actual, PathBuf::from(expected));
        }
    }
}
