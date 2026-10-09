use crate::{Parse, parse_frame, read_varint, write_varint};

pub(crate) fn framed(packet: &[u8]) -> Vec<u8> {
    let mut frame = Vec::new();
    write_varint(i32::try_from(packet.len()).unwrap(), &mut frame);
    frame.extend_from_slice(packet);
    frame
}

pub(crate) fn handshake_packet(protocol: i32, host: &[u8], port: u16, intent: i32) -> Vec<u8> {
    let mut packet = vec![0x00];
    write_varint(protocol, &mut packet);
    write_varint(i32::try_from(host.len()).unwrap(), &mut packet);
    packet.extend_from_slice(host);
    packet.extend_from_slice(&port.to_be_bytes());
    write_varint(intent, &mut packet);
    packet
}

pub(crate) fn decode_string_packet(bytes: &[u8]) -> (i32, String) {
    let Parse::Done {
        value: frame,
        consumed,
    } = parse_frame(bytes, usize::MAX)
    else {
        panic!("not a complete frame: {bytes:02x?}");
    };
    assert_eq!(consumed, bytes.len());
    let Parse::Done {
        value: len,
        consumed,
    } = read_varint(&frame.body)
    else {
        panic!("no string length: {:02x?}", frame.body);
    };
    let text = &frame.body[consumed..];
    assert_eq!(usize::try_from(len).unwrap(), text.len());
    (frame.id, String::from_utf8(text.to_vec()).unwrap())
}
