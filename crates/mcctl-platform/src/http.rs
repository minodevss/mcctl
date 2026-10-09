use std::io::Read;

use serde::de::DeserializeOwned;
use ureq::Agent;

use crate::error::Error;

// PaperMC rejects generic user agents; it asks for the software name and a contact URL.
const USER_AGENT: &str = concat!(
    "mcctl/",
    env!("CARGO_PKG_VERSION"),
    " (+",
    env!("CARGO_PKG_REPOSITORY"),
    ")"
);
const METADATA_LIMIT: u64 = 32 * 1024 * 1024;

pub(crate) fn http_error(url: &str, source: ureq::Error) -> Error {
    Error::Http {
        url: url.to_owned(),
        source: Box::new(source),
    }
}

fn get(agent: &Agent, url: &str) -> Result<ureq::Body, Error> {
    agent
        .get(url)
        .header("User-Agent", USER_AGENT)
        .call()
        .map(ureq::http::Response::into_body)
        .map_err(|source| http_error(url, source))
}

pub(crate) fn get_bytes(agent: &Agent, url: &str) -> Result<Vec<u8>, Error> {
    get(agent, url)?
        .into_with_config()
        .limit(METADATA_LIMIT)
        .read_to_vec()
        .map_err(|source| http_error(url, source))
}

pub(crate) fn get_json<T: DeserializeOwned>(agent: &Agent, url: &str) -> Result<T, Error> {
    parse_json(url, &get_bytes(agent, url)?)
}

pub(crate) fn parse_json<T: DeserializeOwned>(url: &str, bytes: &[u8]) -> Result<T, Error> {
    serde_json::from_slice(bytes).map_err(|source| Error::Json {
        url: url.to_owned(),
        source,
    })
}

pub(crate) fn get_stream(agent: &Agent, url: &str) -> Result<impl Read, Error> {
    Ok(get(agent, url)?.into_reader())
}

pub(crate) fn is_not_found(error: &Error) -> bool {
    matches!(error, Error::Http { source, .. } if matches!(**source, ureq::Error::StatusCode(404)))
}

/// Dropped connections and server errors are worth another try; 4xx answers are not.
pub(crate) fn is_transient(error: &Error) -> bool {
    match error {
        Error::Http { source, .. } => !matches!(**source, ureq::Error::StatusCode(400..=499)),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn http(source: ureq::Error) -> Error {
        http_error("https://example.com", source)
    }

    #[test]
    fn retries_only_transient_failures() {
        let dropped = std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "peer disconnected");
        assert!(is_transient(&http(ureq::Error::Io(dropped))));
        assert!(is_transient(&http(ureq::Error::StatusCode(503))));
        assert!(!is_transient(&http(ureq::Error::StatusCode(404))));
        assert!(is_not_found(&http(ureq::Error::StatusCode(404))));
        let mismatch = Error::ChecksumMismatch {
            url: String::new(),
            expected: String::new(),
            actual: String::new(),
        };
        assert!(!is_transient(&mismatch));
    }
}
