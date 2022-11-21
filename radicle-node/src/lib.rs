#[macro_use]
extern crate amplify;

pub mod address;
pub mod client;
pub mod clock;
pub mod control;
pub mod deserializer;
pub mod logger;
pub mod service;
pub mod sql;
#[cfg(any(test, feature = "test"))]
pub mod test;
#[cfg(test)]
pub mod tests;
pub mod wire;

use cyphernet::addr::UniversalAddr;
use cyphernet::crypto::ed25519::Curve25519;

pub use nakamoto_net::{Io, Link, LocalDuration, LocalTime};
pub use radicle::{collections, crypto, git, hash, identity, node, profile, rad, storage};

pub type PeerAddr = cyphernet::addr::PeerAddr<Curve25519, UniversalAddr>;

pub mod prelude {
    pub use crate::clock::Timestamp;
    pub use crate::crypto::{PublicKey, Signature, Signer};
    pub use crate::deserializer::Deserializer;
    pub use crate::hash::Digest;
    pub use crate::identity::{Did, Id};
    pub use crate::service::filter::Filter;
    pub use crate::service::message::ConnectAddr;
    pub use crate::service::{DisconnectReason, Event, Message, Network, NodeId};
    pub use crate::storage::refs::Refs;
    pub use crate::storage::WriteStorage;
    pub use crate::PeerAddr;
    pub use crate::{LocalDuration, LocalTime};
}
