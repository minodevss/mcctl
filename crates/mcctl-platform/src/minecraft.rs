#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Release {
    major: u32,
    minor: u32,
    patch: u32,
}

impl Release {
    const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }
}

const TEMURIN_LTS: [u8; 5] = [8, 11, 17, 21, 25];
const NEWEST_JAVA: u8 = 25;

/// Reads "1.21.1", "26.3" or "1.21-pre1" as a release number; snapshots like "24w14a" give `None`.
pub(crate) fn parse_release(id: &str) -> Option<Release> {
    let end = id
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(id.len());
    let (numeric, rest) = id.split_at(end);
    if rest.starts_with(|c: char| c.is_ascii_alphanumeric()) {
        return None;
    }
    let mut parts = numeric.split('.').map(|part| part.parse::<u32>().ok());
    let major = parts.next()??;
    let minor = parts.next()??;
    let patch = match parts.next() {
        None => 0,
        Some(patch) => patch?,
    };
    if parts.next().is_some() {
        return None;
    }
    Some(Release::new(major, minor, patch))
}

pub(crate) fn is_plain_release(id: &str) -> bool {
    !id.is_empty()
        && id.chars().all(|c| c.is_ascii_digit() || c == '.')
        && parse_release(id).is_some()
}

/// Java major to run `minecraft` with. A jar's own `java_version` wins over the table,
/// rounded up to a Temurin LTS because Temurin ships no JRE for majors like 16.
pub fn java_major(minecraft: &str, from_jar: Option<u8>) -> u8 {
    from_jar.map_or_else(|| java_from_table(minecraft), round_up_to_lts)
}

fn java_from_table(minecraft: &str) -> u8 {
    match parse_release(minecraft) {
        Some(release) if release < Release::new(1, 17, 0) => 8,
        Some(release) if release < Release::new(1, 20, 5) => 17,
        Some(release) if release < Release::new(1, 22, 0) => 21,
        Some(_) | None => NEWEST_JAVA,
    }
}

fn round_up_to_lts(major: u8) -> u8 {
    TEMURIN_LTS
        .into_iter()
        .find(|&lts| lts >= major)
        .unwrap_or(major)
}

/// Minecraft version a `NeoForge` version targets: "21.1.233" is 1.21.1, "21.0.5" is 1.21,
/// "26.3.0.58-beta" is 26.3 and "26.1.2.7" is 26.1.2.
pub(crate) fn neoforge_minecraft(neoforge: &str) -> Option<String> {
    match leading_numbers(neoforge)?.as_slice() {
        [major @ 20..=21, 0, ..] => Some(format!("1.{major}")),
        [major @ 20..=21, minor, ..] => Some(format!("1.{major}.{minor}")),
        [year, drop, 0, ..] if *year >= 26 => Some(format!("{year}.{drop}")),
        [year, drop, hotfix, ..] if *year >= 26 => Some(format!("{year}.{drop}.{hotfix}")),
        _ => None,
    }
}

/// Dotted numbers before any `-` or `+` suffix: "26.1.0.0-alpha.1+snapshot-1" gives [26, 1, 0, 0].
pub(crate) fn leading_numbers(version: &str) -> Option<Vec<u32>> {
    let numeric = version.split(['-', '+']).next()?;
    numeric
        .split('.')
        .map(|part| part.parse::<u32>().ok())
        .collect()
}

/// Accepts ids that are safe inside a URL path segment and a file name.
pub(crate) fn is_safe_version_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && !id.starts_with(['.', '-'])
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn java_table_covers_old_and_year_versions() {
        let cases = [
            ("1.7.10", 8),
            ("1.12.2", 8),
            ("1.16.5", 8),
            ("1.17", 17),
            ("1.17.1", 17),
            ("1.18.2", 17),
            ("1.20.4", 17),
            ("1.20.5", 21),
            ("1.20.6", 21),
            ("1.21", 21),
            ("1.21.11", 21),
            ("1.21.1-pre1", 21),
            ("26.1", 25),
            ("26.3", 25),
            ("26.1.2", 25),
            ("26.4-snapshot-3", 25),
            ("24w14a", 25),
            ("b1.7.3", 25),
            ("", 25),
        ];
        for (minecraft, java) in cases {
            assert_eq!(java_major(minecraft, None), java, "{minecraft}");
        }
    }

    #[test]
    fn jar_java_version_is_rounded_up_to_lts() {
        let cases = [
            (8, 8),
            (16, 17),
            (17, 17),
            (21, 21),
            (22, 25),
            (25, 25),
            (29, 29),
        ];
        for (from_jar, java) in cases {
            assert_eq!(java_major("1.17.1", Some(from_jar)), java, "{from_jar}");
        }
    }

    #[test]
    fn parses_release_numbers() {
        let cases = [
            ("1.21.1", Some(Release::new(1, 21, 1))),
            ("1.21", Some(Release::new(1, 21, 0))),
            ("26.3", Some(Release::new(26, 3, 0))),
            ("26.1.2", Some(Release::new(26, 1, 2))),
            ("1.18_pre1", Some(Release::new(1, 18, 0))),
            ("1.14 Pre-Release 1", Some(Release::new(1, 14, 0))),
            ("24w14a", None),
            ("1.RV-Pre1", None),
            ("1", None),
            ("1.2.3.4", None),
            ("", None),
        ];
        for (id, release) in cases {
            assert_eq!(parse_release(id), release, "{id}");
        }
    }

    #[test]
    fn maps_neoforge_versions_to_minecraft() {
        let cases = [
            ("20.2.93", Some("1.20.2")),
            ("20.4.251", Some("1.20.4")),
            ("21.0.167", Some("1.21")),
            ("21.1.233", Some("1.21.1")),
            ("21.11.45", Some("1.21.11")),
            ("26.1.0.19-beta", Some("26.1")),
            ("26.1.2.114", Some("26.1.2")),
            ("26.3.0.58-beta", Some("26.3")),
            ("26.1.0.0-alpha.1+snapshot-1", Some("26.1")),
            ("0.25w14craftmine.3-beta", None),
            ("1.20.1-47.1.106", None),
        ];
        for (neoforge, minecraft) in cases {
            assert_eq!(
                neoforge_minecraft(neoforge).as_deref(),
                minecraft,
                "{neoforge}"
            );
        }
    }

    #[test]
    fn rejects_version_ids_unsafe_for_paths() {
        let cases = [
            ("1.21.1", true),
            ("26.3.0.58-beta", true),
            ("0.16.5+build.1", true),
            ("", false),
            ("..", false),
            ("1.21/../x", false),
            ("1.21?x", false),
            ("-rf", false),
        ];
        for (id, safe) in cases {
            assert_eq!(is_safe_version_id(id), safe, "{id}");
        }
    }
}
