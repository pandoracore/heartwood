use std::net;

use crate::collections::HashSet;
use crate::identity::{Id, PublicKey};
use crate::service::filter::Filter;
use crate::service::message::ConnectAddr;
use crate::PeerAddr;

/// Peer-to-peer network.
#[derive(Default, Debug, Copy, Clone, PartialEq, Eq)]
pub enum Network {
    #[default]
    Main,
    Test,
}

/// Project tracking policy.
#[derive(Debug, Clone)]
pub enum ProjectTracking {
    /// Track all projects we come across.
    All { blocked: HashSet<Id> },
    /// Track a static list of projects.
    Allowed(HashSet<Id>),
}

impl Default for ProjectTracking {
    fn default() -> Self {
        Self::All {
            blocked: HashSet::default(),
        }
    }
}

/// Project remote tracking policy.
#[derive(Debug, Default, Clone)]
pub enum RemoteTracking {
    /// Only track remotes of project delegates.
    #[default]
    DelegatesOnly,
    /// Track all remotes.
    All { blocked: HashSet<PublicKey> },
    /// Track a specific list of users as well as the project delegates.
    Allowed(HashSet<PublicKey>),
}

/// Service configuration.
#[derive(Debug, Clone)]
pub struct Config {
    /// Peers to connect to on startup.
    /// Connections to these peers will be maintained.
    pub connect: Vec<PeerAddr>,
    /// Specify the node's public addresses
    pub external_addresses: Vec<PeerAddr>,
    /// SOCKS5 proxy to use for connecting external peers
    pub socks5_proxy: Option<net::SocketAddr>,
    /// Peer-to-peer network.
    pub network: Network,
    /// Project tracking policy.
    pub project_tracking: ProjectTracking,
    /// Project remote tracking policy.
    pub remote_tracking: RemoteTracking,
    /// Whether or not our node should relay inventories.
    pub relay: bool,
    /// List of addresses to listen on for protocol connections.
    pub listen: Vec<ConnectAddr>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            connect: Vec::default(),
            external_addresses: vec![],
            socks5_proxy: None,
            network: Network::default(),
            project_tracking: ProjectTracking::default(),
            remote_tracking: RemoteTracking::default(),
            relay: true,
            listen: vec![],
        }
    }
}

impl Config {
    pub fn is_persistent(&self, addr: &PeerAddr) -> bool {
        self.connect.contains(addr)
    }

    pub fn is_tracking(&self, id: &Id) -> bool {
        match &self.project_tracking {
            ProjectTracking::All { blocked } => !blocked.contains(id),
            ProjectTracking::Allowed(ids) => ids.contains(id),
        }
    }

    /// Track a project. Returns whether the policy was updated.
    pub fn track(&mut self, id: Id) -> bool {
        match &mut self.project_tracking {
            ProjectTracking::All { .. } => false,
            ProjectTracking::Allowed(ids) => ids.insert(id),
        }
    }

    /// Untrack a project. Returns whether the policy was updated.
    pub fn untrack(&mut self, id: Id) -> bool {
        match &mut self.project_tracking {
            ProjectTracking::All { blocked } => blocked.insert(id),
            ProjectTracking::Allowed(ids) => ids.remove(&id),
        }
    }

    pub fn filter(&self) -> Filter {
        match &self.project_tracking {
            ProjectTracking::All { .. } => Filter::default(),
            ProjectTracking::Allowed(ids) => Filter::new(ids.iter()),
        }
    }

    pub fn alias(&self) -> [u8; 32] {
        let mut alias = [0u8; 32];

        alias[..9].copy_from_slice("anonymous".as_bytes());
        alias
    }
}
