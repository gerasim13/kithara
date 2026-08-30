use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::ron_io;
use crate::{
    envelope::{self, DocKind},
    error::UiDocError,
    ids::{DocId, InstanceId, NodeId, SourceUri, StateId},
    module::{BindingRef, MeasureAxis},
    size::SizeSpec,
};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct LayoutDoc {
    pub id: DocId,
    pub root: LayoutNode,
    /// Names the item the pointer is carrying. While it reads as text, the
    /// layout draws that text at the pointer, over everything it lays out.
    #[serde(default)]
    pub dragged: Option<BindingRef>,
    pub schema: String,
    /// A window without system decorations has to be resized by its own edges;
    /// the renderer frames the root with them when this is set.
    #[serde(default)]
    pub resize_edges: bool,
    pub version: u32,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub enum LayoutNode {
    /// Lays its children out along one axis. Declaring `measure` reads the box
    /// it is given on that axis, and each child stands while the band it names
    /// holds that number.
    Split {
        axis: Axis,
        #[serde(default)]
        measure: Option<MeasureAxis>,
        #[serde(default)]
        size: Option<SizeSpec>,
        children: Vec<SplitChild>,
    },
    /// Marks its node as a block the host may hide. While `hidden` reads true
    /// the node is not laid out.
    Optional {
        id: NodeId,
        hidden: BindingRef,
        node: Box<Self>,
    },
    /// Lays out the form that fits the room it is given: the last step whose
    /// threshold the measured axis reaches, and `base` below the first.
    Adaptive {
        id: NodeId,
        measure: MeasureAxis,
        size: SizeSpec,
        base: Box<Self>,
        steps: Vec<AdaptiveStep>,
    },
    Module {
        instance: InstanceId,
        source: String,
        #[serde(default)]
        with: BTreeMap<String, String>,
        #[serde(default)]
        size: Option<SizeSpec>,
        #[serde(default)]
        frame: FrameSides,
        /// Draws the decorative ticks at the top-left and bottom-right of the
        /// module frame.
        #[serde(default)]
        corners: bool,
    },
    /// Shows the one page its state stands at, and compiles no other.
    ///
    /// The body alone: what turns the state is an ordinary control writing a
    /// [`crate::doc::module::BindingRef::Page`], so a document keeps every say
    /// over the chrome that offers the pages.
    Tabs {
        instance: InstanceId,
        state: StateId,
        /// The page a screen that has turned nothing stands at.
        initial: String,
        /// The document each page shows, by the name a control writes.
        pages: BTreeMap<String, String>,
        #[serde(default)]
        with: BTreeMap<String, String>,
        #[serde(default)]
        size: Option<SizeSpec>,
        #[serde(default)]
        frame: FrameSides,
        #[serde(default)]
        corners: bool,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct FrameSides {
    #[serde(default = "default_frame_side")]
    pub bottom: bool,
    #[serde(default = "default_frame_side")]
    pub left: bool,
    #[serde(default = "default_frame_side")]
    pub right: bool,
    #[serde(default = "default_frame_side")]
    pub top: bool,
}

impl Default for FrameSides {
    fn default() -> Self {
        Self {
            top: true,
            right: true,
            bottom: true,
            left: true,
        }
    }
}

/// Which of a box's four corners are the window's own.
///
/// The window has no frame of its own to round: what stands at its corner is
/// whichever module the layout puts there, so the shape of the window is the
/// shape of those boxes. A compiled layout hands each module the corners it
/// inherits from the root, and the skin's frame radius does the rest.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct FrameCorners {
    pub bottom_left: bool,
    pub bottom_right: bool,
    pub top_left: bool,
    pub top_right: bool,
}

impl FrameCorners {
    /// No corner, which is what a box inside the window is given.
    pub const EMPTY: Self = Self {
        bottom_left: false,
        bottom_right: false,
        top_left: false,
        top_right: false,
    };

    /// Every corner, which is what the root of a layout is given.
    pub const ALL: Self = Self {
        bottom_left: true,
        bottom_right: true,
        top_left: true,
        top_right: true,
    };

    /// The top pair of `self`, the bottom pair dropped.
    #[must_use]
    pub const fn top(self) -> Self {
        Self {
            bottom_left: false,
            bottom_right: false,
            ..self
        }
    }

    /// The bottom pair of `self`, the top pair dropped.
    #[must_use]
    pub const fn bottom(self) -> Self {
        Self {
            top_left: false,
            top_right: false,
            ..self
        }
    }

    /// The left pair of `self`, the right pair dropped.
    #[must_use]
    pub const fn left(self) -> Self {
        Self {
            bottom_right: false,
            top_right: false,
            ..self
        }
    }

    /// The right pair of `self`, the left pair dropped.
    #[must_use]
    pub const fn right(self) -> Self {
        Self {
            bottom_left: false,
            top_left: false,
            ..self
        }
    }

    /// Whether any corner is the window's.
    #[must_use]
    pub const fn any(self) -> bool {
        self.bottom_left || self.bottom_right || self.top_left || self.top_right
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub enum Axis {
    Horizontal,
    Vertical,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct AdaptiveStep {
    pub from: f32,
    pub node: LayoutNode,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct SplitChild {
    pub node: LayoutNode,
    #[serde(default = "default_weight")]
    pub weight: f32,
    /// The room the child stands from, and the room it stands until. Both
    /// answer the axis its split measures, and the pair a child keeps by
    /// default stands in every room.
    #[serde(default)]
    pub from: f32,
    #[serde(default)]
    pub until: Option<f32>,
}

const fn default_weight() -> f32 {
    1.0
}

const fn default_frame_side() -> bool {
    true
}

/// Parses a validated layout document.
///
/// # Errors
/// Returns [`UiDocError`] when the envelope or layout body is invalid.
pub fn parse_layout(text: &str, origin: &SourceUri) -> Result<LayoutDoc, UiDocError> {
    let envelope = envelope::probe(text, origin)?;
    if envelope.kind != DocKind::Layout {
        return Err(UiDocError::WrongDocKind {
            origin: origin.clone(),
            expected: DocKind::Layout.name(),
            found: envelope.kind.name(),
        });
    }
    ron_io::options()
        .from_str(text)
        .map_err(|source| UiDocError::Syntax {
            origin: origin.clone(),
            source: Box::new(source),
        })
}
