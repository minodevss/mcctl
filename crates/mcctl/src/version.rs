use std::cmp::Ordering;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Parsed<'a> {
    numbers: Vec<u32>,
    pre_release: Option<&'a str>,
}

/// Reads "1.21.1", "26.3", "1.21-pre1" or "0.2.0"; snapshots like "24w14a" give `None`.
fn parse(version: &str) -> Option<Parsed<'_>> {
    let end = version
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(version.len());
    let (numeric, rest) = version.split_at(end);
    let is_build_metadata = rest.len() > 1 && rest.starts_with('+');
    let pre_release = if rest.is_empty() || is_build_metadata {
        None
    } else {
        Some(rest.strip_prefix('-').filter(|tag| !tag.is_empty())?)
    };
    let numbers = numeric
        .split('.')
        .map(|part| part.parse().ok())
        .collect::<Option<Vec<u32>>>()?;
    Some(Parsed {
        numbers,
        pre_release,
    })
}

/// Orders release versions; 1.x and year-based 26.x compare by their numbers, and a
/// pre-release sorts before its release. `None` when either side is not a release-like version.
pub(crate) fn compare(left: &str, right: &str) -> Option<Ordering> {
    let left = parse(left)?;
    let right = parse(right)?;
    let width = left.numbers.len().max(right.numbers.len());
    let padded = |numbers: &[u32]| {
        let mut numbers = numbers.to_vec();
        numbers.resize(width, 0);
        numbers
    };
    let by_numbers = padded(&left.numbers).cmp(&padded(&right.numbers));
    Some(
        by_numbers.then_with(|| match (left.pre_release, right.pre_release) {
            (None, None) => Ordering::Equal,
            (None, Some(_)) => Ordering::Greater,
            (Some(_), None) => Ordering::Less,
            (Some(left), Some(right)) => pre_release_key(left).cmp(&pre_release_key(right)),
        }),
    )
}

/// Mojang's order before a release: snapshots, then pre-releases, then release candidates, each by number.
fn pre_release_key(tag: &str) -> (u8, u32, &str) {
    let kind_end = tag
        .find(|c: char| !c.is_ascii_alphabetic())
        .unwrap_or(tag.len());
    let (kind, rest) = tag.split_at(kind_end);
    let rank = match kind {
        "pre" => 1,
        "rc" => 2,
        _ => 0,
    };
    let number = rest.trim_start_matches('-').parse().unwrap_or(0);
    (rank, number, tag)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_year_and_legacy_versions() {
        let cases = [
            ("1.21.1", "1.21", Some(Ordering::Greater)),
            ("1.21", "1.21.0", Some(Ordering::Equal)),
            ("1.20.6", "1.21", Some(Ordering::Less)),
            ("1.9", "1.10", Some(Ordering::Less)),
            ("26.3", "1.21.10", Some(Ordering::Greater)),
            ("26.1.2", "26.1", Some(Ordering::Greater)),
            ("26.3", "26.10", Some(Ordering::Less)),
            ("1.21-pre1", "1.21", Some(Ordering::Less)),
            ("1.21-rc1", "1.21-pre1", Some(Ordering::Greater)),
            ("26.3-snapshot-1", "26.2", Some(Ordering::Greater)),
            ("26.1-snapshot-1", "26.1-pre-1", Some(Ordering::Less)),
            ("26.1-rc-1", "26.1-pre-2", Some(Ordering::Greater)),
            ("1.21-pre10", "1.21-pre2", Some(Ordering::Greater)),
            ("0.2.0", "0.1.9", Some(Ordering::Greater)),
            ("24w14a", "1.21", None),
            ("1.21", "", None),
            ("1..21", "1.21", None),
        ];
        for (left, right, expected) in cases {
            assert_eq!(compare(left, right), expected, "{left} vs {right}");
        }
    }
}
