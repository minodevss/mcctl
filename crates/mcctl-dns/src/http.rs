use std::time::Duration;

use serde::de::DeserializeOwned;
use ureq::Body;
use ureq::http::{HeaderMap, Response, StatusCode};

use crate::Error;

const EXCERPT_CHARS: usize = 200;

pub(crate) struct Reply {
    pub(crate) url: String,
    pub(crate) status: u16,
    pub(crate) body: String,
}

impl Reply {
    pub(crate) fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub(crate) fn json<T: DeserializeOwned>(&self) -> Result<T, Error> {
        serde_json::from_str(&self.body).map_err(|err| {
            if self.is_success() {
                Error::Response {
                    url: self.url.clone(),
                    reason: err.to_string(),
                }
            } else {
                self.rejected(excerpt(&self.body))
            }
        })
    }

    pub(crate) fn rejected(&self, message: String) -> Error {
        Error::Rejected {
            url: self.url.clone(),
            status: self.status,
            message,
        }
    }
}

/// `url` must be the endpoint without its query string; it ends up in error messages.
pub(crate) fn receive(
    url: &str,
    sent: Result<Response<Body>, ureq::Error>,
) -> Result<Reply, Error> {
    let mut response = sent.map_err(|err| transport(url, &err))?;
    let status = response.status();
    if status == StatusCode::TOO_MANY_REQUESTS {
        return Err(Error::RateLimited {
            url: url.to_owned(),
            retry_after: retry_after(response.headers()),
        });
    }
    let body = response
        .body_mut()
        .read_to_string()
        .map_err(|err| transport(url, &err))?;
    Ok(Reply {
        url: url.to_owned(),
        status: status.as_u16(),
        body,
    })
}

pub(crate) fn excerpt(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("empty response");
    line.chars().take(EXCERPT_CHARS).collect()
}

fn transport(url: &str, err: &ureq::Error) -> Error {
    let reason = match err {
        ureq::Error::BadUri(_) => "invalid url".to_owned(),
        ureq::Error::RequireHttpsOnly(_) => "refused to send over plain http".to_owned(),
        other => other.to_string(),
    };
    Error::Transport {
        url: url.to_owned(),
        reason,
    }
}

fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    let seconds = headers
        .get("retry-after")?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()?;
    Some(Duration::from_secs(seconds))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(status: u16, retry_after: Option<&str>, body: &str) -> Response<Body> {
        let mut builder = Response::builder().status(status);
        if let Some(value) = retry_after {
            builder = builder.header("retry-after", value);
        }
        builder.body(Body::builder().data(body.to_owned())).unwrap()
    }

    #[test]
    fn rate_limit_carries_retry_after() {
        let cases = [
            (Some("30"), Some(Duration::from_secs(30))),
            (Some("Wed, 21 Oct 2026 07:28:00 GMT"), None),
            (None, None),
        ];
        for (header, expected) in cases {
            let Err(err) = receive(
                "https://api.example.test/zones",
                Ok(response(429, header, "")),
            ) else {
                panic!("429 must be an error");
            };
            assert_eq!(
                err,
                Error::RateLimited {
                    url: "https://api.example.test/zones".into(),
                    retry_after: expected
                }
            );
        }
    }

    #[test]
    fn transport_errors_never_echo_the_query() {
        let secret_url =
            "https://www.duckdns.org/update?domains=alice&token=secret-duck-token".to_owned();
        for err in [
            ureq::Error::BadUri(secret_url.clone()),
            ureq::Error::RequireHttpsOnly(secret_url),
        ] {
            let Err(err) = receive("https://www.duckdns.org/update", Err(err)) else {
                panic!("transport failure must be an error");
            };
            assert!(!err.to_string().contains("secret-duck-token"), "{err}");
        }
    }

    #[test]
    fn non_json_error_body_becomes_rejection() {
        let reply = receive(
            "https://api.example.test/x",
            Ok(response(502, None, "\n<html>bad gateway</html>")),
        )
        .unwrap();
        let err = reply.json::<serde_json::Value>().unwrap_err();
        assert_eq!(
            err,
            Error::Rejected {
                url: "https://api.example.test/x".into(),
                status: 502,
                message: "<html>bad gateway</html>".into()
            }
        );
    }
}
