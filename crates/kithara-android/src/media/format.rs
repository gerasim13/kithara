use std::{
    ffi::{CStr, c_void},
    ptr::NonNull,
};

use derive_more::From;

use super::sys;
use crate::error::AndroidBackendError;

#[derive(From)]
pub struct OwnedFormat {
    raw: NonNull<sys::AMediaFormat>,
}

impl OwnedFormat {
    /// Allocate an empty format.
    ///
    /// # Errors
    ///
    /// Returns [`AndroidBackendError::Operation`] when the platform refuses
    /// the allocation.
    pub fn new() -> Result<Self, AndroidBackendError> {
        // SAFETY: AMediaFormat_new returns a freshly allocated AMediaFormat
        let raw = NonNull::new(unsafe { sys::AMediaFormat_new() })
            .ok_or_else(|| AndroidBackendError::operation("media-format-new", "returned null"))?;
        Ok(Self { raw })
    }

    /// Borrow a byte-buffer property, valid until the format drops.
    #[must_use]
    pub fn get_buffer(&self, key: &CStr) -> Option<&[u8]> {
        let mut data = std::ptr::null_mut();
        let mut size = 0;
        // SAFETY: the format and key are live; both out-parameters are writable.
        let found =
            unsafe { sys::AMediaFormat_getBuffer(self.raw(), key.as_ptr(), &mut data, &mut size) };
        if !found || data.is_null() || size == 0 {
            return None;
        }
        // SAFETY: the successful query returned `size` readable bytes owned by
        // this format.
        Some(unsafe { std::slice::from_raw_parts(data.cast::<u8>(), size) })
    }

    #[must_use]
    pub fn get_i32(&self, key: &CStr) -> Option<i32> {
        let mut value = 0;
        // SAFETY: `raw` is live; `key` is NUL-terminated and `value` is an out-param.
        unsafe { sys::AMediaFormat_getInt32(self.raw(), key.as_ptr(), &mut value) }.then_some(value)
    }

    #[must_use]
    pub fn get_i64(&self, key: &CStr) -> Option<i64> {
        let mut value = 0;
        // SAFETY: `raw` is live; `key` is NUL-terminated and `value` is an out-param.
        unsafe { sys::AMediaFormat_getInt64(self.raw(), key.as_ptr(), &mut value) }.then_some(value)
    }

    #[must_use]
    pub fn get_str(&self, key: &CStr) -> Option<&CStr> {
        let mut value = std::ptr::null();
        // SAFETY: the format and key are live; value is a writable out-parameter.
        let found = unsafe { sys::AMediaFormat_getString(self.raw(), key.as_ptr(), &mut value) };
        if !found || value.is_null() {
            return None;
        }
        // SAFETY: the successful query returns a NUL-terminated string owned by this format.
        Some(unsafe { CStr::from_ptr(value) })
    }

    /// # Errors
    ///
    /// Returns [`AndroidBackendError::Operation`] when the stored value does
    /// not fit the requested width.
    pub fn get_u16(&self, key: &CStr) -> Result<Option<u16>, AndroidBackendError> {
        self.get_uint(key, "media-format-u16")
    }

    /// # Errors
    ///
    /// Returns [`AndroidBackendError::Operation`] when the stored value does
    /// not fit the requested width.
    pub fn get_u32(&self, key: &CStr) -> Result<Option<u32>, AndroidBackendError> {
        self.get_uint(key, "media-format-u32")
    }

    fn get_uint<T>(&self, key: &CStr, op: &'static str) -> Result<Option<T>, AndroidBackendError>
    where
        T: TryFrom<i32>,
    {
        self.get_i32(key)
            .map(|value| {
                T::try_from(value).map_err(|_| {
                    AndroidBackendError::operation(
                        op,
                        format!("{}={value} is out of range", key.to_string_lossy()),
                    )
                })
            })
            .transpose()
    }

    pub(crate) fn raw(&self) -> *mut sys::AMediaFormat {
        self.raw.as_ptr()
    }

    pub fn set_buffer(&mut self, key: &CStr, value: &[u8]) {
        // SAFETY: `raw` is live and exclusively borrowed; setBuffer copies the
        // readable bytes and `key` is NUL-terminated.
        unsafe {
            sys::AMediaFormat_setBuffer(
                self.raw(),
                key.as_ptr(),
                value.as_ptr().cast::<c_void>(),
                value.len(),
            );
        }
    }

    pub fn set_i32(&mut self, key: &CStr, value: i32) {
        // SAFETY: `raw` is live and exclusively borrowed; `key` is NUL-terminated.
        unsafe { sys::AMediaFormat_setInt32(self.raw(), key.as_ptr(), value) };
    }

    pub fn set_str(&mut self, key: &CStr, value: &CStr) {
        // SAFETY: `raw` is live and exclusively borrowed; both strings are NUL-terminated.
        unsafe { sys::AMediaFormat_setString(self.raw(), key.as_ptr(), value.as_ptr()) };
    }
}

impl Drop for OwnedFormat {
    fn drop(&mut self) {
        // SAFETY: `raw` is live and freed exactly once, here.
        unsafe { sys::AMediaFormat_delete(self.raw()) };
    }
}
