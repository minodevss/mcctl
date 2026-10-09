use std::collections::{BTreeMap, BTreeSet};
use std::net::Ipv4Addr;
use std::sync::{Mutex, MutexGuard, PoisonError};

use serde::de::{DeserializeOwned, IgnoredAny};
use serde::{Deserialize, Serialize};

use crate::http::{self, Reply};
use crate::names::{OWNER_MARK, normalize, relative, suffixes};
use crate::{Desired, Error, Listing, Owned, Provider};

const API: &str = "https://api.porkbun.com/api/json/v3";
const DOMAINS_PER_PAGE: usize = 1000;

pub(crate) struct Porkbun {
    agent: ureq::Agent,
    api_key: String,
    secret_key: String,
    roots: Mutex<BTreeMap<String, Option<String>>>,
}

impl Porkbun {
    pub(crate) fn new(agent: ureq::Agent, api_key: &str, secret_key: &str) -> Self {
        Self {
            agent,
            api_key: api_key.to_owned(),
            secret_key: secret_key.to_owned(),
            roots: Mutex::new(BTreeMap::new()),
        }
    }

    fn roots(&self) -> MutexGuard<'_, BTreeMap<String, Option<String>>> {
        self.roots.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn post<T: DeserializeOwned>(&self, path: &str, fields: impl Serialize) -> Result<T, Error> {
        let url = format!("{API}/{path}");
        let body = Signed {
            apikey: &self.api_key,
            secretapikey: &self.secret_key,
            fields,
        };
        checked(&http::receive(
            &url,
            self.agent.post(&url).send_json(&body),
        )?)
    }

    fn domains(&self) -> Result<BTreeSet<String>, Error> {
        let mut domains = BTreeSet::new();
        for start in (0..).step_by(DOMAINS_PER_PAGE) {
            let page: DomainList = self.post("domain/listAll", Start { start })?;
            let full = page.domains.len() >= DOMAINS_PER_PAGE;
            let known = domains.len();
            domains.extend(
                page.domains
                    .into_iter()
                    .map(|entry| normalize(&entry.domain)),
            );
            if !full || domains.len() == known {
                break;
            }
        }
        Ok(domains)
    }

    fn root_of(&self, name: &str) -> Result<String, Error> {
        let cached = self.roots().get(name).cloned();
        let root = match cached {
            Some(root) => root,
            None => root_domain(name, &self.domains()?).map(str::to_owned),
        };
        root.ok_or_else(|| Error::NoZone {
            name: name.to_owned(),
        })
    }
}

impl Provider for Porkbun {
    fn list(&self, names: &BTreeSet<String>) -> Result<Listing, Error> {
        let domains = self.domains()?;
        let roots: BTreeMap<String, Option<String>> = names
            .iter()
            .map(|name| (name.clone(), root_domain(name, &domains).map(str::to_owned)))
            .collect();
        let distinct: BTreeSet<String> = roots.values().flatten().cloned().collect();
        *self.roots() = roots;
        let mut listing = Listing::default();
        for domain in distinct {
            let records: RecordList = self.post(&format!("dns/retrieve/{domain}"), Empty {})?;
            let (owned, foreign) = split(&domain, records.records)?;
            listing.owned.extend(owned);
            listing.foreign.extend(foreign);
        }
        Ok(listing)
    }

    fn upsert(&self, record: &Desired, existing: Option<&Owned>) -> Result<(), Error> {
        if let Some(owned) = existing {
            let (domain, id) = ids(owned)?;
            let path = format!("dns/edit/{domain}/{id}");
            self.post::<IgnoredAny>(&path, record_fields(record, domain))
                .map(drop)
        } else {
            let domain = self.root_of(&record.name)?;
            let path = format!("dns/create/{domain}");
            self.post::<IgnoredAny>(&path, record_fields(record, &domain))
                .map(drop)
        }
    }

    fn delete(&self, existing: &Owned) -> Result<(), Error> {
        let (domain, id) = ids(existing)?;
        self.post::<IgnoredAny>(&format!("dns/delete/{domain}/{id}"), Empty {})
            .map(drop)
    }
}

#[derive(Serialize)]
struct Signed<'a, T> {
    apikey: &'a str,
    secretapikey: &'a str,
    #[serde(flatten)]
    fields: T,
}

#[derive(Serialize)]
struct Empty {}

#[derive(Serialize)]
struct Start {
    start: usize,
}

/// TTL is left out so Porkbun applies the account minimum.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct RecordFields<'a> {
    name: &'a str,
    #[serde(rename = "type")]
    kind: &'a str,
    content: Ipv4Addr,
    notes: &'a str,
}

#[derive(Deserialize)]
struct Outcome {
    status: String,
    message: Option<String>,
    code: Option<String>,
}

#[derive(Deserialize)]
struct DomainList {
    #[serde(default)]
    domains: Vec<Domain>,
}

#[derive(Deserialize)]
struct Domain {
    domain: String,
}

#[derive(Deserialize)]
struct RecordList {
    #[serde(default)]
    records: Vec<Record>,
}

#[derive(Deserialize)]
struct Record {
    id: String,
    name: String,
    #[serde(rename = "type")]
    kind: String,
    content: String,
    notes: Option<String>,
}

fn record_fields<'a>(record: &'a Desired, domain: &str) -> RecordFields<'a> {
    RecordFields {
        name: relative(&record.name, domain),
        kind: "A",
        content: record.ip,
        notes: OWNER_MARK,
    }
}

fn checked<T: DeserializeOwned>(reply: &Reply) -> Result<T, Error> {
    let outcome: Outcome = reply.json()?;
    if outcome.status != "SUCCESS" || !reply.is_success() {
        let message = outcome
            .message
            .unwrap_or_else(|| outcome.status.to_lowercase());
        let message = match outcome.code {
            Some(code) => format!("{message} ({code})"),
            None => message,
        };
        return Err(reply.rejected(message));
    }
    reply.json()
}

/// The longest account domain that `name` sits in, so `a.example.co.uk` maps to `example.co.uk`.
fn root_domain<'a>(name: &'a str, domains: &BTreeSet<String>) -> Option<&'a str> {
    suffixes(name).find(|suffix| domains.contains(*suffix))
}

fn split(domain: &str, records: Vec<Record>) -> Result<(Vec<Owned>, BTreeSet<String>), Error> {
    let mut owned = Vec::new();
    let mut foreign = BTreeSet::new();
    for record in records.into_iter().filter(|record| record.kind == "A") {
        if record.notes.as_deref() != Some(OWNER_MARK) {
            foreign.insert(normalize(&record.name));
            continue;
        }
        let ip = record.content.parse().map_err(|_| Error::Response {
            url: format!("{API}/dns/retrieve/{domain}"),
            reason: format!(
                "record {} holds {:?}, not an ipv4 address",
                record.name, record.content
            ),
        })?;
        owned.push(Owned {
            name: normalize(&record.name),
            ip,
            id: Some(format!("{domain}/{}", record.id)),
        });
    }
    Ok((owned, foreign))
}

fn ids(owned: &Owned) -> Result<(&str, &str), Error> {
    owned
        .id
        .as_deref()
        .and_then(|id| id.rsplit_once('/'))
        .ok_or_else(|| Error::MissingId {
            name: owned.name.clone(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const RECORDS: &str = include_str!("../tests/fixtures/porkbun_dns_records.json");
    const DOMAINS: &str = include_str!("../tests/fixtures/porkbun_domains.json");
    const INVALID_KEY: &str = include_str!("../tests/fixtures/porkbun_error.json");

    fn reply(status: u16, body: &str) -> Reply {
        Reply {
            url: format!("{API}/dns/retrieve/example.com"),
            status,
            body: body.into(),
        }
    }

    #[test]
    fn porkbun_root_domain() {
        let domains: BTreeSet<String> = ["example.com", "example.co.uk", "play.example.net"]
            .map(String::from)
            .into();
        let cases = [
            ("survival.example.com", Some("example.com")),
            ("example.com", Some("example.com")),
            ("a.b.example.co.uk", Some("example.co.uk")),
            ("mc.play.example.net", Some("play.example.net")),
            ("example.net", None),
            ("survival.example.org", None),
        ];
        for (name, expected) in cases {
            assert_eq!(root_domain(name, &domains), expected, "{name}");
        }
    }

    #[test]
    fn parses_porkbun_domain_list() {
        let list: DomainList = checked(&reply(200, DOMAINS)).unwrap();
        let names: Vec<&str> = list
            .domains
            .iter()
            .map(|entry| entry.domain.as_str())
            .collect();
        assert_eq!(names, ["example.com", "Example.co.uk"]);
    }

    #[test]
    fn splits_porkbun_records_by_notes() {
        let list: RecordList = checked(&reply(200, RECORDS)).unwrap();
        let (owned, foreign) = split("example.com", list.records).unwrap();
        assert_eq!(
            owned,
            [Owned {
                name: "survival.example.com".into(),
                ip: Ipv4Addr::new(198, 51, 100, 4),
                id: Some("example.com/106926652".into()),
            }]
        );
        assert_eq!(
            foreign,
            ["example.com".to_owned(), "www.example.com".to_owned()].into()
        );
        assert_eq!(ids(&owned[0]), Ok(("example.com", "106926652")));
    }

    #[test]
    fn porkbun_record_fields_use_subdomain_and_notes() {
        let ip = Ipv4Addr::new(1, 2, 3, 4);
        let record = Desired {
            name: "survival.example.com".into(),
            ip,
        };
        assert_eq!(
            serde_json::to_value(Signed {
                apikey: "pk",
                secretapikey: "sk",
                fields: record_fields(&record, "example.com")
            })
            .unwrap(),
            serde_json::json!({
                "apikey": "pk",
                "secretapikey": "sk",
                "name": "survival",
                "type": "A",
                "content": "1.2.3.4",
                "notes": "managed-by-mcctl"
            })
        );
        let apex = Desired {
            name: "example.com".into(),
            ip,
        };
        assert_eq!(record_fields(&apex, "example.com").name, "");
    }

    #[test]
    fn reports_porkbun_errors() {
        let Err(err) = checked::<IgnoredAny>(&reply(400, INVALID_KEY)) else {
            panic!("an invalid key must fail");
        };
        assert_eq!(
            err,
            Error::Rejected {
                url: format!("{API}/dns/retrieve/example.com"),
                status: 400,
                message: "Invalid API key. (INVALID_API_KEYS_001)".into()
            }
        );
    }
}
