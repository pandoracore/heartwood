//! Implementation of the transport protocol.
//!
//! We use the Noise XK handshake pattern to establish an encrypted stream with a remote peer.
//! The handshake itself is implemented in the external [`netservices`] crate.
use amplify::Wrapper;
use std::collections::VecDeque;
use std::fmt::Debug;
use std::net::SocketAddr;
use std::os::unix::io::AsRawFd;
use std::os::unix::prelude::RawFd;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use std::{io, net};

use crossbeam_channel as chan;
use cyphernet::{Cert, Digest, EcSign, Sha256};
use nakamoto_net::LocalTime;
use netservices::resource::{ListenerEvent, NetAccept, NetTransport, SessionEvent};
use netservices::session::ProtocolArtifact;
use netservices::{LinkDirection, NetConnection, NetSession};

use radicle::collections::HashMap;
use radicle::crypto::Signature;
use radicle::node::NodeId;
use radicle::storage::WriteStorage;

use crate::crypto::Signer;
use crate::service::reactor::{Fetch, Io};
use crate::service::{routing, session, DisconnectReason, Message, Service};
use crate::wire::{Decode, Encode, WireSession};
use crate::worker::{WorkerReq, WorkerResp};
use crate::{address, service};

/// Reactor action.
type Action<G> = reactor::Action<NetAccept<WireSession<G>>, NetTransport<WireSession<G>>>;

/// Peer connection state machine.
#[derive(Debug, Display)]
enum Peer<G: Signer + EcSign> {
    /// The initial state before handshake is completed.
    #[display("<connecting>")]
    Connecting { link: LinkDirection },

    /// The state after handshake is completed.
    /// Peers in this state are handled by the underlying service.
    #[display("{id}")]
    Connected { link: LinkDirection, id: NodeId },

    /// The state after a peer was disconnected, either during handshake,
    /// or once connected.
    #[display("<disconnected>")]
    Disconnected {
        id: Option<NodeId>,
        reason: DisconnectReason,
    },

    /// The state after we've started the process of upgraded the peer for a fetch.
    /// The request to handover the socket was made to the reactor.
    #[display("<upgrading>{id}")]
    Upgrading {
        fetch: Fetch,
        link: LinkDirection,
        id: NodeId,
    },

    /// The peer is now upgraded and we are in control of the socket.
    #[display("<upgraded>{id}")]
    Upgraded {
        link: LinkDirection,
        id: NodeId,
        response: chan::Receiver<WorkerResp<G>>,
    },
}

impl<G: Signer + EcSign> Peer<G> {
    /// Return a new connecting peer.
    fn connecting(link: LinkDirection) -> Self {
        Self::Connecting { link }
    }

    /// Switch to connected state.
    fn connected(&mut self, id: NodeId) {
        if let Self::Connecting { link } = self {
            *self = Self::Connected { link: *link, id };
        } else {
            panic!("Peer::connected: session for {} is already established", id);
        }
    }

    /// Switch to disconnected state.
    fn disconnected(&mut self, reason: DisconnectReason) {
        if let Self::Connected { id, .. } = self {
            *self = Self::Disconnected {
                id: Some(*id),
                reason,
            };
        } else if let Self::Connecting { .. } = self {
            *self = Self::Disconnected { id: None, reason };
        } else {
            panic!("Peer::disconnected: session is not connected ({self})");
        }
    }

    /// Switch to upgrading state.
    fn upgrading(&mut self, fetch: Fetch) {
        if let Self::Connected { id, link } = self {
            *self = Self::Upgrading {
                fetch,
                id: *id,
                link: *link,
            };
        } else {
            panic!("Peer::upgrading: session is not connected");
        }
    }

    /// Switch to upgraded state.
    fn upgraded(&mut self, listener: chan::Receiver<WorkerResp<G>>) -> Fetch {
        if let Self::Upgrading { fetch, id, link } = self {
            let fetch = fetch.clone();
            log::debug!(target: "transport", "Peer {id} upgraded for fetch {}", fetch.repo);

            *self = Self::Upgraded {
                id: *id,
                link: *link,
                response: listener,
            };
            fetch
        } else {
            panic!("Peer::upgraded: can't upgrade before handover");
        }
    }

    /// Switch back from upgraded to connected state.
    fn downgrade(&mut self) {
        if let Self::Upgraded { id, link, .. } = self {
            *self = Self::Connected {
                id: *id,
                link: *link,
            };
        } else {
            panic!("Peer::downgrade: can't downgrade if not in upgraded state");
        }
    }
}

/// Transport protocol implementation for a set of peers.
pub struct Wire<R, S, W, G: Signer + EcSign> {
    /// Backing service instance.
    service: Service<R, S, W, G>,
    /// Worker pool interface.
    worker: chan::Sender<WorkerReq<G>>,
    /// Used for authorization; keeps local identity.
    cert: Cert<Signature>,
    /// Used for authorization to sign the remote challenge.
    signer: G,
    /// Address of SOCKS5 proxy.
    proxy_addr: SocketAddr,
    /// Internal queue of actions to send to the reactor.
    actions: VecDeque<Action<G>>,
    /// Peer sessions.
    peers: HashMap<RawFd, Peer<G>>,
    /// Buffer for incoming peer data.
    read_queue: VecDeque<u8>,
}

impl<R, S, W, G> Wire<R, S, W, G>
where
    R: routing::Store,
    S: address::Store,
    W: WriteStorage + 'static,
    G: Signer + EcSign,
{
    pub fn new(
        mut service: Service<R, S, W, G>,
        worker: chan::Sender<WorkerReq<G>>,
        cert: Cert<Signature>,
        signer: G,
        proxy_addr: SocketAddr,
        clock: LocalTime,
    ) -> Self {
        service.initialize(clock);

        Self {
            service,
            worker,
            cert,
            signer,
            proxy_addr,
            actions: VecDeque::new(),
            peers: HashMap::default(),
            read_queue: VecDeque::new(),
        }
    }

    fn peer_mut_by_fd(&mut self, fd: RawFd) -> &mut Peer<G> {
        self.peers.get_mut(&fd).unwrap_or_else(|| {
            log::error!(target: "transport", "Peer with fd {fd} was not found");
            panic!("peer with fd {fd} is not known");
        })
    }

    fn fd_by_id(&self, node_id: &NodeId) -> (RawFd, &Peer<G>) {
        self.peers
            .iter()
            .find(|(_, peer)| match peer {
                Peer::Connected { id, .. }
                | Peer::Disconnected { id: Some(id), .. }
                | Peer::Upgrading { id, .. }
                | Peer::Upgraded { id, .. }
                    if id == node_id =>
                {
                    true
                }
                _ => false,
            })
            .map(|(fd, peer)| (*fd, peer))
            .unwrap_or_else(|| {
                log::error!(target: "transport", "No peer with id {node_id}");
                panic!("peer id {node_id} was expected to be known to the transport")
            })
    }

    fn connected_fd_by_id(&self, node_id: &NodeId) -> RawFd {
        match self.fd_by_id(node_id) {
            (fd, Peer::Connected { .. }) => fd,
            (fd, peer) => {
                log::error!(target: "transport", "Peer {peer}(fd={fd} is not in a connected state");
                panic!("peer {peer}(fd={fd} was expected to be in a connected state")
            }
        }
    }

    fn connected(&self) -> impl Iterator<Item = (RawFd, &NodeId)> {
        self.peers.iter().filter_map(|(fd, peer)| {
            if let Peer::Connected { id, .. } = peer {
                Some((*fd, id))
            } else {
                None
            }
        })
    }

    fn disconnect(&mut self, fd: RawFd, reason: DisconnectReason) {
        let peer = self.peer_mut_by_fd(fd);
        if let Peer::Disconnected { .. } = peer {
            log::error!(target: "transport", "Peer {peer}(fd={fd}) is already disconnected");
            return;
        };
        log::debug!(target: "transport", "Disconnecting peer {peer}(fd={fd}) because of {}...", reason);
        peer.disconnected(reason);

        self.actions.push_back(Action::UnregisterTransport(fd));
    }

    fn upgrade(&mut self, fd: RawFd, fetch: Fetch) {
        let peer = self.peer_mut_by_fd(fd);
        if let Peer::Disconnected { .. } = peer {
            log::error!(target: "transport", "Peer {peer}(fd={fd}) is already disconnected");
            return;
        };
        log::debug!(target: "transport", "Requesting transport handover from reactor for peer {peer}(fd={fd})");
        peer.upgrading(fetch);

        self.actions.push_back(Action::UnregisterTransport(fd));
    }

    fn upgraded(&mut self, transport: NetTransport<WireSession<G>>) {
        let fd = transport.as_raw_fd();
        let peer = self.peer_mut_by_fd(fd);
        let (send, recv) = chan::bounded::<WorkerResp<G>>(1);
        let fetch = peer.upgraded(recv);
        log::debug!(target: "transport", "Peer {peer}(fd={fd}) got upgraded");

        let session = match transport.into_session() {
            Ok(session) => session,
            Err(_) => panic!("write buffer is not empty when doing upgrade"),
        };

        if self
            .worker
            .send(WorkerReq {
                fetch,
                session,
                drain: self.read_queue.drain(..).collect(),
                channel: send,
            })
            .is_err()
        {
            log::error!(target: "transport", "Worker pool is disconnected; cannot send fetch request");
        }
    }

    fn fetch_complete(&mut self, resp: WorkerResp<G>) {
        let session = resp.session;
        let fd = session.as_connection().as_raw_fd();
        let peer = self.peer_mut_by_fd(fd);

        let session = if let Peer::Disconnected { .. } = peer {
            log::error!(target: "transport", "Peer with fd {fd} is already disconnected");
            return;
        } else if let Peer::Upgraded { link, .. } = peer {
            match NetTransport::with_session(session, *link) {
                Ok(session) => session,
                Err(err) => {
                    log::error!(target: "transport", "Session downgrade has failed due to {err}");
                    return;
                }
            }
        } else {
            todo!();
        };
        peer.downgrade();

        self.actions.push_back(Action::RegisterTransport(session));
        self.service.fetch_complete(resp.result);
    }
}

impl<R, S, W, G> reactor::Handler for Wire<R, S, W, G>
where
    R: routing::Store + Send,
    S: address::Store + Send,
    W: WriteStorage + Send + 'static,
    G: Signer + EcSign<Pk = NodeId, Sig = Signature> + Clone + Send,
{
    type Listener = NetAccept<WireSession<G>>;
    type Transport = NetTransport<WireSession<G>>;
    type Command = service::Command;

    fn tick(&mut self, _time: Duration) {
        // FIXME: Change this once a proper timestamp is passed into the function.
        self.service.tick(LocalTime::from(SystemTime::now()));

        let mut completed = Vec::new();
        for peer in self.peers.values() {
            if let Peer::Upgraded { response, .. } = peer {
                if let Ok(resp) = response.try_recv() {
                    completed.push(resp);
                }
            }
        }
        for resp in completed {
            self.fetch_complete(resp);
        }
    }

    fn handle_wakeup(&mut self) {
        self.service.wake()
    }

    fn handle_listener_event(
        &mut self,
        socket_addr: net::SocketAddr,
        event: ListenerEvent<WireSession<G>>,
        _: Duration,
    ) {
        match event {
            ListenerEvent::Accepted(connection) => {
                log::debug!(
                    target: "transport",
                    "Accepting inbound peer connection from {}..",
                    connection.remote_addr()
                );
                self.peers.insert(
                    connection.as_raw_fd(),
                    Peer::connecting(LinkDirection::Inbound),
                );

                let session = WireSession::accept::<{ Sha256::OUTPUT_LEN }>(
                    connection,
                    self.cert.clone(),
                    vec![],
                    self.signer.clone(),
                );

                let transport = match NetTransport::with_session(session, LinkDirection::Inbound) {
                    Ok(transport) => transport,
                    Err(err) => {
                        log::error!(target: "transport", "Failed to create reactor resource for the accepted connection: {err}");
                        return;
                    }
                };
                self.service.accepted(socket_addr);
                self.actions
                    .push_back(reactor::Action::RegisterTransport(transport))
            }
            ListenerEvent::Failure(err) => {
                log::error!(target: "transport", "Error listening for inbound connections: {err}");
            }
        }
    }

    fn handle_transport_event(
        &mut self,
        fd: RawFd,
        event: SessionEvent<WireSession<G>>,
        _: Duration,
    ) {
        match event {
            SessionEvent::Established(ProtocolArtifact {
                state: Cert { pk: node_id, .. },
                ..
            }) => {
                log::debug!(target: "transport", "Session established with {node_id}");

                let conflicting = self
                    .connected()
                    .filter(|(_, id)| **id == node_id)
                    .map(|(fd, _)| fd)
                    .collect::<Vec<_>>();

                for fd in conflicting {
                    log::warn!(
                        target: "transport", "Closing conflicting session with {node_id} (fd={fd})"
                    );
                    self.disconnect(
                        fd,
                        DisconnectReason::Dial(Arc::new(io::Error::from(
                            io::ErrorKind::AlreadyExists,
                        ))),
                    );
                }

                let Some(peer) = self.peers.get_mut(&fd) else {
                    log::error!(target: "transport", "Session not found for fd {fd}");
                    return;
                };
                let Peer::Connecting { link } = peer else {
                    log::error!(
                        target: "transport",
                        "Session for {node_id} was either not found, or in an invalid state"
                    );
                    return;
                };
                let link = *link;

                peer.connected(node_id);
                self.service.connected(node_id, link);
            }
            SessionEvent::Data(data) => {
                if let Some(Peer::Connected { id, .. }) = self.peers.get(&fd) {
                    self.read_queue.extend(data);

                    loop {
                        match Message::decode(&mut self.read_queue) {
                            Ok(msg) => self.service.received_message(*id, msg),
                            Err(err) if err.is_eof() => {
                                // Buffer is empty, or message isn't complete.
                                break;
                            }
                            Err(err) => {
                                // TODO(cloudhead): Include error in reason.
                                log::error!(target: "transport", "Invalid message from {}: {err}", id);
                                self.disconnect(
                                    fd,
                                    DisconnectReason::Session(session::Error::Misbehavior),
                                );
                                break;
                            }
                        }
                    }
                } else {
                    log::warn!(target: "transport", "Dropping message from unconnected peer with fd {fd}");
                }
            }
            SessionEvent::Terminated(err) => {
                log::debug!(target: "transport", "Session for fd {fd} terminated: {err}");
                self.disconnect(fd, DisconnectReason::Connection(Arc::new(err)));
            }
        }
    }

    fn handle_command(&mut self, cmd: Self::Command) {
        self.service.command(cmd);
    }

    fn handle_error(
        &mut self,
        err: reactor::Error<NetAccept<WireSession<G>>, NetTransport<WireSession<G>>>,
    ) {
        match &err {
            reactor::Error::ListenerUnknown(id) => {
                // TODO: What are we supposed to do here? Remove this error.
                log::error!(target: "transport", "Received error: unknown listener {}", id);
            }
            reactor::Error::TransportUnknown(id) => {
                // TODO: What are we supposed to do here? Remove this error.
                log::error!(target: "transport", "Received error: unknown peer {}", id);
            }
            reactor::Error::Poll(err) => {
                // TODO: This should be a fatal error, there's nothing we can do here.
                log::error!(target: "transport", "Can't poll connections: {}", err);
            }
            reactor::Error::ListenerPollError(id, err) => {
                // TODO: This should be a fatal error, there's nothing we can do here.
                log::error!(target: "transport", "Received error: listener {} disconnected: {}", id, err);
                self.actions.push_back(Action::UnregisterListener(*id));
            }
            reactor::Error::ListenerDisconnect(id, _, err) => {
                // TODO: This should be a fatal error, there's nothing we can do here.
                log::error!(target: "transport", "Received error: listener {} disconnected: {}", id, err);
            }
            reactor::Error::TransportPollError(id, err) => {
                log::error!(target: "transport", "Received error: peer {} disconnected: {}", id, err);
                self.actions.push_back(Action::UnregisterTransport(*id));
            }
            reactor::Error::TransportDisconnect(id, _, err) => {
                log::error!(target: "transport", "Received error: peer {} disconnected: {}", id, err);
            }
            reactor::Error::WriteFailure(id, err) => {
                // TODO: Disconnect peer?
                log::error!(target: "transport", "Error during writing to peer {id}: {err}")
            }
            reactor::Error::WriteLogicError(id, _) => {
                // TODO: We shouldn't be receiving this error. There's nothing we can do.
                log::error!(target: "transport", "Write logic error for peer {id}: {err}")
            }
        }
    }

    fn handover_listener(&mut self, _listener: Self::Listener) {
        panic!("Transport::handover_listener: listener handover is not supported");
    }

    fn handover_transport(&mut self, transport: Self::Transport) {
        let fd = transport.as_raw_fd();

        match self.peers.get(&fd) {
            Some(Peer::Disconnected { id, reason }) => {
                // Disconnect TCP stream.
                drop(transport);

                if let Some(id) = id {
                    self.service.disconnected(*id, reason);
                } else {
                    // TODO: Handle this case by calling `disconnected` with the address instead of
                    // the node id.
                }
            }
            Some(Peer::Upgrading { .. }) => {
                log::debug!(target: "transport", "Received handover of transport with fd {fd}");

                self.upgraded(transport);
            }
            Some(_) => {
                panic!("Transport::handover_transport: Unexpected peer with fd {fd} handed over from the reactor");
            }
            None => {
                panic!("Transport::handover_transport: Unknown peer with fd {fd} handed over");
            }
        }
    }
}

impl<R, S, W, G> Iterator for Wire<R, S, W, G>
where
    R: routing::Store,
    S: address::Store,
    W: WriteStorage + 'static,
    G: Signer + EcSign<Pk = NodeId, Sig = Signature>,
{
    type Item = Action<G>;

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(ev) = self.service.next() {
            match ev {
                Io::Write(node_id, msgs) => {
                    log::debug!(
                        target: "transport", "Sending {} message(s) to {}", msgs.len(), node_id
                    );
                    let fd = self.connected_fd_by_id(&node_id);
                    let mut data = Vec::new();
                    for msg in msgs {
                        msg.encode(&mut data).expect("in-memory writes never fail");
                    }
                    self.actions.push_back(reactor::Action::Send(fd, data));
                }
                Io::Event(_e) => {
                    log::warn!(
                        target: "transport", "Events are not currently supported"
                    );
                }
                Io::Connect(node_id, addr) => {
                    if self.connected().any(|(_, id)| id == &node_id) {
                        log::error!(
                            target: "transport",
                            "Attempt to connect to already connected peer {node_id}"
                        );
                        break;
                    }

                    match WireSession::connect_nonblocking::<{ Sha256::OUTPUT_LEN }>(
                        addr.to_inner(),
                        self.cert,
                        vec![node_id],
                        self.signer.clone(),
                        self.proxy_addr.into(),
                        false,
                    )
                    .and_then(|session| {
                        NetTransport::<WireSession<G>>::with_session(
                            session,
                            LinkDirection::Outbound,
                        )
                    }) {
                        Ok(transport) => {
                            self.service.attempted(node_id, &addr);
                            // TODO: Keep track of peer address for when peer disconnects before
                            // handshake is complete.
                            self.peers.insert(
                                transport.as_raw_fd(),
                                Peer::connecting(LinkDirection::Outbound),
                            );

                            self.actions
                                .push_back(reactor::Action::RegisterTransport(transport));
                        }
                        Err(err) => {
                            self.service
                                .disconnected(node_id, &DisconnectReason::Dial(Arc::new(err)));
                            break;
                        }
                    }
                }
                Io::Disconnect(node_id, reason) => {
                    let fd = self.connected_fd_by_id(&node_id);
                    self.disconnect(fd, reason);
                }
                Io::Wakeup(d) => {
                    self.actions.push_back(reactor::Action::SetTimer(d.into()));
                }
                Io::Fetch(fetch) => {
                    // TODO: Check that the node_id is connected, queue request otherwise.
                    let fd = self.connected_fd_by_id(&fetch.remote);
                    self.upgrade(fd, fetch);
                }
            }
        }
        self.actions.pop_front()
    }
}
