use std::convert::Infallible;

// TODO: Implement Try trait once stabilized
/// Result of a handskae stage
pub enum HandshakeResult<H: Handshake, T: Transcode> {
    /// Handshake is not completed; we process to the next handshake stage.
    Next(H, Vec<u8>),
    /// Handshake is completed; we now can communicate in encrypted way.
    Complete(T, Vec<u8>),
    /// Handshake has failed with some error.
    Error(H::Error),
}

/// State machine implementation of a handshake protocol which can be run by
/// peers.
pub trait Handshake: Sized {
    /// The resulting transcoder which will be constructed upon a successful
    /// handshake
    type Transcoder: Transcode;

    /// Errors which may happen during the handshake.
    type Error: std::error::Error;

    /// Constructs a new handshake state machine.
    fn new() -> Self;

    /// Post a new byte stream received by local peer and progress handshake
    /// protocol.
    fn next_stage(self, input: &[u8]) -> HandshakeResult<Self, Self::Transcoder>;
}

/// Dumb handshake structure which runs void protocol.
#[derive(Debug, Default)]
pub struct NoHandshake;

impl Handshake for NoHandshake {
    type Transcoder = PlainTranscoder;
    type Error = Infallible;

    fn new() -> Self {
        NoHandshake
    }

    fn next_stage(self, _input: &[u8]) -> HandshakeResult<Self, Self::Transcoder> {
        HandshakeResult::Complete(PlainTranscoder, vec![])
    }
}

/// Trait allowing transcoding the stream using some form of stream encryption
/// and/or encoding.
pub trait Transcode {
    /// Decodes data received from the remote peer and update the internal state
    /// of the transcoder, if necessary.
    fn decrypt(&mut self, data: &[u8]) -> Vec<u8>;

    /// Encodes data before sending them to the remote peer.
    fn encrypt(&mut self, data: Vec<u8>) -> Vec<u8>;
}

/// Transcoder which does nothing.
#[derive(Debug, Default)]
pub struct PlainTranscoder;

impl Transcode for PlainTranscoder {
    fn decrypt(&mut self, data: &[u8]) -> Vec<u8> {
        data.to_vec()
    }

    fn encrypt(&mut self, data: Vec<u8>) -> Vec<u8> {
        data
    }
}
