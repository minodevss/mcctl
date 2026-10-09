/// Reduces a raw handshake host to the lowercase name used for routing.
pub fn normalize_host(raw: &str) -> String {
    // Forge appends "\0FML\0", "\0FML2\0" or "\0FML3\0"; route on the part before the first NUL.
    let host = before(raw, "\0");
    // TCPShield-style proxies append "///<client ip>///<timestamp>".
    let host = before(host, "///");
    let host = host.strip_suffix('.').unwrap_or(host);
    host.to_ascii_lowercase()
}

fn before<'a>(text: &'a str, marker: &str) -> &'a str {
    text.split_once(marker).map_or(text, |(head, _)| head)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_normalizes(cases: &[(&str, &str)]) {
        for &(raw, expected) in cases {
            assert_eq!(normalize_host(raw), expected, "{raw:?}");
        }
    }

    #[test]
    fn keeps_plain_host() {
        assert_normalizes(&[("play.example.com", "play.example.com"), ("", "")]);
    }

    #[test]
    fn cuts_forge_marker() {
        assert_normalizes(&[
            ("play.example.com\0FML\0", "play.example.com"),
            ("play.example.com\0FML2\0", "play.example.com"),
            ("play.example.com\0FML3\0", "play.example.com"),
        ]);
    }

    #[test]
    fn cuts_tcpshield_suffix() {
        assert_normalizes(&[(
            "play.example.com///203.0.113.7:51234///1700000000",
            "play.example.com",
        )]);
    }

    #[test]
    fn strips_trailing_dot() {
        assert_normalizes(&[
            ("play.example.com.", "play.example.com"),
            ("play.example.com..", "play.example.com."),
            ("play.example.com.\0FML3\0", "play.example.com"),
        ]);
    }

    #[test]
    fn lowercases_host() {
        assert_normalizes(&[
            ("Play.Example.COM", "play.example.com"),
            ("PLAY.EXAMPLE.COM.\0FML2\0", "play.example.com"),
        ]);
    }
}
