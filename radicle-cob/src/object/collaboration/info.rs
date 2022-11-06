// Copyright © 2022 The Radicle Link Contributors
//
// This file is part of radicle-link, distributed under the GPLv3 with Radicle
// Linking Exception. For full terms see the included LICENSE file.

//! [`ChangeGraphInfo`] provides a useful debugging structure for
//! represnting a single [`crate::CollaborativeObject`]'s underlying
//! change graph. This includes a [`ChangeGraphInfo::dotviz`] for
//! describing the graph via [graphviz].
//!
//! [graphviz]: https://graphviz.org/

use std::collections::BTreeSet;

use git_ext::Oid;

use crate::{change_graph::ChangeGraph, identity::Identity, ObjectId, Store, TypeName};

use super::error;

/// Additional information about the change graph of an object
pub struct ChangeGraphInfo {
    /// The ID of the object
    pub object_id: ObjectId,
    /// A graphviz description of the changegraph of the object
    pub dotviz: String,
    /// The number of nodes in the change graph of the object
    pub number_of_nodes: u64,
    /// The "tips" of the change graph, i.e the object IDs pointed to by
    /// references to the object
    pub tips: BTreeSet<Oid>,
}

/// Retrieve additional information about the change graph of an object. This
/// is mostly useful for debugging and testing
///
/// The `storage` is the backing storage for storing
/// [`crate::Change`]s at content-addressable locations. Please see
/// [`Store`] for further information.
///
/// The `resource` is the parent of this object, for example a
/// software project.
///
/// The `typename` is the type of object to be found, while the `oid`
/// is the identifier for the particular object under that type.
pub fn changegraph<S, Resource>(
    storage: &S,
    resource: &Resource,
    typename: &TypeName,
    oid: &ObjectId,
) -> Result<Option<ChangeGraphInfo>, error::Retrieve>
where
    S: Store<Resource>,
    Resource: Identity,
{
    let tip_refs = storage
        .objects(&resource.identifier(), typename, oid)
        .map_err(|err| error::Retrieve::Refs { err: Box::new(err) })?;
    Ok(
        ChangeGraph::load(storage, tip_refs.iter(), typename, oid).map(|graph| ChangeGraphInfo {
            object_id: *oid,
            dotviz: graph.graphviz(),
            number_of_nodes: graph.number_of_nodes(),
            tips: graph.tips(),
        }),
    )
}