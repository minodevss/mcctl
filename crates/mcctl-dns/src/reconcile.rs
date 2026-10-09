use std::collections::{BTreeMap, BTreeSet};
use std::net::Ipv4Addr;

use crate::Error;
use crate::names::normalize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Desired {
    pub name: String,
    pub ip: Ipv4Addr,
}

/// An A record mcctl created; `id` is opaque to everything but the provider that listed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Owned {
    pub name: String,
    pub ip: Ipv4Addr,
    pub id: Option<String>,
}

/// `foreign` holds names of A records mcctl did not create.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Listing {
    pub owned: Vec<Owned>,
    pub foreign: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Create(Desired),
    Update(Desired, Owned),
    Delete(Owned),
    Conflict(String),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub applied: Vec<Action>,
    pub conflicts: Vec<String>,
    pub failures: Vec<(Action, Error)>,
}

pub trait Provider: Send + Sync {
    /// Lists A records in every zone that covers one of `names` (lowercase, no trailing dot).
    fn list(&self, names: &BTreeSet<String>) -> Result<Listing, Error>;
    fn upsert(&self, record: &Desired, existing: Option<&Owned>) -> Result<(), Error>;
    fn delete(&self, existing: &Owned) -> Result<(), Error>;
}

/// Upserts come before deletes; a name with a foreign record is a conflict and is left alone.
pub fn plan(desired: &BTreeMap<String, Ipv4Addr>, listing: &Listing) -> Vec<Action> {
    let desired: BTreeMap<String, Ipv4Addr> = desired
        .iter()
        .map(|(name, ip)| (normalize(name), *ip))
        .collect();
    let foreign: BTreeSet<String> = listing.foreign.iter().map(|name| normalize(name)).collect();
    let mut kept: BTreeMap<String, &Owned> = BTreeMap::new();
    let mut surplus = Vec::new();
    for owned in &listing.owned {
        let name = normalize(&owned.name);
        if desired.contains_key(&name) && !kept.contains_key(&name) {
            kept.insert(name, owned);
        } else {
            surplus.push(owned);
        }
    }
    let mut actions = Vec::new();
    for (name, ip) in desired {
        if foreign.contains(&name) {
            actions.push(Action::Conflict(name));
            continue;
        }
        let record = Desired { name, ip };
        match kept.get(&record.name) {
            Some(owned) if owned.ip == ip => {}
            Some(owned) => actions.push(Action::Update(record, (*owned).clone())),
            None => actions.push(Action::Create(record)),
        }
    }
    actions.extend(surplus.into_iter().cloned().map(Action::Delete));
    actions
}

/// Points every name at `ip` and deletes owned records whose name is no longer wanted.
/// Errors only when listing fails; each failed change lands in `Report::failures`.
pub fn sync(
    provider: &dyn Provider,
    names: &BTreeSet<String>,
    ip: Ipv4Addr,
) -> Result<Report, Error> {
    let names: BTreeSet<String> = names.iter().map(|name| normalize(name)).collect();
    let listing = provider.list(&names)?;
    let desired = names.into_iter().map(|name| (name, ip)).collect();
    Ok(apply(provider, plan(&desired, &listing)))
}

/// Deletes the owned record for one name, even when no other wanted name shares its zone.
/// Returns whether a record was deleted.
pub fn remove(provider: &dyn Provider, name: &str) -> Result<bool, Error> {
    let name = normalize(name);
    let listing = provider.list(&BTreeSet::from([name.clone()]))?;
    let mut removed = false;
    for owned in listing
        .owned
        .iter()
        .filter(|owned| normalize(&owned.name) == name)
    {
        provider.delete(owned)?;
        removed = true;
    }
    Ok(removed)
}

fn apply(provider: &dyn Provider, actions: Vec<Action>) -> Report {
    let mut report = Report::default();
    for action in actions {
        let outcome = match &action {
            Action::Create(record) => provider.upsert(record, None),
            Action::Update(record, owned) => provider.upsert(record, Some(owned)),
            Action::Delete(owned) => provider.delete(owned),
            Action::Conflict(name) => {
                report.conflicts.push(name.clone());
                continue;
            }
        };
        match outcome {
            Ok(()) => report.applied.push(action),
            Err(err) => report.failures.push((action, err)),
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    const HOME: Ipv4Addr = Ipv4Addr::new(1, 2, 3, 4);
    const OLD: Ipv4Addr = Ipv4Addr::new(5, 6, 7, 8);

    fn desired(names: &[&str]) -> BTreeMap<String, Ipv4Addr> {
        names
            .iter()
            .map(|name| ((*name).to_owned(), HOME))
            .collect()
    }

    fn owned(name: &str, ip: Ipv4Addr) -> Owned {
        Owned {
            name: name.into(),
            ip,
            id: Some(format!("id-{name}")),
        }
    }

    fn want(name: &str) -> Desired {
        Desired {
            name: name.into(),
            ip: HOME,
        }
    }

    fn listing(owned: Vec<Owned>, foreign: &[&str]) -> Listing {
        Listing {
            owned,
            foreign: foreign.iter().map(|name| (*name).to_owned()).collect(),
        }
    }

    #[test]
    fn plan_creates_missing() {
        let actions = plan(&desired(&["survival.example.com"]), &Listing::default());
        assert_eq!(actions, [Action::Create(want("survival.example.com"))]);
    }

    #[test]
    fn plan_updates_stale_ip() {
        let stale = owned("survival.example.com", OLD);
        let actions = plan(
            &desired(&["survival.example.com"]),
            &listing(vec![stale.clone()], &[]),
        );
        assert_eq!(
            actions,
            [Action::Update(want("survival.example.com"), stale)]
        );
    }

    #[test]
    fn plan_noop_in_sync() {
        let current = owned("survival.example.com", HOME);
        let actions = plan(
            &desired(&["survival.example.com"]),
            &listing(vec![current], &[]),
        );
        assert_eq!(actions, []);
    }

    #[test]
    fn plan_deletes_undesired_owned() {
        let gone = owned("creative.example.com", HOME);
        let kept = owned("survival.example.com", HOME);
        let actions = plan(
            &desired(&["survival.example.com"]),
            &listing(vec![gone.clone(), kept], &[]),
        );
        assert_eq!(actions, [Action::Delete(gone)]);
    }

    #[test]
    fn plan_deletes_duplicate_owned_records() {
        let first = owned("survival.example.com", HOME);
        let second = Owned {
            id: Some("other".into()),
            ..first.clone()
        };
        let actions = plan(
            &desired(&["survival.example.com"]),
            &listing(vec![first, second.clone()], &[]),
        );
        assert_eq!(actions, [Action::Delete(second)]);
    }

    #[test]
    fn plan_never_touches_foreign() {
        let actions = plan(
            &desired(&[]),
            &listing(vec![], &["www.example.com", "mail.example.com"]),
        );
        assert_eq!(actions, []);
    }

    #[test]
    fn plan_reports_conflict_on_foreign_name() {
        let actions = plan(
            &desired(&["survival.example.com"]),
            &listing(vec![], &["survival.example.com"]),
        );
        assert_eq!(actions, [Action::Conflict("survival.example.com".into())]);
    }

    #[test]
    fn plan_leaves_owned_record_alone_when_foreign_shares_its_name() {
        let ours = owned("survival.example.com", OLD);
        let actions = plan(
            &desired(&["survival.example.com"]),
            &listing(vec![ours], &["survival.example.com"]),
        );
        assert_eq!(actions, [Action::Conflict("survival.example.com".into())]);
    }

    #[test]
    fn names_compare_case_insensitively() {
        let current = owned("Survival.Example.com.", HOME);
        let in_sync = plan(
            &desired(&["survival.example.COM"]),
            &listing(vec![current], &[]),
        );
        assert_eq!(in_sync, []);
        let clash = plan(
            &desired(&["Lobby.Example.com"]),
            &listing(vec![], &["lobby.example.com."]),
        );
        assert_eq!(clash, [Action::Conflict("lobby.example.com".into())]);
    }

    struct Fake {
        listing: Listing,
        broken: &'static str,
        calls: Mutex<Vec<String>>,
    }

    impl Fake {
        fn record(&self, call: String, name: &str) -> Result<(), Error> {
            self.calls.lock().unwrap().push(call);
            if name == self.broken {
                return Err(Error::Rejected {
                    url: "https://dns.example.test".into(),
                    status: 500,
                    message: "boom".into(),
                });
            }
            Ok(())
        }
    }

    impl Provider for Fake {
        fn list(&self, _names: &BTreeSet<String>) -> Result<Listing, Error> {
            Ok(self.listing.clone())
        }

        fn upsert(&self, record: &Desired, _existing: Option<&Owned>) -> Result<(), Error> {
            self.record(format!("upsert {}", record.name), &record.name)
        }

        fn delete(&self, existing: &Owned) -> Result<(), Error> {
            self.record(format!("delete {}", existing.name), &existing.name)
        }
    }

    #[test]
    fn sync_continues_after_one_failure() {
        let fake = Fake {
            listing: listing(vec![owned("old.example.com", HOME)], &["taken.example.com"]),
            broken: "b.example.com",
            calls: Mutex::new(Vec::new()),
        };
        let names = [
            "a.example.com",
            "b.example.com",
            "c.example.com",
            "taken.example.com",
        ]
        .map(String::from)
        .into();
        let report = sync(&fake, &names, HOME).unwrap();
        assert_eq!(
            report.applied,
            [
                Action::Create(want("a.example.com")),
                Action::Create(want("c.example.com")),
                Action::Delete(owned("old.example.com", HOME)),
            ]
        );
        assert_eq!(report.conflicts, ["taken.example.com"]);
        assert_eq!(report.failures.len(), 1);
        assert_eq!(report.failures[0].0, Action::Create(want("b.example.com")));
        assert_eq!(fake.calls.lock().unwrap().len(), 4);
    }

    #[test]
    fn remove_deletes_only_the_named_owned_record() {
        let fake = Fake {
            listing: listing(
                vec![owned("a.example.com", HOME), owned("b.example.com", HOME)],
                &[],
            ),
            broken: "",
            calls: Mutex::new(Vec::new()),
        };
        assert_eq!(remove(&fake, "B.example.com."), Ok(true));
        assert_eq!(remove(&fake, "c.example.com"), Ok(false));
        assert_eq!(*fake.calls.lock().unwrap(), ["delete b.example.com"]);
    }
}
