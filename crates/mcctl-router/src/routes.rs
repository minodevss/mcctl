use std::collections::BTreeSet;
use std::collections::btree_map::{self, BTreeMap};
use std::collections::hash_map::{self, HashMap};
use std::net::SocketAddr;

use mcctl_protocol::normalize_host;

use crate::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backend {
    pub server: String,
    pub addr: SocketAddr,
}

/// The main port routes by handshake host; each dedicated port sends everything to one backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteTable {
    main_port: u16,
    hosts: HashMap<String, Backend>,
    ports: BTreeMap<u16, Backend>,
}

impl RouteTable {
    pub fn new(main_port: u16) -> Self {
        Self {
            main_port,
            hosts: HashMap::new(),
            ports: BTreeMap::new(),
        }
    }

    pub fn main_port(&self) -> u16 {
        self.main_port
    }

    /// Normalizes `host` the same way as handshake hosts, so two spellings of one name collide.
    pub fn add_host(&mut self, host: &str, backend: Backend) -> Result<(), Error> {
        match self.hosts.entry(normalize_host(host)) {
            hash_map::Entry::Occupied(taken) => Err(Error::DuplicateHost {
                host: taken.key().clone(),
                server: taken.get().server.clone(),
            }),
            hash_map::Entry::Vacant(slot) => {
                slot.insert(backend);
                Ok(())
            }
        }
    }

    pub fn add_port(&mut self, port: u16, backend: Backend) -> Result<(), Error> {
        if port == self.main_port {
            return Err(Error::MainPort {
                port,
                server: backend.server,
            });
        }
        match self.ports.entry(port) {
            btree_map::Entry::Occupied(taken) => Err(Error::DuplicatePort {
                port,
                server: taken.get().server.clone(),
            }),
            btree_map::Entry::Vacant(slot) => {
                slot.insert(backend);
                Ok(())
            }
        }
    }

    /// `host` is the raw handshake host; it is normalized here.
    pub fn route(&self, listen_port: u16, host: &str) -> Option<&Backend> {
        if listen_port == self.main_port {
            self.hosts.get(&normalize_host(host))
        } else {
            self.ports.get(&listen_port)
        }
    }

    pub fn listen_ports(&self) -> BTreeSet<u16> {
        std::iter::once(self.main_port)
            .chain(self.ports.keys().copied())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn backend(server: &str, port: u16) -> Backend {
        Backend {
            server: server.to_owned(),
            addr: SocketAddr::from(([127, 0, 0, 1], port)),
        }
    }

    fn table() -> RouteTable {
        let mut table = RouteTable::new(25565);
        table
            .add_host("Survival.Example.com", backend("survival", 30001))
            .unwrap();
        table
            .add_host("creative.example.com", backend("creative", 30002))
            .unwrap();
        table.add_port(25570, backend("modded", 30003)).unwrap();
        table
    }

    #[test]
    fn routes_main_port_by_normalized_host() {
        let table = table();
        let cases = [
            ("survival.example.com", Some("survival")),
            ("SURVIVAL.example.com.", Some("survival")),
            ("survival.example.com\0FML3\0", Some("survival")),
            ("creative.example.com", Some("creative")),
            ("unknown.example.com", None),
            ("", None),
        ];
        for (host, expected) in cases {
            let routed = table
                .route(25565, host)
                .map(|backend| backend.server.as_str());
            assert_eq!(routed, expected, "{host:?}");
        }
    }

    #[test]
    fn dedicated_port_ignores_host() {
        let table = table();
        for host in ["survival.example.com", "anything.invalid", ""] {
            let routed = table
                .route(25570, host)
                .map(|backend| backend.server.as_str());
            assert_eq!(routed, Some("modded"), "{host:?}");
        }
    }

    #[test]
    fn unlisted_port_has_no_route() {
        assert_eq!(table().route(25571, "survival.example.com"), None);
    }

    #[test]
    fn listens_on_main_and_dedicated_ports() {
        assert_eq!(table().listen_ports(), BTreeSet::from([25565, 25570]));
    }

    #[test]
    fn rejects_duplicate_host_after_normalizing() {
        let mut table = table();
        assert_eq!(
            table.add_host("survival.example.com.", backend("other", 30009)),
            Err(Error::DuplicateHost {
                host: "survival.example.com".to_owned(),
                server: "survival".to_owned(),
            })
        );
        assert_eq!(
            table
                .route(25565, "survival.example.com")
                .map(|b| b.server.as_str()),
            Some("survival")
        );
    }

    #[test]
    fn rejects_duplicate_port() {
        let mut table = table();
        assert_eq!(
            table.add_port(25570, backend("other", 30009)),
            Err(Error::DuplicatePort {
                port: 25570,
                server: "modded".to_owned(),
            })
        );
    }

    #[test]
    fn rejects_main_port_as_dedicated_port() {
        let mut table = table();
        assert_eq!(
            table.add_port(25565, backend("other", 30009)),
            Err(Error::MainPort {
                port: 25565,
                server: "other".to_owned(),
            })
        );
    }
}
