use crate::frame::{encode_packet, write_string};
use crate::json;

const DISCONNECT_ID: i32 = 0x00;

/// Login-state Disconnect; through 1.21.x and 26.x its reason is still a JSON text component.
pub fn login_disconnect(message: &str) -> Vec<u8> {
    let mut reason = String::from(r#"{"text":"#);
    json::push_string(&mut reason, message);
    reason.push('}');
    let mut body = Vec::with_capacity(reason.len() + 2);
    write_string(&reason, &mut body);
    encode_packet(DISCONNECT_ID, &body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::decode_string_packet;

    #[test]
    fn wraps_message_in_text_component() {
        let cases = [
            (
                "survival is offline. Try again later.",
                r#"{"text":"survival is offline. Try again later."}"#,
            ),
            ("say \"later\"", r#"{"text":"say \"later\""}"#),
        ];
        for (message, expected) in cases {
            assert_eq!(
                decode_string_packet(&login_disconnect(message)),
                (0x00, expected.to_owned())
            );
        }
    }
}
