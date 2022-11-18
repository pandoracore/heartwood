use std::collections::VecDeque;
use std::convert::Infallible;
use std::io;
use std::io::Read;

use crate::prelude::Address;
use nakamoto_net::Link;

// TODO: Implement Try trait once stabilized
/// Result of a handskae stage
pub enum HandshakeResult<H: Handshake, T: Transcode> {
    /// Handshake is not completed; we process to the next handshake stage.
    Next(H, Vec<u8>),
    /// Handshake is completed; we now can communicate in encrypted way.
    Complete(T, Vec<u8>, Link),
    /// Handshake has failed with some error.
    Error(H::Error),
}

/// State machine implementation of a handshake protocol which can be run by
/// peers.
pub trait Handshake: Sized {
    type InitState;

    /// The resulting transcoder which will be constructed upon a successful
    /// handshake
    type Transcoder: Transcode;

    /// Errors which may happen during the handshake.
    type Error: std::error::Error;

    /// Constructs a new handshake state machine for outbound connection
    fn init(state: Self::InitState) -> Self;

    /// Constructs a new handshake state machine for inbound connection
    fn accept() -> Self;

    /// Post a new byte stream received by local peer and progress handshake
    /// protocol.
    fn next_stage(self, input: &[u8]) -> HandshakeResult<Self, Self::Transcoder>;

    /// Returns direction of the handshake protocol
    fn link(&self) -> Link;
}

/// Dumb handshake structure which runs void protocol.
#[derive(Debug)]
pub struct NoHandshake(Link);

impl Handshake for NoHandshake {
    type InitState = Address;
    type Transcoder = PlainTranscoder;
    type Error = Infallible;

    fn init(_: Address) -> Self {
        NoHandshake(Link::Outbound)
    }

    fn accept() -> Self {
        NoHandshake(Link::Inbound)
    }

    fn next_stage(self, _input: &[u8]) -> HandshakeResult<Self, Self::Transcoder> {
        HandshakeResult::Complete(PlainTranscoder, vec![], self.0)
    }

    fn link(&self) -> Link {
        self.0
    }
}

/// Trait allowing transcoding the stream using some form of stream encryption
/// and/or encoding.
pub trait Transcode {
    /// Decodes data received from the remote peer and update the internal state
    /// of the transcoder, if necessary.
    fn decode(&mut self, data: &[u8]) -> Vec<u8>;

    /// Encodes data before sending them to the remote peer.
    fn encode(&mut self, data: Vec<u8>) -> Vec<u8>;
}

/// Transcoder which does nothing.
#[derive(Debug, Default)]
pub struct PlainTranscoder;

impl Transcode for PlainTranscoder {
    fn decode(&mut self, data: &[u8]) -> Vec<u8> {
        data.to_vec()
    }

    fn encode(&mut self, data: Vec<u8>) -> Vec<u8> {
        data
    }
}

pub type Frame = Vec<u8>;

#[derive(Copy, Clone, Debug)]
pub struct OversizedData(usize);

#[derive(Debug, Default)]
pub struct Framer<T: Transcode> {
    input: VecDeque<u8>,
    inner: T,
}

impl<T: Transcode> Framer<T> {
    pub fn new(inner: T) -> Self {
        Framer {
            input: Default::default(),
            inner,
        }
    }

    pub fn input(&mut self, encoded: &[u8]) {
        self.input.extend(self.inner.decode(encoded));
    }

    pub fn frame(&mut self, decoded: Vec<u8>) -> Result<Frame, OversizedData> {
        let mut data = self.inner.encode(decoded);
        let len = data.len();
        let len = u8::try_from(len).map_err(|_| OversizedData(len))?;
        let len = len.to_be_bytes();
        let mut buf = Vec::with_capacity(data.len() + 2);
        buf.extend(len);
        buf.append(&mut data);
        Ok(buf)
    }
}

impl<T: Transcode> Iterator for Framer<T> {
    type Item = Frame;

    fn next(&mut self) -> Option<Self::Item> {
        if self.input.len() < 2 {
            return None;
        }
        let mut len = [0u8; 2];
        self.input
            .read_exact(&mut len)
            .expect("the length is checked");
        let len = u16::from_be_bytes(len) as usize;
        if self.input.len() < 2 + len {
            return None;
        }
        self.input.pop_front();
        self.input.pop_front();
        let reminder = self.input.split_off(len);
        let mut data = vec![0u8; len];
        self.input.read_exact(&mut data).expect("checked length");
        self.input = reminder;
        Some(data)
    }
}

#[derive(Copy, Clone, Debug)]
pub struct ChannelError;

#[derive(Clone, Ord, PartialOrd, Eq, PartialEq, Hash, Debug)]
pub struct MuxMsg {
    pub channel: u16,
    pub data: Vec<u8>,
}

impl From<MuxMsg> for Frame {
    fn from(mut msg: MuxMsg) -> Self {
        let channel = msg.channel.to_be_bytes();
        let mut data = Vec::with_capacity(msg.data.len() + 2);
        data.extend(channel);
        data.append(&mut msg.data);
        data
    }
}

impl TryFrom<Frame> for MuxMsg {
    type Error = ChannelError;

    fn try_from(frame: Frame) -> Result<Self, Self::Error> {
        if frame.len() < 2 {
            return Err(ChannelError);
        }
        let mut channel = [0u8; 2];
        let mut cursor = io::Cursor::new(frame);
        cursor
            .read_exact(&mut channel)
            .expect("the length is checked");
        let channel = u16::from_be_bytes(channel);
        Ok(MuxMsg {
            channel,
            data: cursor.into_inner(),
        })
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::deserializer::Deserializer;

    #[test]
    fn decode() {
        let mut pipeline = Framer::new(PlainTranscoder);
        let mut deser = Deserializer::<String>::new(512);

        let data = [
            0x00, 0x04, 0x00, 0x00, b'a', b'b', 0x00, 0x07, 0x00, 0x01, b'M', b'a', b'x', b'i',
            b'm',
        ];
        let mut expected_payloads = [(0u16, b"ab".to_vec()), (1, b"Maxim".to_vec())].into_iter();
        let mut expected_msgs = ["ab", "Maxim"].into_iter();

        for byte in data {
            // Writing data byte by byte, ensuring that the reading is not broken
            pipeline.input(&[byte]);
            for frame in &mut pipeline {
                let msg = MuxMsg::try_from(frame).unwrap();
                let (channel, data) = expected_payloads.next().unwrap();
                deser.input(&data);
                assert_eq!(msg, MuxMsg { channel, data });
            }
        }

        for msg in deser {
            let msg = msg.unwrap();
            assert_eq!(msg, expected_msgs.next().unwrap());
        }
    }
}
