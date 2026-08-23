use std::collections::BTreeSet;

use super::{module::walk_module, path::NodePath};
use crate::{
    error::UiDocError,
    ids::{NodeId, SourceUri},
    module::{AdaptiveStep, ControlNode, Measure, MeasureAxis},
    size::{Dim, SizeSpec},
};

pub(super) fn check_adaptive_steps(
    id: &NodeId,
    steps: &[AdaptiveStep],
    path: &NodePath,
    origin: &SourceUri,
) -> Result<(), UiDocError> {
    let thresholds: Vec<f32> = steps.iter().map(|step| step.from).collect();
    check_thresholds(id, &thresholds, path, origin)
}

pub(super) fn check_thresholds(
    id: &NodeId,
    steps: &[f32],
    path: &NodePath,
    origin: &SourceUri,
) -> Result<(), UiDocError> {
    if steps.is_empty() {
        return Err(UiDocError::AdaptiveWithoutSteps {
            origin: origin.clone(),
            id: id.0.clone(),
            path: path.render(),
        });
    }
    let mut below = f32::NEG_INFINITY;
    for (index, from) in steps.iter().copied().enumerate() {
        if from <= below || !from.is_finite() {
            return Err(UiDocError::AdaptiveStepOrder {
                origin: origin.clone(),
                path: path.render(),
                from,
                index,
            });
        }
        below = from;
    }
    Ok(())
}

pub(super) fn check_measured_box(
    axis: MeasureAxis,
    size: Option<SizeSpec>,
    path: &NodePath,
    origin: &SourceUri,
) -> Result<(), UiDocError> {
    let declared = size.map(|size| match axis {
        MeasureAxis::Width => size.w,
        MeasureAxis::Height => size.h,
    });
    if matches!(declared, Some(dim) if dim != Dim::Shrink) {
        return Ok(());
    }
    Err(UiDocError::UnmeasuredAxis {
        origin: origin.clone(),
        path: path.render(),
        axis: axis.name(),
    })
}

pub(super) fn check_adaptive_measure(
    id: &NodeId,
    measure: &Measure,
    size: Option<SizeSpec>,
    path: &NodePath,
    origin: &SourceUri,
) -> Result<(), UiDocError> {
    match (measure.axis(), size) {
        (Some(axis), size) => check_measured_box(axis, size, path, origin),
        (None, None) => Ok(()),
        (None, Some(_)) => Err(UiDocError::MeasuredBoxWithoutAxis {
            origin: origin.clone(),
            id: id.0.clone(),
            path: path.render(),
        }),
    }
}

pub(super) fn walk_branches(
    base: &ControlNode,
    steps: &[AdaptiveStep],
    path: &NodePath,
    origin: &SourceUri,
    seen: &mut BTreeSet<String>,
) -> Result<(), UiDocError> {
    let taken = seen.clone();
    walk_branch(base, &path.push("base"), origin, &taken, seen)?;
    for (index, step) in steps.iter().enumerate() {
        let branch = path.push(format!("steps[{index}]"));
        walk_branch(&step.node, &branch, origin, &taken, seen)?;
    }
    Ok(())
}

pub(super) fn walk_branch(
    node: &ControlNode,
    path: &NodePath,
    origin: &SourceUri,
    taken: &BTreeSet<String>,
    seen: &mut BTreeSet<String>,
) -> Result<(), UiDocError> {
    let mut claimed = taken.clone();
    walk_module(node, path, origin, &mut claimed, Sibling::Only)?;
    seen.extend(claimed);
    Ok(())
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Sibling {
    Among,
    Measured,
    Only,
}

impl Sibling {
    const fn laid_out_among_siblings(self) -> bool {
        matches!(self, Self::Among | Self::Measured)
    }
}

pub(super) fn check_block_position(
    id: &NodeId,
    path: &NodePath,
    origin: &SourceUri,
    sibling: Sibling,
) -> Result<(), UiDocError> {
    if sibling.laid_out_among_siblings() {
        return Ok(());
    }
    Err(UiDocError::RootBlock {
        origin: origin.clone(),
        id: id.0.clone(),
        path: path.render(),
    })
}

pub(super) fn check_reveal(
    from: f32,
    until: Option<f32>,
    path: &NodePath,
    origin: &SourceUri,
    sibling: Sibling,
) -> Result<(), UiDocError> {
    if sibling != Sibling::Measured {
        return Err(UiDocError::UnmeasuredReveal {
            origin: origin.clone(),
            path: path.render(),
        });
    }
    if !from.is_finite() || from < 0.0 {
        return Err(UiDocError::RevealThreshold {
            origin: origin.clone(),
            path: path.render(),
            from,
        });
    }
    match until {
        Some(until) if !until.is_finite() || until <= from => Err(UiDocError::RevealBand {
            origin: origin.clone(),
            path: path.render(),
            from,
            until,
        }),
        _ => Ok(()),
    }
}
