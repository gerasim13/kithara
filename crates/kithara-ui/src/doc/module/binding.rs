use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::ids::{EndpointId, StateId};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub enum BindingRef {
    Command {
        id: EndpointId,
        #[serde(default)]
        with: BTreeMap<String, String>,
    },
    Parameter {
        id: EndpointId,
        #[serde(default)]
        with: BTreeMap<String, String>,
    },
    Telemetry {
        id: EndpointId,
        #[serde(default)]
        with: BTreeMap<String, String>,
    },
    Model {
        id: EndpointId,
        #[serde(default)]
        with: BTreeMap<String, String>,
    },
    /// State the view keeps for itself, which no application declares or is
    /// told about. `set` is what a press does to it, and is read only on the
    /// side that writes: which side a binding sits on is the slot it fills.
    View {
        id: StateId,
        #[serde(default)]
        set: ViewSet,
    },
    /// One page of a [`crate::doc::layout::LayoutNode::Tabs`], named by the
    /// state that says which page stands. A read answers whether the state
    /// stands at this page, a write stands it here.
    Page { id: StateId, name: String },
}

/// What a write on a [`BindingRef::View`] does to the state it names.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub enum ViewSet {
    /// Leaves the state at whichever of its two values it was not.
    #[default]
    Toggle,
    On,
    Off,
}
