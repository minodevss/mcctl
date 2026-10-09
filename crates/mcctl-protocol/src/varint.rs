use crate::{Invalid, Parse};

pub(crate) const MAX_VARINT_LEN: usize = 5;

pub fn read_varint(buf: &[u8]) -> Parse<i32> {
    let mut value: u32 = 0;
    for (index, &byte) in buf.iter().take(MAX_VARINT_LEN).enumerate() {
        value |= u32::from(byte & 0x7F) << (7 * index);
        if byte & 0x80 == 0 {
            return Parse::Done {
                value: value.cast_signed(),
                consumed: index + 1,
            };
        }
    }
    if buf.len() >= MAX_VARINT_LEN {
        Parse::Invalid(Invalid::BadVarInt)
    } else {
        Parse::Incomplete
    }
}

pub fn write_varint(value: i32, out: &mut Vec<u8>) {
    let mut rest = value.cast_unsigned();
    loop {
        let low = rest.to_le_bytes()[0] & 0x7F;
        rest >>= 7;
        if rest == 0 {
            out.push(low);
            return;
        }
        out.push(low | 0x80);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOUNDARIES: &[(i32, &[u8])] = &[
        (0, &[0x00]),
        (1, &[0x01]),
        (127, &[0x7F]),
        (128, &[0x80, 0x01]),
        (255, &[0xFF, 0x01]),
        (25_565, &[0xDD, 0xC7, 0x01]),
        (2_097_151, &[0xFF, 0xFF, 0x7F]),
        (i32::MAX, &[0xFF, 0xFF, 0xFF, 0xFF, 0x07]),
        (-1, &[0xFF, 0xFF, 0xFF, 0xFF, 0x0F]),
        (i32::MIN, &[0x80, 0x80, 0x80, 0x80, 0x08]),
    ];

    #[test]
    fn varint_round_trips_boundaries() {
        for &(value, bytes) in BOUNDARIES {
            let mut written = Vec::new();
            write_varint(value, &mut written);
            assert_eq!(written, bytes, "write {value}");
            assert_eq!(
                read_varint(bytes),
                Parse::Done {
                    value,
                    consumed: bytes.len()
                },
                "read {value}"
            );
        }
    }

    #[test]
    fn varint_stops_at_last_byte() {
        assert_eq!(
            read_varint(&[0xAC, 0x02, 0xFF]),
            Parse::Done {
                value: 300,
                consumed: 2
            }
        );
    }

    #[test]
    fn varint_incomplete_until_last_byte() {
        let cases: &[&[u8]] = &[&[], &[0x80], &[0xFF, 0xFF, 0xFF, 0xFF]];
        for &bytes in cases {
            assert_eq!(read_varint(bytes), Parse::Incomplete, "{bytes:02x?}");
        }
    }

    #[test]
    fn rejects_six_byte_varint() {
        let cases: &[&[u8]] = &[
            &[0x80, 0x80, 0x80, 0x80, 0x80],
            &[0x80, 0x80, 0x80, 0x80, 0x80, 0x01],
            &[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF],
        ];
        for &bytes in cases {
            assert_eq!(
                read_varint(bytes),
                Parse::Invalid(Invalid::BadVarInt),
                "{bytes:02x?}"
            );
        }
    }
}
