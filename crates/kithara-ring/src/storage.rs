use std::{mem::MaybeUninit, ops::DerefMut, ptr::NonNull};

use ringbuf::storage::Storage;

/// Ring storage that keeps an owner alive and addresses its slice in place.
///
/// The owner sits behind its own allocation and is not touched again until
/// the storage drops, so the slot pointer taken at construction stays valid
/// whether the slice lives inside the owner or on the heap.
pub struct OwnedSlice<B> {
    owner: NonNull<B>,
    slots: NonNull<u8>,
    len: usize,
}

impl<B, T> OwnedSlice<B>
where
    B: DerefMut<Target = [T]>,
    T: Copy,
{
    pub(crate) fn new(owner: B) -> Result<Self, B> {
        if owner.is_empty() {
            return Err(owner);
        }
        let owner = Box::into_raw(Box::new(owner));
        // SAFETY: `owner` comes from `Box::into_raw` above, so it is valid,
        // aligned, and not reachable through any other pointer.
        let slots = unsafe { (*owner).deref_mut() };
        let len = slots.len();
        if len == 0 {
            // SAFETY: `owner` still comes from `Box::into_raw` and is
            // reclaimed exactly once, here.
            return Err(*unsafe { Box::from_raw(owner) });
        }
        Ok(Self {
            len,
            slots: NonNull::from(slots).cast(),
            // SAFETY: `Box::into_raw` never returns null.
            owner: unsafe { NonNull::new_unchecked(owner) },
        })
    }
}

// SAFETY: `slots` points at `len` contiguous `T` owned by `owner`, which is
// never accessed until drop, so the pointer and the length stay fixed and the
// storage does not alias the slot memory it hands out.
unsafe impl<B, T> Storage for OwnedSlice<B>
where
    B: DerefMut<Target = [T]>,
    T: Copy,
{
    type Item = T;

    fn len(&self) -> usize {
        self.len
    }

    fn as_mut_ptr(&self) -> *mut MaybeUninit<T> {
        self.slots.cast().as_ptr()
    }
}

// SAFETY: moving the storage moves the owner, which is `Send`; slot values are
// `Send` and cross threads only through the ring.
unsafe impl<B, T> Send for OwnedSlice<B>
where
    B: DerefMut<Target = [T]> + Send,
    T: Send,
{
}

// SAFETY: shared access reaches only the slots, and the ring's
// single-producer single-consumer protocol keeps the halves on disjoint
// ranges; the owner itself is never shared.
unsafe impl<B, T> Sync for OwnedSlice<B>
where
    B: DerefMut<Target = [T]> + Send,
    T: Send,
{
}

impl<B> Drop for OwnedSlice<B> {
    fn drop(&mut self) {
        // SAFETY: `owner` comes from `Box::into_raw` in `new` and is reclaimed
        // exactly once, here.
        drop(unsafe { Box::from_raw(self.owner.as_ptr()) });
    }
}
