use std::fmt;

/// Result of reading one value from the start of a buffer that may hold only part of a stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parse<T> {
    Incomplete,
    Done { value: T, consumed: usize },
    Invalid(Invalid),
}

impl<T> Parse<T> {
    pub(crate) fn and_then<U>(self, read: impl FnOnce(T) -> Result<U, Invalid>) -> Parse<U> {
        match self {
            Self::Incomplete => Parse::Incomplete,
            Self::Done { value, consumed } => match read(value) {
                Ok(value) => Parse::Done { value, consumed },
                Err(invalid) => Parse::Invalid(invalid),
            },
            Self::Invalid(invalid) => Parse::Invalid(invalid),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Invalid {
    FrameTooLong,
    BadLength,
    BadVarInt,
    UnexpectedPacket(i32),
    BadString,
    BadIntent(i32),
    LegacyPing,
}

impl fmt::Display for Invalid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FrameTooLong => f.write_str("frame is longer than the limit"),
            Self::BadLength => f.write_str("frame length does not match its fields"),
            Self::BadVarInt => f.write_str("varint is longer than 5 bytes"),
            Self::UnexpectedPacket(id) => write!(f, "unexpected packet id {id:#04x}"),
            Self::BadString => f.write_str("string is too long or not utf-8"),
            Self::BadIntent(intent) => write!(f, "unknown handshake intent {intent}"),
            Self::LegacyPing => f.write_str("legacy server list ping"),
        }
    }
}

impl std::error::Error for Invalid {}
