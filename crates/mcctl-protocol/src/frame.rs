use crate::varint::MAX_VARINT_LEN;
use crate::{Invalid, Parse, read_varint, write_varint};

pub const MAX_FRAME_LEN: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub id: i32,
    pub body: Vec<u8>,
}

/// Reads one length-prefixed packet; a declared length over `max_len` is invalid before its body arrives.
pub fn parse_frame(buf: &[u8], max_len: usize) -> Parse<Frame> {
    split_frame(buf, max_len).and_then(|packet| {
        let mut fields = Fields::new(packet);
        let id = fields.varint()?;
        Ok(Frame {
            id,
            body: fields.rest().to_vec(),
        })
    })
}

pub(crate) fn split_frame(buf: &[u8], max_len: usize) -> Parse<&[u8]> {
    let (len, header) = match read_varint(buf) {
        Parse::Done { value, consumed } => (value, consumed),
        Parse::Incomplete => return Parse::Incomplete,
        Parse::Invalid(invalid) => return Parse::Invalid(invalid),
    };
    let Some(len) = usize::try_from(len).ok().filter(|&len| len > 0) else {
        return Parse::Invalid(Invalid::BadLength);
    };
    if len > max_len {
        return Parse::Invalid(Invalid::FrameTooLong);
    }
    match buf.get(header..).and_then(|rest| rest.get(..len)) {
        Some(packet) => Parse::Done {
            value: packet,
            consumed: header + len,
        },
        None => Parse::Incomplete,
    }
}

pub(crate) struct Fields<'a> {
    rest: &'a [u8],
}

impl<'a> Fields<'a> {
    pub(crate) fn new(packet: &'a [u8]) -> Self {
        Self { rest: packet }
    }

    pub(crate) fn varint(&mut self) -> Result<i32, Invalid> {
        match read_varint(self.rest) {
            Parse::Done { value, consumed } => {
                self.rest = self.rest.get(consumed..).unwrap_or_default();
                Ok(value)
            }
            Parse::Incomplete => Err(Invalid::BadLength),
            Parse::Invalid(invalid) => Err(invalid),
        }
    }

    pub(crate) fn string(&mut self, max_bytes: usize) -> Result<&'a str, Invalid> {
        let len = usize::try_from(self.varint()?).map_err(|_| Invalid::BadString)?;
        if len > max_bytes {
            return Err(Invalid::BadString);
        }
        let (bytes, rest) = self.rest.split_at_checked(len).ok_or(Invalid::BadLength)?;
        self.rest = rest;
        std::str::from_utf8(bytes).map_err(|_| Invalid::BadString)
    }

    pub(crate) fn array<const N: usize>(&mut self) -> Result<[u8; N], Invalid> {
        let (bytes, rest) = self
            .rest
            .split_first_chunk::<N>()
            .ok_or(Invalid::BadLength)?;
        self.rest = rest;
        Ok(*bytes)
    }

    pub(crate) fn rest(self) -> &'a [u8] {
        self.rest
    }

    pub(crate) fn finish(self) -> Result<(), Invalid> {
        if self.rest.is_empty() {
            Ok(())
        } else {
            Err(Invalid::BadLength)
        }
    }
}

pub(crate) fn encode_packet(id: i32, body: &[u8]) -> Vec<u8> {
    let mut packet = Vec::with_capacity(MAX_VARINT_LEN + body.len());
    write_varint(id, &mut packet);
    packet.extend_from_slice(body);
    let mut frame = Vec::with_capacity(MAX_VARINT_LEN + packet.len());
    write_len(packet.len(), &mut frame);
    frame.extend_from_slice(&packet);
    frame
}

pub(crate) fn write_string(value: &str, out: &mut Vec<u8>) {
    write_len(value.len(), out);
    out.extend_from_slice(value.as_bytes());
}

fn write_len(len: usize, out: &mut Vec<u8>) {
    write_varint(i32::try_from(len).unwrap_or(i32::MAX), out);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::framed;

    #[test]
    fn parses_frame_and_leaves_following_bytes() {
        let mut buf = framed(&[0x01, 1, 2, 3, 4, 5, 6, 7, 8]);
        buf.extend_from_slice(&[0x01, 0x00]);
        assert_eq!(
            parse_frame(&buf, MAX_FRAME_LEN),
            Parse::Done {
                value: Frame {
                    id: 0x01,
                    body: vec![1, 2, 3, 4, 5, 6, 7, 8]
                },
                consumed: 10
            }
        );
    }

    #[test]
    fn incomplete_until_frame_body_arrives() {
        let buf = framed(&[0x00, 0xAA, 0xBB]);
        for end in 0..buf.len() {
            assert_eq!(
                parse_frame(&buf[..end], MAX_FRAME_LEN),
                Parse::Incomplete,
                "prefix {end}"
            );
        }
    }

    #[test]
    fn rejects_frame_over_given_limit() {
        let buf = framed(&[0x01, 1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(
            parse_frame(&buf[..1], 8),
            Parse::Invalid(Invalid::FrameTooLong)
        );
    }

    #[test]
    fn rejects_frames_without_packet_id() {
        let cases: &[&[u8]] = &[&[0x00], &[0xFF, 0xFF, 0xFF, 0xFF, 0x0F]];
        for &buf in cases {
            assert_eq!(
                parse_frame(buf, MAX_FRAME_LEN),
                Parse::Invalid(Invalid::BadLength),
                "{buf:02x?}"
            );
        }
    }

    #[test]
    fn encodes_length_before_packet_id() {
        assert_eq!(encode_packet(0x01, &[0xAB]), [0x02, 0x01, 0xAB]);
    }
}
