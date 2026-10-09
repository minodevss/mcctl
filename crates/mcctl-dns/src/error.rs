use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::time::Duration;

/// No variant ever carries a token, API key, or a URL query string.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("request to {url} failed: {reason}")]
    Transport { url: String, reason: String },
    #[error("{url} rejected the request with http {status}: {message}")]
    Rejected {
        url: String,
        status: u16,
        message: String,
    },
    #[error("{url} is rate limiting requests")]
    RateLimited {
        url: String,
        retry_after: Option<Duration>,
    },
    #[error("unexpected response from {url}: {reason}")]
    Response { url: String, reason: String },
    #[error("public ip sources disagree: {first} and {second}")]
    IpMismatch { first: Ipv4Addr, second: Ipv4Addr },
    #[error("{ip} is not a public ipv4 address")]
    NotPublic { ip: Ipv4Addr },
    #[error("no dns zone in the account covers {name}")]
    NoZone { name: String },
    #[error("{name} is not a duckdns.org name")]
    NotDuckDns { name: String },
    #[error("dns record {name} has no provider id")]
    MissingId { name: String },
    #[error("dns hook {command} could not run: {reason}")]
    HookIo { command: PathBuf, reason: String },
    #[error("dns hook {command} did not finish within {} s", timeout.as_secs())]
    HookTimeout { command: PathBuf, timeout: Duration },
    #[error("dns hook {command} exited with {status}: {stderr}")]
    HookFailed {
        command: PathBuf,
        status: String,
        stderr: String,
    },
}
