use crate::size::SizeSpec;

/// A region that moves the window rather than drawing anything.
#[derive(kithara_derive::Control)]
#[control(size = SizeSpec::FILL)]
pub(crate) struct Drag;
