use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use mcctl_router::RouteTable;
use tokio::sync::watch;
use tokio::time::MissedTickBehavior;

use crate::config::{self, DnsSetting};
use crate::error::Error;
use crate::names::MAIN_PORT;
use crate::output::{self, Level};
use crate::paths::Paths;
use crate::status_file;

const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// What the DNS loop keeps in sync: the provider setting and every routed hostname.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct DnsTarget {
    pub(super) setting: DnsSetting,
    pub(super) hostnames: BTreeSet<String>,
}

#[derive(Debug)]
pub(super) struct Loaded {
    pub(super) routes: RouteTable,
    pub(super) dns: DnsTarget,
    servers: usize,
}

impl Loaded {
    fn empty() -> Self {
        Self {
            routes: RouteTable::new(MAIN_PORT),
            dns: DnsTarget::default(),
            servers: 0,
        }
    }
}

/// Identity and size of each config file; any edit, rename or new file changes it.
type Fingerprint = BTreeMap<PathBuf, (u64, i64, i64, u64)>;

pub(super) struct Watcher {
    paths: Paths,
    seen: Fingerprint,
}

impl Watcher {
    pub(super) fn new(paths: Paths) -> Self {
        Self {
            paths,
            seen: Fingerprint::new(),
        }
    }

    /// Falls back to no routes and no DNS, so the router still starts with a broken config.
    pub(super) fn load_or_empty(&mut self) -> Loaded {
        self.seen = fingerprint(&self.paths);
        match load(&self.paths) {
            Ok(loaded) => {
                log_loaded(&loaded);
                loaded
            }
            Err(err) => {
                output::log(Level::Error, format_args!("cannot load the config: {err}"));
                Loaded::empty()
            }
        }
    }

    fn poll(&mut self) -> Option<Result<Loaded, Error>> {
        let now = fingerprint(&self.paths);
        if now == self.seen {
            return None;
        }
        self.seen = now;
        Some(load(&self.paths))
    }
}

/// Reloads on every config change; a broken file keeps the last good routes.
pub(super) async fn watch(
    mut watcher: Watcher,
    routes: watch::Sender<Arc<RouteTable>>,
    dns: watch::Sender<DnsTarget>,
) {
    let mut ticker = tokio::time::interval(POLL_INTERVAL);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        match watcher.poll() {
            None => {}
            Some(Ok(loaded)) => {
                log_loaded(&loaded);
                routes.send_if_modified(|current| {
                    let changed = **current != loaded.routes;
                    if changed {
                        *current = Arc::new(loaded.routes);
                    }
                    changed
                });
                dns.send_if_modified(|current| {
                    let changed = *current != loaded.dns;
                    if changed {
                        *current = loaded.dns;
                    }
                    changed
                });
            }
            Some(Err(err)) => output::log(
                Level::Error,
                format_args!("keeping the previous config: {err}"),
            ),
        }
    }
}

fn log_loaded(loaded: &Loaded) {
    output::info(format!(
        "loaded {}, dns {}",
        status_file::counted(loaded.servers, "server"),
        loaded.dns.setting.name()
    ));
}

fn load(paths: &Paths) -> Result<Loaded, Error> {
    let global = config::load_global(paths)?;
    let servers = config::load_servers(paths)?;
    Ok(Loaded {
        routes: config::route_table(&servers)?,
        dns: DnsTarget {
            setting: global.dns,
            hostnames: config::hostnames(&servers)
                .into_iter()
                .map(|host| host.to_string())
                .collect(),
        },
        servers: servers.len(),
    })
}

fn fingerprint(paths: &Paths) -> Fingerprint {
    let mut watched = vec![paths.global_config()];
    if let Ok(found) = config::server_files(paths) {
        watched.extend(found.into_iter().map(|(_, path)| path));
    }
    watched
        .into_iter()
        .filter_map(|path| {
            let metadata = fs::metadata(&path).ok()?;
            let identity = (
                metadata.ino(),
                metadata.mtime(),
                metadata.mtime_nsec(),
                metadata.len(),
            );
            Some((path, identity))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notices_new_changed_and_broken_configs() {
        let root = tempfile::tempdir().unwrap();
        let paths = Paths::under(root.path());
        let mut watcher = Watcher::new(paths.clone());
        let initial = watcher.load_or_empty();
        assert_eq!(initial.servers, 0);
        assert!(watcher.poll().is_none());
        let dir = paths.servers_config_dir();
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("survival.toml"),
            "address = \"survival.example.com\"\ninternal-port = 25600\n",
        )
        .unwrap();
        let loaded = watcher.poll().unwrap().unwrap();
        assert_eq!(loaded.servers, 1);
        assert_eq!(
            loaded.dns.hostnames,
            BTreeSet::from(["survival.example.com".to_owned()])
        );
        assert!(watcher.poll().is_none());
        fs::write(dir.join("broken.toml"), "internal-port = \"x\"\n").unwrap();
        assert!(watcher.poll().unwrap().is_err());
        assert!(watcher.poll().is_none());
    }
}
