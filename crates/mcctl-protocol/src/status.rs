use crate::frame::{Fields, encode_packet, split_frame, write_string};
use crate::{Invalid, MAX_FRAME_LEN, Parse, json};

const STATUS_REQUEST_ID: i32 = 0x00;
const PING_REQUEST_ID: i32 = 0x01;
const STATUS_RESPONSE_ID: i32 = 0x00;
const PONG_ID: i32 = 0x01;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusPacket {
    Request,
    Ping([u8; 8]),
}

/// Reads one serverbound packet of the status state.
pub fn parse_status_packet(buf: &[u8]) -> Parse<StatusPacket> {
    split_frame(buf, MAX_FRAME_LEN).and_then(|packet| {
        let mut fields = Fields::new(packet);
        let status = match fields.varint()? {
            STATUS_REQUEST_ID => StatusPacket::Request,
            PING_REQUEST_ID => StatusPacket::Ping(fields.array()?),
            other => return Err(Invalid::UnexpectedPacket(other)),
        };
        fields.finish()?;
        Ok(status)
    })
}

/// Status reply with no players that echoes `protocol` so the client lists the server as compatible.
pub fn status_response(protocol: i32, description: &str) -> Vec<u8> {
    let mut status = String::from(r#"{"version":{"name":"mcctl","protocol":"#);
    status.push_str(&protocol.to_string());
    status.push_str(r#"},"players":{"max":0,"online":0},"description":{"text":"#);
    json::push_string(&mut status, description);
    status.push_str("}}");
    let mut body = Vec::with_capacity(status.len() + 2);
    write_string(&status, &mut body);
    encode_packet(STATUS_RESPONSE_ID, &body)
}

pub fn pong(payload: [u8; 8]) -> Vec<u8> {
    encode_packet(PONG_ID, &payload)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{decode_string_packet, framed};
    use crate::{Frame, parse_frame};

    #[test]
    fn parses_status_packets() {
        let cases = [
            (framed(&[0x00]), StatusPacket::Request),
            (
                framed(&[0x01, 1, 2, 3, 4, 5, 6, 7, 8]),
                StatusPacket::Ping([1, 2, 3, 4, 5, 6, 7, 8]),
            ),
        ];
        for (buf, expected) in cases {
            assert_eq!(
                parse_status_packet(&buf),
                Parse::Done {
                    value: expected,
                    consumed: buf.len()
                }
            );
        }
    }

    #[test]
    fn rejects_malformed_status_packets() {
        let cases = [
            (framed(&[0x00, 0x00]), Invalid::BadLength),
            (framed(&[0x01, 1, 2, 3]), Invalid::BadLength),
            (
                framed(&[0x01, 1, 2, 3, 4, 5, 6, 7, 8, 9]),
                Invalid::BadLength,
            ),
            (framed(&[0x02]), Invalid::UnexpectedPacket(0x02)),
        ];
        for (buf, expected) in cases {
            assert_eq!(
                parse_status_packet(&buf),
                Parse::Invalid(expected),
                "{buf:02x?}"
            );
        }
    }

    #[test]
    fn status_response_reports_protocol_and_description() {
        let (id, status) = decode_string_packet(&status_response(772, "survival is offline"));
        assert_eq!(id, 0x00);
        assert_eq!(
            status,
            r#"{"version":{"name":"mcctl","protocol":772},"players":{"max":0,"online":0},"description":{"text":"survival is offline"}}"#
        );
    }

    #[test]
    fn escapes_quotes_in_description() {
        let (_, status) = decode_string_packet(&status_response(-1, "the \"lobby\"\\\n"));
        assert!(status.contains(r#""protocol":-1}"#), "{status}");
        assert!(
            status.ends_with(r#""description":{"text":"the \"lobby\"\\\u000a"}}"#),
            "{status}"
        );
    }

    #[test]
    fn pong_echoes_payload() {
        let payload = [0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x01, 0x02, 0x03];
        let bytes = pong(payload);
        assert_eq!(
            parse_frame(&bytes, MAX_FRAME_LEN),
            Parse::Done {
                value: Frame {
                    id: 0x01,
                    body: payload.to_vec()
                },
                consumed: bytes.len()
            }
        );
    }
}
