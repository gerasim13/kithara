use kithara_platform::time::Duration;

use super::{Size2, SizeLimits, TextMeasurer};
use crate::{
    draw::{DrawListBuilder, Rect},
    interact::{Hit, Input, Outcome},
};

/// Frame scheduling requested by custom content.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub enum Repaint {
    /// Paint only when external state invalidates the widget.
    #[default]
    None,
    /// Request one animation frame and then return to external invalidation.
    NextFrame,
    /// Keep requesting animation frames until the widget changes this declaration.
    Continuous,
}

/// Public drawing and intrinsic-measurement contract for hosted content.
///
/// Intrinsic measurement is authoritative only on document axes declared
/// `Shrink`. A `Fill` axis receives the resolved document rectangle in
/// [`Self::paint`]. Content that preserves an authored aspect ratio must
/// letterbox itself inside that rectangle; the host never stretches or applies
/// an affine compensation behind the document's back.
pub trait CustomWidget: 'static {
    /// Consumer-owned action emitted by this component.
    type Action: std::fmt::Debug + Send + 'static;

    /// Returns the content's intrinsic logical extent under the supplied limits.
    fn measure(&mut self, text: &mut TextMeasurer<'_>, limits: SizeLimits) -> Size2;

    /// Handles one neutral input event in the component's local hit space.
    fn input(&mut self, _input: Input<'_>, _hit: Hit) -> Outcome<Self::Action> {
        Outcome::IGNORED
    }

    /// Whether focusing this component should start a platform input-method
    /// session. Only the retained host opens one; iced owns that session in the
    /// immediate host, so the question is asked there.
    #[cfg(feature = "masonry")]
    fn accepts_text_input(&self) -> bool {
        false
    }

    /// Advances retained component state and returns at most one typed action.
    ///
    /// This hook is the egress for actions recognised while drawing or by a
    /// consumer-owned signal such as a long-press recognizer. The host invokes
    /// it only from its normal animation-frame lifecycle.
    fn frame(&mut self, _elapsed: Duration) -> Option<Self::Action> {
        None
    }

    /// Appends drawing commands for the resolved local rectangle.
    fn paint(&mut self, list: &mut DrawListBuilder, text: &mut TextMeasurer<'_>, bounds: Rect);

    /// Declares whether the host should schedule animation frames.
    fn repaint(&self) -> Repaint {
        Repaint::None
    }
}
