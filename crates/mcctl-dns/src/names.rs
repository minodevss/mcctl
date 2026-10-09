pub(crate) const OWNER_MARK: &str = "managed-by-mcctl";

pub(crate) fn normalize(name: &str) -> String {
    name.trim().trim_end_matches('.').to_ascii_lowercase()
}

/// The name itself, then each parent, longest first; stops before the bare top-level label.
pub(crate) fn suffixes(name: &str) -> impl Iterator<Item = &str> {
    std::iter::successors(Some(name), |rest| {
        rest.split_once('.').map(|(_, parent)| parent)
    })
    .filter(|candidate| candidate.contains('.'))
}

/// The part of `name` left of `zone`; empty for the zone apex.
pub(crate) fn relative<'a>(name: &'a str, zone: &str) -> &'a str {
    if name == zone {
        return "";
    }
    name.strip_suffix(zone)
        .and_then(|rest| rest.strip_suffix('.'))
        .unwrap_or(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_case_and_trailing_dot() {
        let cases = [
            ("Survival.Example.COM.", "survival.example.com"),
            ("survival.example.com", "survival.example.com"),
            (" lobby.example.com ", "lobby.example.com"),
        ];
        for (input, expected) in cases {
            assert_eq!(normalize(input), expected, "{input}");
        }
    }

    #[test]
    fn cloudflare_zone_suffixes_longest_first() {
        let cases: [(&str, &[&str]); 4] = [
            (
                "a.b.example.com",
                &["a.b.example.com", "b.example.com", "example.com"],
            ),
            (
                "survival.example.co.uk",
                &["survival.example.co.uk", "example.co.uk", "co.uk"],
            ),
            ("example.com", &["example.com"]),
            ("localhost", &[]),
        ];
        for (name, expected) in cases {
            assert_eq!(suffixes(name).collect::<Vec<_>>(), expected, "{name}");
        }
    }

    #[test]
    fn relative_name_strips_zone() {
        let cases = [
            ("survival.example.com", "example.com", "survival"),
            ("a.b.example.com", "example.com", "a.b"),
            ("example.com", "example.com", ""),
        ];
        for (name, zone, expected) in cases {
            assert_eq!(relative(name, zone), expected, "{name} in {zone}");
        }
    }
}
