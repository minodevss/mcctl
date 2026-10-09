use std::collections::{BTreeMap, BTreeSet};
use std::net::Ipv4Addr;
use std::sync::{Mutex, MutexGuard, PoisonError};

use serde::de::{DeserializeOwned, IgnoredAny};
use serde::{Deserialize, Serialize};

use crate::http::{self, Reply};
use crate::names::{OWNER_MARK, normalize, suffixes};
use crate::{Desired, Error, Listing, Owned, Provider};

const API: &str = "https://api.cloudflare.com/client/v4";
const TTL_SECONDS: u32 = 60;
const RECORDS_PER_PAGE: u32 = 1000;

pub(crate) struct Cloudflare {
    agent: ureq::Agent,
    bearer: String,
    zones: Mutex<BTreeMap<String, Option<String>>>,
}

impl Cloudflare {
    pub(crate) fn new(agent: ureq::Agent, token: &str) -> Self {
        Self {
            agent,
            bearer: format!("Bearer {token}"),
            zones: Mutex::new(BTreeMap::new()),
        }
    }

    fn zones(&self) -> MutexGuard<'_, BTreeMap<String, Option<String>>> {
        self.zones.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn zone_of(&self, name: &str) -> Result<String, Error> {
        let cached = self.zones().get(name).cloned();
        let zone = match cached {
            Some(zone) => zone,
            None => self.find_zone(name, &mut BTreeMap::new())?,
        };
        zone.ok_or_else(|| Error::NoZone {
            name: name.to_owned(),
        })
    }

    fn find_zone(
        &self,
        name: &str,
        probed: &mut BTreeMap<String, Option<String>>,
    ) -> Result<Option<String>, Error> {
        for suffix in suffixes(name) {
            let zone = if let Some(known) = probed.get(suffix) {
                known.clone()
            } else {
                let zone = self.zone_named(suffix)?;
                probed.insert(suffix.to_owned(), zone.clone());
                zone
            };
            if zone.is_some() {
                return Ok(zone);
            }
        }
        Ok(None)
    }

    fn zone_named(&self, suffix: &str) -> Result<Option<String>, Error> {
        let url = format!("{API}/zones");
        let request = self
            .agent
            .get(&url)
            .header("Authorization", &self.bearer)
            .query("name", suffix);
        let envelope: Envelope<Vec<Zone>> = parse(&http::receive(&url, request.call())?)?;
        Ok(zone_matching(suffix, envelope.result.unwrap_or_default()))
    }

    fn records(&self, zone: &str) -> Result<Vec<Record>, Error> {
        let url = format!("{API}/zones/{zone}/dns_records");
        let mut records = Vec::new();
        let mut page_number = 1_u32;
        loop {
            let request = self
                .agent
                .get(&url)
                .header("Authorization", &self.bearer)
                .query("type", "A")
                .query("per_page", RECORDS_PER_PAGE.to_string())
                .query("page", page_number.to_string());
            let envelope: Envelope<Vec<Record>> = parse(&http::receive(&url, request.call())?)?;
            let (page, last) = envelope.into_page(page_number);
            records.extend(page);
            if last {
                return Ok(records);
            }
            page_number += 1;
        }
    }

    fn write(
        &self,
        request: ureq::RequestBuilder<ureq::typestate::WithBody>,
        url: &str,
        record: &Desired,
    ) -> Result<(), Error> {
        let sent = request
            .header("Authorization", &self.bearer)
            .send_json(record_body(record));
        parse::<IgnoredAny>(&http::receive(url, sent)?).map(drop)
    }
}

impl Provider for Cloudflare {
    fn list(&self, names: &BTreeSet<String>) -> Result<Listing, Error> {
        let mut probed = BTreeMap::new();
        let mut zones = BTreeMap::new();
        for name in names {
            zones.insert(name.clone(), self.find_zone(name, &mut probed)?);
        }
        let distinct: BTreeSet<String> = zones.values().flatten().cloned().collect();
        *self.zones() = zones;
        let mut listing = Listing::default();
        for zone in distinct {
            let (owned, foreign) = split(&zone, self.records(&zone)?)?;
            listing.owned.extend(owned);
            listing.foreign.extend(foreign);
        }
        Ok(listing)
    }

    fn upsert(&self, record: &Desired, existing: Option<&Owned>) -> Result<(), Error> {
        if let Some(owned) = existing {
            let (zone, id) = ids(owned)?;
            let url = format!("{API}/zones/{zone}/dns_records/{id}");
            self.write(self.agent.patch(&url), &url, record)
        } else {
            let zone = self.zone_of(&record.name)?;
            let url = format!("{API}/zones/{zone}/dns_records");
            self.write(self.agent.post(&url), &url, record)
        }
    }

    fn delete(&self, existing: &Owned) -> Result<(), Error> {
        let (zone, id) = ids(existing)?;
        let url = format!("{API}/zones/{zone}/dns_records/{id}");
        let sent = self
            .agent
            .delete(&url)
            .header("Authorization", &self.bearer)
            .call();
        parse::<IgnoredAny>(&http::receive(&url, sent)?).map(drop)
    }
}

#[derive(Deserialize)]
struct Envelope<T> {
    success: bool,
    #[serde(default)]
    errors: Vec<ApiMessage>,
    result: Option<T>,
    result_info: Option<ResultInfo>,
}

impl<T> Envelope<Vec<T>> {
    fn into_page(self, page_number: u32) -> (Vec<T>, bool) {
        let items = self.result.unwrap_or_default();
        let last = items.is_empty()
            || self
                .result_info
                .is_none_or(|info| page_number >= info.total_pages);
        (items, last)
    }
}

#[derive(Deserialize)]
struct ApiMessage {
    code: i64,
    message: String,
}

#[derive(Deserialize)]
struct ResultInfo {
    total_pages: u32,
}

#[derive(Deserialize)]
struct Zone {
    id: String,
    name: String,
}

#[derive(Deserialize)]
struct Record {
    id: String,
    name: String,
    #[serde(rename = "type")]
    kind: String,
    content: String,
    comment: Option<String>,
}

#[derive(Serialize)]
struct RecordBody<'a> {
    #[serde(rename = "type")]
    kind: &'a str,
    name: &'a str,
    content: Ipv4Addr,
    ttl: u32,
    proxied: bool,
    comment: &'a str,
}

fn record_body(record: &Desired) -> RecordBody<'_> {
    RecordBody {
        kind: "A",
        name: &record.name,
        content: record.ip,
        ttl: TTL_SECONDS,
        proxied: false,
        comment: OWNER_MARK,
    }
}

fn parse<T: DeserializeOwned>(reply: &Reply) -> Result<Envelope<T>, Error> {
    let envelope: Envelope<T> = reply.json()?;
    if !envelope.success || !reply.is_success() {
        return Err(reply.rejected(describe(&envelope.errors)));
    }
    Ok(envelope)
}

fn describe(errors: &[ApiMessage]) -> String {
    if errors.is_empty() {
        return "no error details".into();
    }
    errors
        .iter()
        .map(|error| format!("{} (code {})", error.message, error.code))
        .collect::<Vec<_>>()
        .join("; ")
}

fn zone_matching(suffix: &str, zones: Vec<Zone>) -> Option<String> {
    zones
        .into_iter()
        .find(|zone| normalize(&zone.name) == suffix)
        .map(|zone| zone.id)
}

fn split(zone: &str, records: Vec<Record>) -> Result<(Vec<Owned>, BTreeSet<String>), Error> {
    let mut owned = Vec::new();
    let mut foreign = BTreeSet::new();
    for record in records.into_iter().filter(|record| record.kind == "A") {
        if record.comment.as_deref() != Some(OWNER_MARK) {
            foreign.insert(normalize(&record.name));
            continue;
        }
        let ip = record.content.parse().map_err(|_| Error::Response {
            url: format!("{API}/zones/{zone}/dns_records"),
            reason: format!(
                "record {} holds {:?}, not an ipv4 address",
                record.name, record.content
            ),
        })?;
        owned.push(Owned {
            name: normalize(&record.name),
            ip,
            id: Some(format!("{zone}/{}", record.id)),
        });
    }
    Ok((owned, foreign))
}

fn ids(owned: &Owned) -> Result<(&str, &str), Error> {
    owned
        .id
        .as_deref()
        .and_then(|id| id.split_once('/'))
        .ok_or_else(|| Error::MissingId {
            name: owned.name.clone(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const RECORDS: &str = include_str!("../tests/fixtures/cloudflare_dns_records.json");
    const ZONES: &str = include_str!("../tests/fixtures/cloudflare_zones.json");
    const AUTH_ERROR: &str = include_str!("../tests/fixtures/cloudflare_error.json");
    const ZONE: &str = "023e105f4ecef8ad9ca31a8372d0c353";

    fn reply(status: u16, body: &str) -> Reply {
        Reply {
            url: format!("{API}/zones"),
            status,
            body: body.into(),
        }
    }

    fn records() -> Vec<Record> {
        let envelope: Envelope<Vec<Record>> = parse(&reply(200, RECORDS)).unwrap();
        envelope.into_page(1).0
    }

    #[test]
    fn parses_cloudflare_record_listing() {
        let envelope: Envelope<Vec<Record>> = parse(&reply(200, RECORDS)).unwrap();
        let (records, last) = envelope.into_page(1);
        assert!(last);
        let names: Vec<&str> = records.iter().map(|record| record.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "survival.example.com",
                "Creative.Example.com",
                "www.example.com",
                "lobby.example.com"
            ]
        );
        assert_eq!(records[0].id, ZONE);
        assert_eq!(records[0].content, "198.51.100.4");
        assert_eq!(records[2].comment, None);
    }

    #[test]
    fn splits_owned_and_foreign_by_comment() {
        let (owned, foreign) = split(ZONE, records()).unwrap();
        assert_eq!(
            owned,
            [
                Owned {
                    name: "survival.example.com".into(),
                    ip: Ipv4Addr::new(198, 51, 100, 4),
                    id: Some(format!("{ZONE}/023e105f4ecef8ad9ca31a8372d0c353")),
                },
                Owned {
                    name: "creative.example.com".into(),
                    ip: Ipv4Addr::new(198, 51, 100, 9),
                    id: Some(format!("{ZONE}/1c3e1a6ff2b5c1c0a1e9a2b7d38e6a11")),
                },
            ]
        );
        assert_eq!(
            foreign,
            ["lobby.example.com".to_owned(), "www.example.com".to_owned()].into()
        );
    }

    #[test]
    fn owned_ids_round_trip_zone_and_record() {
        let (owned, _) = split(ZONE, records()).unwrap();
        assert_eq!(
            ids(&owned[0]),
            Ok((ZONE, "023e105f4ecef8ad9ca31a8372d0c353"))
        );
        let bare = Owned {
            name: "a.example.com".into(),
            ip: Ipv4Addr::new(1, 2, 3, 4),
            id: None,
        };
        assert_eq!(
            ids(&bare),
            Err(Error::MissingId {
                name: "a.example.com".into()
            })
        );
    }

    #[test]
    fn picks_zone_with_exact_name() {
        let envelope: Envelope<Vec<Zone>> = parse(&reply(200, ZONES)).unwrap();
        let zones = envelope.result.unwrap();
        assert_eq!(zone_matching("example.com", zones), Some(ZONE.to_owned()));
        let envelope: Envelope<Vec<Zone>> = parse(&reply(200, ZONES)).unwrap();
        assert_eq!(
            zone_matching("sub.example.com", envelope.result.unwrap()),
            None
        );
    }

    #[test]
    fn pages_until_total_pages() {
        let page = |number: u32, total: u32| {
            Envelope {
                success: true,
                errors: Vec::new(),
                result: Some(vec![1]),
                result_info: Some(ResultInfo { total_pages: total }),
            }
            .into_page(number)
            .1
        };
        assert!(!page(1, 3));
        assert!(page(3, 3));
        assert!(page(1, 0));
    }

    #[test]
    fn reports_cloudflare_api_errors() {
        let Err(err) = parse::<IgnoredAny>(&reply(403, AUTH_ERROR)) else {
            panic!("an authentication error must fail");
        };
        assert_eq!(
            err,
            Error::Rejected {
                url: format!("{API}/zones"),
                status: 403,
                message: "Authentication error (code 10000)".into()
            }
        );
    }

    #[test]
    fn record_body_is_unproxied_owned_a_record() {
        let record = Desired {
            name: "survival.example.com".into(),
            ip: Ipv4Addr::new(1, 2, 3, 4),
        };
        assert_eq!(
            serde_json::to_value(record_body(&record)).unwrap(),
            serde_json::json!({
                "type": "A",
                "name": "survival.example.com",
                "content": "1.2.3.4",
                "ttl": 60,
                "proxied": false,
                "comment": "managed-by-mcctl"
            })
        );
    }
}
