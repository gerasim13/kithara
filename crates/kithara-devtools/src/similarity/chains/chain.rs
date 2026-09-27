//! One row of the report: two sides that part at a fork.

use std::{collections::BTreeSet, fmt};

use serde::{Serialize, Serializer};

use super::{body::DecisionKind, minhash, search::Region};

/// How a chain was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Origin {
    /// The arms of one decision.
    Decision,
    /// The impls of one trait method.
    Dyn,
    /// Two alike roots.
    Pair,
    /// Two roots whose owned regions are alike.
    Region,
}

/// What separates the two sides of a chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Split {
    /// The arms of a decision reach one side each.
    Decision(DecisionKind),
    /// One side runs where the other side's arm fails.
    Fallback(DecisionKind),
    /// A trait-object call picks the side.
    Dyn,
    /// The common caller runs both sides one after the other.
    Sequence,
    /// The common caller reaches both sides only through one callee.
    ViaCallee,
    /// No common caller: two entries of an API.
    Entry,
}

impl fmt::Display for Split {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Decision(kind) => formatter.write_str(kind.label()),
            Self::Fallback(kind) => write!(formatter, "fallback:{}", kind.label()),
            Self::Dyn => formatter.write_str("dyn"),
            Self::Sequence => formatter.write_str("seq"),
            Self::ViaCallee => formatter.write_str("via-callee"),
            Self::Entry => formatter.write_str("entry"),
        }
    }
}

impl Serialize for Split {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

/// The function where the two sides part.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct Fork {
    pub(crate) function: String,
    pub(crate) location: String,
}

/// The two most alike functions across the sides.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct Alike {
    pub(crate) functions: [String; 2],
    pub(crate) similarity: f64,
}

/// One side: the functions only its entry calls into.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct Side {
    /// The root function, or the arm of the decision.
    pub(crate) entry: String,
    pub(crate) location: String,
    pub(crate) members: Vec<String>,
    /// The arm's own code counts as one more unit.
    pub(crate) inline: bool,
    pub(crate) depth: usize,
    pub(crate) functions: usize,
    pub(crate) lines: usize,
}

impl Side {
    pub(crate) fn units(&self) -> usize {
        self.functions + usize::from(self.inline)
    }
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct Chain {
    pub(crate) alike: Option<Alike>,
    /// Where the value of each impl is built, for a trait-object chain.
    pub(crate) built: Option<[Vec<String>; 2]>,
    pub(crate) fork: Option<Fork>,
    pub(crate) origin: Origin,
    pub(crate) split: Split,
    pub(crate) sides: [Side; 2],
    /// The sides build under different cfgs.
    pub(crate) platform: bool,
    pub(crate) containment: f64,
    pub(crate) jaccard: f64,
    /// The functions of both sides: two rows over the same functions are one.
    #[serde(skip)]
    pub(super) fns: BTreeSet<usize>,
}

impl Chain {
    pub(super) fn new(
        origin: Origin,
        split: Split,
        fork: Option<Fork>,
        [x, y]: [&Region; 2],
        alike: Option<Alike>,
    ) -> Self {
        Self {
            origin,
            split,
            fork,
            alike,
            platform: false,
            sides: [x.side.clone(), y.side.clone()],
            jaccard: minhash::jaccard(&x.shingles, &y.shingles),
            containment: minhash::containment(&x.shingles, &y.shingles),
            built: None,
            fns: x.fns.union(&y.fns).copied().collect(),
        }
    }
}
