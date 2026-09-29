use std::{
    ffi::c_void,
    num::NonZeroUsize,
    ptr::{self, NonNull},
};

use super::ffi::{vDSP_DFT_DestroySetup, vDSP_DFT_Execute, vDSP_DFT_zrop_CreateSetup};

mod consts {
    /// `vDSP_DFT_FORWARD`.
    pub(super) const FORWARD: i32 = 1;
}

/// Why a real DFT could not be built or run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DftError {
    #[error("vDSP has no real DFT of this length")]
    Setup,
    #[error("buffer shape does not match the DFT")]
    Shape,
}

/// `vDSP_DFT_zrop`: the forward DFT of `len` real samples, where
/// `len = f·2ⁿ` with `f` in 1, 3, 5, 15 and `n ≥ 4`. The input arrives as
/// its even and odd samples, `len / 2` of each; the output is bins
/// `0..len / 2`, every one doubled, with the real Nyquist bin carried in
/// `im[0]` beside the real DC bin in `re[0]`.
///
/// vDSP picks its algorithm by where the planes sit, and two algorithms round
/// the same input differently. Every plane starts on [`Self::ALIGN`], where
/// the placement no longer changes the pick, so the output is a function of
/// the input alone.
pub struct RealDft {
    setup: NonNull<c_void>,
    half: NonZeroUsize,
}

// SAFETY: RealDft owns its setup and destroys it once, on drop.
unsafe impl Send for RealDft {}

// SAFETY: vDSP_DFT_Execute only reads the setup, and vDSP allows concurrent calls on one setup.
unsafe impl Sync for RealDft {}

impl RealDft {
    /// The byte boundary every plane starts on.
    pub const ALIGN: usize = 64;

    /// # Errors
    /// [`DftError::Setup`] when vDSP has no real DFT of `len` samples.
    pub fn new(len: usize) -> Result<Self, DftError> {
        let half = NonZeroUsize::new(len / 2).ok_or(DftError::Setup)?;
        // SAFETY: a null previous setup asks vDSP for a fresh one; vDSP returns null for an unsupported length.
        let setup = unsafe { vDSP_DFT_zrop_CreateSetup(ptr::null_mut(), len, consts::FORWARD) };
        NonNull::new(setup)
            .map(|setup| Self { setup, half })
            .ok_or(DftError::Setup)
    }

    /// # Errors
    /// [`DftError::Shape`] when a plane is not `len / 2` long or does not
    /// start on [`Self::ALIGN`].
    pub fn execute(
        &self,
        [even, odd]: [&[f32]; 2],
        [re, im]: [&mut [f32]; 2],
    ) -> Result<(), DftError> {
        let half = self.half.get();
        let planes = [even, odd, &*re, &*im];
        if planes
            .iter()
            .any(|plane| plane.len() != half || !plane.as_ptr().addr().is_multiple_of(Self::ALIGN))
        {
            return Err(DftError::Shape);
        }
        // SAFETY: setup is live and was created for 2 · half real samples.
        // SAFETY: even and odd hold half readable floats; re and im hold half writable floats.
        // SAFETY: the output planes are exclusive borrows, so they overlap neither input.
        unsafe {
            vDSP_DFT_Execute(
                self.setup.as_ptr(),
                even.as_ptr(),
                odd.as_ptr(),
                re.as_mut_ptr(),
                im.as_mut_ptr(),
            );
        }
        Ok(())
    }
}

impl Drop for RealDft {
    fn drop(&mut self) {
        // SAFETY: setup came from vDSP_DFT_zrop_CreateSetup and is destroyed exactly once.
        unsafe { vDSP_DFT_DestroySetup(self.setup.as_ptr()) };
    }
}
