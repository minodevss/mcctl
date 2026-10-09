pub(crate) fn push_string(out: &mut String, value: &str) {
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\0'..='\u{1f}' => push_unicode_escape(out, c),
            _ => out.push(c),
        }
    }
    out.push('"');
}

fn push_unicode_escape(out: &mut String, c: char) {
    let code = u32::from(c);
    out.push_str("\\u00");
    out.push(hex_digit(code >> 4));
    out.push(hex_digit(code & 0xF));
}

fn hex_digit(nibble: u32) -> char {
    char::from_digit(nibble, 16).unwrap_or('0')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_json_strings() {
        let cases = [
            ("plain", r#""plain""#),
            (r#"say "hi""#, r#""say \"hi\"""#),
            (r"C:\worlds", r#""C:\\worlds""#),
            ("line\nbreak\ttab", r#""line\u000abreak\u0009tab""#),
            ("\0\u{1f}", r#""\u0000\u001f""#),
            ("é ☃ \u{7f}", "\"é ☃ \u{7f}\""),
        ];
        for (input, expected) in cases {
            let mut out = String::new();
            push_string(&mut out, input);
            assert_eq!(out, expected, "{input:?}");
        }
    }
}
