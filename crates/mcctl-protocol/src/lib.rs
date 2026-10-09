//! Minecraft Java wire codec for handshakes, status replies and login disconnects.
//! Pure: no I/O, no async, std only.

mod frame;
mod handshake;
mod host;
mod json;
mod login;
mod parse;
mod status;
#[cfg(test)]
mod test_support;
mod varint;

pub use frame::{Frame, MAX_FRAME_LEN, parse_frame};
pub use handshake::{Handshake, Intent, parse_handshake};
pub use host::normalize_host;
pub use login::login_disconnect;
pub use parse::{Invalid, Parse};
pub use status::{StatusPacket, parse_status_packet, pong, status_response};
pub use varint::{read_varint, write_varint};
