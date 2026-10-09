use std::collections::{BTreeMap, BTreeSet};
use std::net::Ipv4Addr;
use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::{Desired, Error, Listing, Owned, Provider};

/// A backend that can change records but cannot list them.
pub(crate) trait Writer: Send + Sync {
    fn write(&self, record: &Desired) -> Result<(), Error>;
    /// `remaining` holds every other name still applied after this one goes.
    fn erase(&self, name: &str, remaining: &BTreeSet<String>) -> Result<(), Error>;
}

/// Lists what this process applied, so the first run after start upserts every name.
pub(crate) struct Remembered<W> {
    writer: W,
    applied: Mutex<BTreeMap<String, Ipv4Addr>>,
}

impl<W> Remembered<W> {
    pub(crate) fn new(writer: W) -> Self {
        Self {
            writer,
            applied: Mutex::new(BTreeMap::new()),
        }
    }

    fn applied(&self) -> MutexGuard<'_, BTreeMap<String, Ipv4Addr>> {
        self.applied.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl<W: Writer> Provider for Remembered<W> {
    fn list(&self, _names: &BTreeSet<String>) -> Result<Listing, Error> {
        let owned = self
            .applied()
            .iter()
            .map(|(name, ip)| Owned {
                name: name.clone(),
                ip: *ip,
                id: None,
            })
            .collect();
        Ok(Listing {
            owned,
            foreign: BTreeSet::new(),
        })
    }

    fn upsert(&self, record: &Desired, _existing: Option<&Owned>) -> Result<(), Error> {
        self.writer.write(record)?;
        self.applied().insert(record.name.clone(), record.ip);
        Ok(())
    }

    fn delete(&self, existing: &Owned) -> Result<(), Error> {
        let remaining = self
            .applied()
            .keys()
            .filter(|name| **name != existing.name)
            .cloned()
            .collect();
        self.writer.erase(&existing.name, &remaining)?;
        self.applied().remove(&existing.name);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Log {
        calls: Mutex<Vec<String>>,
    }

    impl Writer for Log {
        fn write(&self, record: &Desired) -> Result<(), Error> {
            if record.name.starts_with("broken") {
                return Err(Error::MissingId {
                    name: record.name.clone(),
                });
            }
            self.calls
                .lock()
                .unwrap()
                .push(format!("write {} {}", record.name, record.ip));
            Ok(())
        }

        fn erase(&self, name: &str, remaining: &BTreeSet<String>) -> Result<(), Error> {
            let remaining: Vec<&str> = remaining.iter().map(String::as_str).collect();
            self.calls
                .lock()
                .unwrap()
                .push(format!("erase {name} keeping {}", remaining.join(",")));
            Ok(())
        }
    }

    fn names(provider: &Remembered<Log>) -> Vec<(String, Ipv4Addr)> {
        provider
            .list(&BTreeSet::new())
            .unwrap()
            .owned
            .into_iter()
            .map(|owned| (owned.name, owned.ip))
            .collect()
    }

    #[test]
    fn remembered_tracks_applied_records() {
        let provider = Remembered::new(Log::default());
        let ip = Ipv4Addr::new(1, 2, 3, 4);
        assert_eq!(names(&provider), []);

        provider
            .upsert(
                &Desired {
                    name: "a.example.com".into(),
                    ip,
                },
                None,
            )
            .unwrap();
        provider
            .upsert(
                &Desired {
                    name: "b.example.com".into(),
                    ip,
                },
                None,
            )
            .unwrap();
        assert!(
            provider
                .upsert(
                    &Desired {
                        name: "broken.example.com".into(),
                        ip
                    },
                    None
                )
                .is_err()
        );
        assert_eq!(
            names(&provider),
            [("a.example.com".into(), ip), ("b.example.com".into(), ip)]
        );

        let a = Owned {
            name: "a.example.com".into(),
            ip,
            id: None,
        };
        provider.delete(&a).unwrap();
        assert_eq!(names(&provider), [("b.example.com".into(), ip)]);
        assert_eq!(
            *provider.writer.calls.lock().unwrap(),
            [
                "write a.example.com 1.2.3.4",
                "write b.example.com 1.2.3.4",
                "erase a.example.com keeping b.example.com"
            ]
        );
    }
}
