use std::num::NonZeroU32;

use kithara_signal::{SessionEpoch, SourceSpan};
use kithara_warp::{MapAxis, WarpCursor, WarpMapRevision, WarpPlan};

/// Where a plan's staged lane enters the session: the cursor it activates
/// at, the Session epoch it belongs to, and the output and source rates it
/// maps between.
#[derive(Clone, Copy, Debug, Eq, PartialEq, fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
pub struct ActivationHead {
    /// The output frame, source frame and map revision the lane enters at.
    #[field(get, copy)]
    activation: WarpCursor,
    /// The Session epoch the plan's output axis belongs to.
    #[field(get, copy)]
    epoch: SessionEpoch,
    /// The sample rate of the plan's Session output.
    #[field(get, copy)]
    output_rate: NonZeroU32,
    /// The sample rate of the plan's asset source.
    #[field(get, copy)]
    source_rate: NonZeroU32,
}

impl ActivationHead {
    /// The head of `plan`, or `None` when the plan maps no Session output
    /// onto an asset source.
    #[must_use]
    pub fn of(plan: &WarpPlan) -> Option<Self> {
        let Some(MapAxis::Session(output)) = plan.output_axis() else {
            return None;
        };
        let Some(MapAxis::Asset(source)) = plan.source_axis() else {
            return None;
        };
        Some(Self {
            activation: plan.activation(),
            epoch: output.epoch(),
            output_rate: output.sample_rate(),
            source_rate: source.sample_rate(),
        })
    }

    /// The frame `stereo`, decoded from `source`, when it is the one frame
    /// this head enters at: one output frame from the head's source frame,
    /// at the source rate, on the head's map.
    #[must_use]
    pub(crate) fn first(self, stereo: [f32; 2], source: SourceSpan) -> Option<PreparedFirst> {
        (source.output_frames() == 1
            && source.start() == self.activation.source()
            && source.sample_rate() == self.source_rate
            && source.mapping_revision().map(WarpMapRevision::from)
                == Some(self.activation.revision()))
        .then_some(PreparedFirst {
            stereo,
            source,
            head: self,
        })
    }
}

/// The one frame a staged lane decoded at its head, consumed from the lane
/// off the audio callback.
#[derive(Clone, Copy, Debug, fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
pub struct PreparedFirst {
    /// The frame's left and right samples.
    #[field(get, copy)]
    stereo: [f32; 2],
    /// The source interval the frame was decoded from.
    #[field(get, copy)]
    source: SourceSpan,
    /// The head the frame enters at.
    #[field(get, copy)]
    head: ActivationHead,
}

/// A lane a port staged for a head, with the first frame it decoded.
pub struct Staged<L> {
    lane: L,
    stereo: [f32; 2],
    source: SourceSpan,
}

impl<L> Staged<L> {
    /// `lane`, whose first decoded frame is `stereo` from `source`.
    #[must_use]
    pub const fn new(lane: L, stereo: [f32; 2], source: SourceSpan) -> Self {
        Self {
            lane,
            stereo,
            source,
        }
    }

    /// The lane and its first frame, when that frame is the one `head`
    /// enters at; `None` drops the lane.
    pub(crate) fn at(self, head: ActivationHead) -> Option<(L, PreparedFirst)> {
        let first = head.first(self.stereo, self.source)?;
        Some((self.lane, first))
    }
}
