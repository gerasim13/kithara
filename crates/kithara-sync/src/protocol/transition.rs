use crate::{SyncExecutionStamp, SyncPreparation};

/// Every preparation one committed state change issues and withdraws across
/// the subtree it reached.
///
/// Each issued preparation replaces whatever its member held before; each
/// withdrawn stamp names a preparation dropped with no successor. An executor
/// that follows only admissions and transitions holds exactly the
/// preparations the owners hold.
#[derive(Clone, Debug, Default, PartialEq, fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
#[non_exhaustive]
pub struct SyncTransition {
    /// Preparations issued, each superseding its member's previous one.
    #[field(get)]
    issued: Vec<SyncPreparation>,
    /// Preparations dropped without a successor.
    #[field(get)]
    withdrawn: Vec<SyncExecutionStamp>,
}

impl SyncTransition {
    pub(crate) const fn new(
        issued: Vec<SyncPreparation>,
        withdrawn: Vec<SyncExecutionStamp>,
    ) -> Self {
        Self { issued, withdrawn }
    }

    /// Adds the preparations a nested group's change issued and withdrew.
    pub(crate) fn append(&mut self, mut nested: Self) {
        self.issued.append(&mut nested.issued);
        self.withdrawn.append(&mut nested.withdrawn);
    }
}
