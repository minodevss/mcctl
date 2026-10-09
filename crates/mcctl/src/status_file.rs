use std::collections::BTreeMap;
use std::fs;
use std::net::Ipv4Addr;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::Error;
use crate::files;
use crate::paths::Paths;

/// What `mcctl serve` publishes for the CLI in /run/mcctl/serve/status.json.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StatusFile {
    pub(crate) updated: u64,
    pub(crate) public_ip: Option<Ipv4Addr>,
    pub(crate) dns: DnsStatus,
    pub(crate) connections: BTreeMap<String, usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DnsStatus {
    pub(crate) provider: String,
    pub(crate) ok: bool,
    pub(crate) conflicts: Vec<String>,
    pub(crate) failures: Vec<String>,
    pub(crate) last_sync: Option<u64>,
}

impl Default for DnsStatus {
    fn default() -> Self {
        Self {
            provider: "none".to_owned(),
            ok: true,
            conflicts: Vec::new(),
            failures: Vec::new(),
            last_sync: None,
        }
    }
}

impl StatusFile {
    pub(crate) fn connections_to(&self, server: &str) -> usize {
        self.connections.get(server).copied().unwrap_or(0)
    }

    pub(crate) fn total_connections(&self) -> usize {
        self.connections.values().sum()
    }
}

impl DnsStatus {
    /// `off`, `ok`, `pending`, `N failures` or `N conflicts`.
    pub(crate) fn summary(&self) -> String {
        if self.provider == "none" {
            return "off".to_owned();
        }
        if !self.failures.is_empty() {
            return counted(self.failures.len(), "failure");
        }
        if !self.conflicts.is_empty() {
            return counted(self.conflicts.len(), "conflict");
        }
        if self.last_sync.is_none() {
            return "pending".to_owned();
        }
        "ok".to_owned()
    }
}

pub(crate) fn counted(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

/// `None` when the router has not written the file or it cannot be read.
pub(crate) fn read(paths: &Paths) -> Option<StatusFile> {
    let text = fs::read_to_string(paths.status_file()).ok()?;
    serde_json::from_str(&text).ok()
}

pub(crate) fn write(paths: &Paths, status: &StatusFile) -> Result<(), Error> {
    let path = paths.status_file();
    let mut json = serde_json::to_vec_pretty(status).map_err(|err| Error::Io {
        path: path.clone(),
        source: err.into(),
    })?;
    json.push(b'\n');
    files::write_atomic(&path, &json, 0o644)
}

pub(crate) fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> StatusFile {
        StatusFile {
            updated: 1_790_000_000,
            public_ip: Some(Ipv4Addr::new(203, 0, 113, 7)),
            dns: DnsStatus {
                provider: "cloudflare".to_owned(),
                ok: false,
                conflicts: vec!["lobby.example.com".to_owned()],
                failures: Vec::new(),
                last_sync: Some(1_789_999_990),
            },
            connections: BTreeMap::from([("survival".to_owned(), 3)]),
        }
    }

    #[test]
    fn status_json_round_trips() {
        let root = tempfile::tempdir().unwrap();
        let paths = Paths::under(root.path());
        fs::create_dir_all(paths.status_file().parent().unwrap()).unwrap();
        write(&paths, &sample()).unwrap();
        assert_eq!(read(&paths), Some(sample()));
        let json: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(paths.status_file()).unwrap()).unwrap();
        assert_eq!(json["public_ip"], "203.0.113.7");
        assert_eq!(json["dns"]["provider"], "cloudflare");
        assert_eq!(json["dns"]["last_sync"], 1_789_999_990);
        assert_eq!(json["connections"]["survival"], 3);
    }

    #[test]
    fn missing_or_broken_status_reads_as_none() {
        let root = tempfile::tempdir().unwrap();
        let paths = Paths::under(root.path());
        assert_eq!(read(&paths), None);
        fs::create_dir_all(paths.status_file().parent().unwrap()).unwrap();
        fs::write(paths.status_file(), "{").unwrap();
        assert_eq!(read(&paths), None);
    }

    #[test]
    fn summarizes_dns_state() {
        let mut dns = sample().dns;
        assert_eq!(dns.summary(), "1 conflict");
        dns.failures = vec!["a".to_owned(), "b".to_owned()];
        assert_eq!(dns.summary(), "2 failures");
        dns.failures.clear();
        dns.conflicts.clear();
        assert_eq!(dns.summary(), "ok");
        dns.last_sync = None;
        assert_eq!(dns.summary(), "pending");
        assert_eq!(DnsStatus::default().summary(), "off");
        assert_eq!(sample().connections_to("survival"), 3);
        assert_eq!(sample().connections_to("creative"), 0);
    }
}
