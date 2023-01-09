use std::path::{Path, PathBuf};
use std::{
    collections::{BTreeMap, BTreeSet},
    net, thread,
};

use nakamoto_net::LocalTime;

use radicle::identity::Id;
use radicle::node::Handle;
use radicle::test::arbitrary;
use radicle::Profile;
use radicle::Storage;
use radicle_crypto::ssh::keystore::MemorySigner;

use crate::address;
use crate::client::ROUTING_DB_FILE;
use crate::clock::Timestamp;
use crate::logger;
use crate::node::NodeId;
use crate::service::routing;
use crate::service::routing::Store as _;
use crate::wire::Transport;
use crate::{client, client::Runtime, service};

type TestHandle = (
    client::handle::Handle<Transport<routing::Table, address::Book, Storage, MemorySigner>>,
    thread::JoinHandle<Result<(), client::Error>>,
);

fn runtime(
    home: &Path,
    config: service::Config,
    routes: impl IntoIterator<Item = (Id, NodeId, Timestamp)>,
) -> Runtime<MemorySigner> {
    let profile = Profile::init(home, "pasphrase".to_owned()).unwrap();
    let signer = MemorySigner::gen();
    let listen = vec![([0, 0, 0, 0], 0).into()];
    let proxy = net::SocketAddr::new(net::Ipv4Addr::LOCALHOST.into(), 9050);

    let mut routing = routing::Table::open(profile.paths().node().join(ROUTING_DB_FILE)).unwrap();
    for (rid, node, time) in routes {
        routing.insert(rid, node, time).unwrap();
    }
    Runtime::with(profile, config, listen, proxy, signer).unwrap()
}

fn network(
    configs: impl IntoIterator<Item = (service::Config, PathBuf)>,
) -> BTreeMap<(NodeId, net::SocketAddr), TestHandle> {
    let mut runtimes = BTreeMap::new();
    for (config, home) in configs.into_iter() {
        let routes = {
            let rid = arbitrary::gen::<Id>(1);
            let node = arbitrary::gen::<NodeId>(1);
            let time = LocalTime::now().as_secs();
            vec![(rid, node, time)]
        };
        let rt = runtime(home.as_ref(), config, routes);
        let id = rt.id;
        let addr = *rt.local_addrs.first().unwrap();
        let handle = rt.handle.clone();
        let join = thread::spawn(|| rt.run());

        runtimes.insert((id, addr), (handle, join));
    }

    let mut connect = Vec::new();
    for (i, (from, _)) in runtimes.iter().enumerate() {
        let peers = runtimes
            .iter()
            .skip(i + 1)
            .map(|(p, _)| *p)
            .collect::<Vec<(NodeId, net::SocketAddr)>>();
        for to in peers {
            connect.push((*from, to));
        }
    }

    for (from, (to_id, to_addr)) in connect {
        let (handle, _) = runtimes.get_mut(&from).unwrap();
        handle.connect(to_id, to_addr.into()).unwrap();
    }
    runtimes
}

/// Checks whether the nodes have converged in their inventories.
#[track_caller]
fn check(
    nodes: impl IntoIterator<Item = ((NodeId, net::SocketAddr), TestHandle)>,
) -> BTreeSet<(Id, NodeId)> {
    let mut by_node = BTreeMap::<NodeId, BTreeSet<(Id, NodeId)>>::new();
    let mut all = BTreeSet::<(Id, NodeId)>::new();

    for ((id, _), (handle, _)) in nodes {
        let routing = handle.routing().unwrap();

        for (rid, node) in routing.try_iter() {
            all.insert((rid, node));
            by_node
                .entry(id)
                .or_insert_with(BTreeSet::new)
                .insert((rid, node));
        }
    }

    for (node, routes) in by_node {
        assert_eq!(routes, all, "{node} failed to converge");
    }
    all
}

#[test]
fn test_e2e() {
    logger::init(log::Level::Trace).unwrap();

    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    let nodes = network(vec![
        (service::Config::default(), base.join("alice")),
        (service::Config::default(), base.join("bob")),
        (service::Config::default(), base.join("eve")),
    ]);

    thread::sleep(std::time::Duration::from_secs(3));

    let routes = check(nodes);
    assert_eq!(routes.len(), 3);
}