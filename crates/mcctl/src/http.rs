use std::time::Duration;

use serde::de::DeserializeOwned;

use crate::error::Error;

pub(crate) const USER_AGENT: &str = concat!("mcctl/", env!("CARGO_PKG_VERSION"));
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(60);
const JSON_LIMIT: u64 = 4 * 1024 * 1024;

/// HTTPS-only agent for downloads; 4xx and 5xx answers are errors.
pub(crate) fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .https_only(true)
        .user_agent(USER_AGENT)
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .timeout_recv_response(Some(RESPONSE_TIMEOUT))
        .build()
        .new_agent()
}

fn failed(url: &str, err: &ureq::Error) -> Error {
    Error::Http {
        url: url.to_owned(),
        reason: err.to_string(),
    }
}

pub(crate) fn get_bytes(agent: &ureq::Agent, url: &str, limit: u64) -> Result<Vec<u8>, Error> {
    let response = agent.get(url).call().map_err(|err| failed(url, &err))?;
    response
        .into_body()
        .into_with_config()
        .limit(limit)
        .read_to_vec()
        .map_err(|err| failed(url, &err))
}

pub(crate) fn get_json<T: DeserializeOwned>(agent: &ureq::Agent, url: &str) -> Result<T, Error> {
    let bytes = get_bytes(agent, url, JSON_LIMIT)?;
    serde_json::from_slice(&bytes).map_err(|err| Error::Http {
        url: url.to_owned(),
        reason: format!("unexpected answer: {err}"),
    })
}
