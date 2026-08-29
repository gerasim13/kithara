use std::collections::BTreeSet;

use super::path::NodePath;
use crate::{
    error::UiDocError,
    ids::SourceUri,
    module::{BindingRef, ControlNode, Magnet},
};

/// The placements one stage holds, by the id a magnet names them with.
pub(super) fn placements(children: &[ControlNode]) -> BTreeSet<&str> {
    children
        .iter()
        .filter_map(|child| match child {
            ControlNode::Placed { id, .. } => Some(id.0.as_str()),
            _ => None,
        })
        .collect()
}

/// What a placement has to declare for the parts of it to mean anything.
///
/// A point the document reads, somewhere to publish the point a drag ends on,
/// and a magnet are three halves of one contract: publishing without reading
/// leaves the placement standing still while the endpoint moves, and a magnet
/// on a placement no pointer carries never has an occasion to pull.
pub(super) fn check_placement(
    read: Option<&BindingRef>,
    write: Option<&BindingRef>,
    magnet: Option<&Magnet>,
    scene: &BTreeSet<&str>,
    path: &NodePath,
    origin: &SourceUri,
) -> Result<(), UiDocError> {
    if write.is_some() && read.is_none() {
        return Err(UiDocError::PlacedUnread {
            origin: origin.clone(),
            path: path.render(),
        });
    }
    let Some(magnet) = magnet else {
        return Ok(());
    };
    if write.is_none() {
        return Err(UiDocError::MagnetUncarried {
            origin: origin.clone(),
            path: path.render(),
        });
    }
    if !(magnet.within.is_finite() && magnet.within > 0.0) {
        return Err(UiDocError::MagnetReach {
            origin: origin.clone(),
            path: path.render(),
            within: magnet.within,
        });
    }
    for target in &magnet.to {
        if !scene.contains(target.0.as_str()) {
            return Err(UiDocError::MagnetUnknown {
                origin: origin.clone(),
                path: path.render(),
                target: target.0.clone(),
            });
        }
    }
    Ok(())
}
