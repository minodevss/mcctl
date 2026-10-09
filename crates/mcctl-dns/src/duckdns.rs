use std::collections::BTreeSet;

use crate::remembered::Writer;
use crate::{Desired, Error, http};

const UPDATE_URL: &str = "https://www.duckdns.org/update";
const ZONE_SUFFIX: &str = ".duckdns.org";

pub(crate) struct DuckDns {
    agent: ureq::Agent,
    token: String,
}

impl DuckDns {
    pub(crate) fn new(agent: ureq::Agent, token: &str) -> Self {
        Self {
            agent,
            token: token.to_owned(),
        }
    }

    fn update(&self, label: &str, (key, value): (&str, &str)) -> Result<(), Error> {
        let request = self
            .agent
            .get(UPDATE_URL)
            .query("domains", label)
            .query("token", &self.token)
            .query(key, value);
        let reply = http::receive(UPDATE_URL, request.call())?;
        if reply.is_success() && reply.body.trim() == "OK" {
            return Ok(());
        }
        Err(reply.rejected(format!(
            "duckdns answered {} for {label}",
            http::excerpt(&reply.body)
        )))
    }
}

impl Writer for DuckDns {
    fn write(&self, record: &Desired) -> Result<(), Error> {
        self.update(label(&record.name)?, ("ip", &record.ip.to_string()))
    }

    fn erase(&self, name: &str, remaining: &BTreeSet<String>) -> Result<(), Error> {
        let label = label(name)?;
        if label_in_use(label, remaining) {
            return Ok(());
        }
        self.update(label, ("clear", "true"))
    }
}

/// Every name under `<label>.duckdns.org` resolves to the one address stored for that label.
pub(crate) fn label(name: &str) -> Result<&str, Error> {
    name.strip_suffix(ZONE_SUFFIX)
        .and_then(|rest| rest.rsplit('.').next())
        .filter(|label| !label.is_empty())
        .ok_or_else(|| Error::NotDuckDns {
            name: name.to_owned(),
        })
}

fn label_in_use(label: &str, names: &BTreeSet<String>) -> bool {
    names
        .iter()
        .any(|name| self::label(name).is_ok_and(|other| other == label))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duckdns_label_from_name() {
        let cases = [
            ("alice.duckdns.org", "alice"),
            ("survival.alice.duckdns.org", "alice"),
            ("a.b.alice.duckdns.org", "alice"),
        ];
        for (name, expected) in cases {
            assert_eq!(label(name), Ok(expected), "{name}");
        }
    }

    #[test]
    fn duckdns_rejects_non_duckdns_name() {
        for name in [
            "example.com",
            "duckdns.org",
            ".duckdns.org",
            "alice.duckdns.org.example.com",
            "aliceduckdns.org",
        ] {
            assert_eq!(
                label(name),
                Err(Error::NotDuckDns { name: name.into() }),
                "{name}"
            );
        }
    }

    #[test]
    fn duckdns_clears_label_only_when_no_name_remains() {
        let remaining: BTreeSet<String> = [
            "creative.alice.duckdns.org".into(),
            "bob.duckdns.org".into(),
        ]
        .into();
        assert!(label_in_use("alice", &remaining));
        assert!(label_in_use("bob", &remaining));
        assert!(!label_in_use("carol", &remaining));
    }
}
