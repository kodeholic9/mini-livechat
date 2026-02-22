// author: kodeholic (powered by Gemini)

use std::fmt;

#[derive(Debug)]
pub enum LiveError {
    ChannelFull(String),
    PeerNotFound(u32),
    CryptoError(String),
    IoError(std::io::Error),
}

impl fmt::Display for LiveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LiveError::ChannelFull(id) => write!(f, "Capacity exceeded for channel: {}", id),
            LiveError::PeerNotFound(ssrc) => write!(f, "Peer not found for SSRC: {}", ssrc),
            LiveError::CryptoError(msg) => write!(f, "Crypto processing error: {}", msg),
            LiveError::IoError(err) => write!(f, "Network I/O error: {}", err),
        }
    }
}

impl std::error::Error for LiveError {}

pub type LiveResult<T> = Result<T, LiveError>;