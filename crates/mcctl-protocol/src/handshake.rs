use crate::frame::{Fields, split_frame};
use crate::{Invalid, MAX_FRAME_LEN, Parse};

const HANDSHAKE_ID: i32 = 0x00;
// Pre-1.7 clients open with 0xFE instead of a VarInt frame length.
const LEGACY_PING: u8 = 0xFE;
// The host is capped at 255 UTF-16 units; 4 UTF-8 bytes per unit is a safe upper bound.
const MAX_HOST_BYTES: usize = 255 * 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handshake {
    pub protocol: i32,
    pub host: String,
    pub port: u16,
    pub intent: Intent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intent {
    Status,
    Login,
    Transfer,
}

/// Reads the first frame a client sends; `consumed` covers only that frame, not a Login Start behind it.
pub fn parse_handshake(buf: &[u8]) -> Parse<Handshake> {
    if buf.first() == Some(&LEGACY_PING) {
        return Parse::Invalid(Invalid::LegacyPing);
    }
    split_frame(buf, MAX_FRAME_LEN).and_then(read_handshake)
}

fn read_handshake(packet: &[u8]) -> Result<Handshake, Invalid> {
    let mut fields = Fields::new(packet);
    let id = fields.varint()?;
    if id != HANDSHAKE_ID {
        return Err(Invalid::UnexpectedPacket(id));
    }
    let protocol = fields.varint()?;
    let host = fields.string(MAX_HOST_BYTES)?.to_owned();
    let port = u16::from_be_bytes(fields.array()?);
    let intent = intent(fields.varint()?)?;
    fields.finish()?;
    Ok(Handshake {
        protocol,
        host,
        port,
        intent,
    })
}

fn intent(id: i32) -> Result<Intent, Invalid> {
    match id {
        1 => Ok(Intent::Status),
        2 => Ok(Intent::Login),
        3 => Ok(Intent::Transfer),
        other => Err(Invalid::BadIntent(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{framed, handshake_packet};

    fn handshake_frame(host: &str, intent: i32) -> Vec<u8> {
        framed(&handshake_packet(772, host.as_bytes(), 25565, intent))
    }

    #[test]
    fn parses_login_handshake() {
        let buf = handshake_frame("play.example.com", 2);
        assert_eq!(
            parse_handshake(&buf),
            Parse::Done {
                value: Handshake {
                    protocol: 772,
                    host: "play.example.com".to_owned(),
                    port: 25565,
                    intent: Intent::Login,
                },
                consumed: buf.len()
            }
        );
    }

    #[test]
    fn parses_every_intent() {
        let cases = [
            (1, Intent::Status),
            (2, Intent::Login),
            (3, Intent::Transfer),
        ];
        for (id, expected) in cases {
            let Parse::Done { value, .. } = parse_handshake(&handshake_frame("a.test", id)) else {
                panic!("intent {id} did not parse");
            };
            assert_eq!(value.intent, expected, "intent {id}");
        }
    }

    #[test]
    fn keeps_raw_host() {
        let raw = "Play.Example.com.\0FML3\0";
        let Parse::Done { value, .. } = parse_handshake(&handshake_frame(raw, 2)) else {
            panic!("handshake did not parse");
        };
        assert_eq!(value.host, raw);
    }

    #[test]
    fn incomplete_until_full_frame() {
        let buf = handshake_frame("play.example.com", 2);
        for end in 0..buf.len() {
            assert_eq!(
                parse_handshake(&buf[..end]),
                Parse::Incomplete,
                "prefix {end}"
            );
        }
    }

    #[test]
    fn reports_consumed_leaving_login_start() {
        let handshake = handshake_frame("play.example.com", 2);
        let login_start = framed(&[0x00, 0x05, b'S', b't', b'e', b'v', b'e']);
        let buf = [handshake.as_slice(), login_start.as_slice()].concat();
        let Parse::Done { consumed, .. } = parse_handshake(&buf) else {
            panic!("handshake did not parse");
        };
        assert_eq!(&buf[consumed..], login_start.as_slice());
    }

    #[test]
    fn rejects_legacy_ping() {
        let cases: &[&[u8]] = &[&[0xFE], &[0xFE, 0x01], &[0xFE, 0x01, 0xFA, 0x00]];
        for &buf in cases {
            assert_eq!(
                parse_handshake(buf),
                Parse::Invalid(Invalid::LegacyPing),
                "{buf:02x?}"
            );
        }
    }

    #[test]
    fn rejects_frame_over_limit() {
        let mut header = Vec::new();
        crate::write_varint(1025, &mut header);
        assert_eq!(
            parse_handshake(&header),
            Parse::Invalid(Invalid::FrameTooLong)
        );
    }

    #[test]
    fn accepts_frame_at_limit() {
        let host = "a".repeat(1016);
        let buf = handshake_frame(&host, 2);
        assert_eq!(buf.len(), 2 + MAX_FRAME_LEN);
        assert!(matches!(parse_handshake(&buf), Parse::Done { .. }));
    }

    #[test]
    fn rejects_malformed_handshakes() {
        let full = handshake_packet(772, b"a.test", 25565, 2);
        let mut trailing = full.clone();
        trailing.push(0x00);
        let mut long_host = vec![0x00, 0x84, 0x06];
        crate::write_varint(1021, &mut long_host);
        let cases: &[(&str, Vec<u8>, Invalid)] = &[
            (
                "status request id",
                framed(&[0x01, 0x00]),
                Invalid::UnexpectedPacket(0x01),
            ),
            (
                "intent 0",
                framed(&handshake_packet(772, b"a.test", 25565, 0)),
                Invalid::BadIntent(0),
            ),
            (
                "intent 4",
                framed(&handshake_packet(772, b"a.test", 25565, 4)),
                Invalid::BadIntent(4),
            ),
            ("empty frame", vec![0x00], Invalid::BadLength),
            (
                "negative length",
                vec![0xFF, 0xFF, 0xFF, 0xFF, 0x0F],
                Invalid::BadLength,
            ),
            (
                "missing port and intent",
                framed(&full[..full.len() - 3]),
                Invalid::BadLength,
            ),
            ("byte after intent", framed(&trailing), Invalid::BadLength),
            (
                "host not utf-8",
                framed(&handshake_packet(772, &[0xC3, 0x28], 25565, 2)),
                Invalid::BadString,
            ),
            ("host over limit", framed(&long_host), Invalid::BadString),
            (
                "six byte protocol varint",
                framed(&[0x00, 0x80, 0x80, 0x80, 0x80, 0x80, 0x01]),
                Invalid::BadVarInt,
            ),
        ];
        for (name, buf, expected) in cases {
            assert_eq!(parse_handshake(buf), Parse::Invalid(*expected), "{name}");
        }
    }
}
