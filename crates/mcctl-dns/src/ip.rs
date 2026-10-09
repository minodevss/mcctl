use std::net::Ipv4Addr;

use crate::{Error, http};

const TRACE_URL: &str = "https://api.cloudflare.com/cdn-cgi/trace";
const PLAIN_URL: &str = "https://checkip.amazonaws.com";

/// Asks two independent sources over IPv4 and errors unless both agree on one public address.
pub fn public_ipv4(agent: &ureq::Agent) -> Result<Ipv4Addr, Error> {
    let traced = ask(agent, TRACE_URL, parse_trace)?;
    let plain = ask(agent, PLAIN_URL, parse_plain)?;
    agree(traced, plain)
}

fn ask(
    agent: &ureq::Agent,
    url: &str,
    parse: fn(&str) -> Option<Ipv4Addr>,
) -> Result<Ipv4Addr, Error> {
    let reply = http::receive(url, agent.get(url).call())?;
    if !reply.is_success() {
        return Err(reply.rejected(http::excerpt(&reply.body)));
    }
    parse(&reply.body).ok_or_else(|| Error::Response {
        url: url.to_owned(),
        reason: "no ipv4 address in the body".into(),
    })
}

pub(crate) fn parse_trace(body: &str) -> Option<Ipv4Addr> {
    body.lines()
        .find_map(|line| line.strip_prefix("ip="))?
        .trim()
        .parse()
        .ok()
}

pub(crate) fn parse_plain(body: &str) -> Option<Ipv4Addr> {
    body.trim().parse().ok()
}

pub(crate) fn agree(first: Ipv4Addr, second: Ipv4Addr) -> Result<Ipv4Addr, Error> {
    if first != second {
        return Err(Error::IpMismatch { first, second });
    }
    if !is_public(first) {
        return Err(Error::NotPublic { ip: first });
    }
    Ok(first)
}

pub(crate) fn is_public(ip: Ipv4Addr) -> bool {
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_documentation()
        || ip.is_multicast()
        || is_special_purpose(ip.octets()))
}

fn is_special_purpose(octets: [u8; 4]) -> bool {
    matches!(
        octets,
        [0 | 240..=255, ..] | [100, 64..=127, ..] | [192, 0, 0, _] | [198, 18 | 19, ..]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRACE: &str = include_str!("../tests/fixtures/cloudflare_trace.txt");

    #[test]
    fn parses_cloudflare_trace() {
        assert_eq!(parse_trace(TRACE), Some(Ipv4Addr::new(203, 0, 113, 7)));
        assert_eq!(parse_trace("fl=1\nh=api.cloudflare.com\n"), None);
        assert_eq!(parse_trace("ip=2001:db8::1\n"), None);
    }

    #[test]
    fn parses_plain_ip() {
        let cases = [
            ("203.0.113.7\n", Some(Ipv4Addr::new(203, 0, 113, 7))),
            ("  198.51.100.20\r\n", Some(Ipv4Addr::new(198, 51, 100, 20))),
            ("", None),
            ("<html>", None),
            ("2001:db8::1\n", None),
        ];
        for (body, expected) in cases {
            assert_eq!(parse_plain(body), expected, "{body:?}");
        }
    }

    #[test]
    fn rejects_private_and_cgnat() {
        let cases = [
            ("1.1.1.1", true),
            ("8.8.8.8", true),
            ("100.63.255.255", true),
            ("100.128.0.1", true),
            ("172.32.0.1", true),
            ("10.0.0.5", false),
            ("172.16.4.1", false),
            ("192.168.1.10", false),
            ("100.64.0.1", false),
            ("100.127.255.254", false),
            ("127.0.0.1", false),
            ("169.254.10.1", false),
            ("0.0.0.0", false),
            ("192.0.2.1", false),
            ("198.51.100.1", false),
            ("203.0.113.1", false),
            ("198.18.0.1", false),
            ("224.0.0.1", false),
            ("255.255.255.255", false),
        ];
        for (ip, expected) in cases {
            let ip: Ipv4Addr = ip.parse().unwrap();
            assert_eq!(is_public(ip), expected, "{ip}");
        }
    }

    #[test]
    fn requires_both_sources_to_agree() {
        let public = Ipv4Addr::new(1, 1, 1, 1);
        let other = Ipv4Addr::new(8, 8, 8, 8);
        let private = Ipv4Addr::new(192, 168, 0, 2);
        assert_eq!(agree(public, public), Ok(public));
        assert_eq!(
            agree(public, other),
            Err(Error::IpMismatch {
                first: public,
                second: other
            })
        );
        assert_eq!(
            agree(private, private),
            Err(Error::NotPublic { ip: private })
        );
    }
}
