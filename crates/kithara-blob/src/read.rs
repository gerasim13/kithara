use std::str;

use super::{BlobError, MAX_PREALLOC};

/// Little-endian cursor reader over a byte slice.
pub struct Reader<'a> {
    bytes: &'a [u8],
    cursor: usize,
}

impl<'a> Reader<'a> {
    #[must_use]
    pub const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, cursor: 0 }
    }

    /// Succeed only if the whole blob was consumed.
    ///
    /// # Errors
    ///
    /// Errors if any byte is left unread.
    pub const fn finish(&self) -> Result<(), BlobError> {
        if self.cursor == self.bytes.len() {
            Ok(())
        } else {
            Err(BlobError::Corrupt)
        }
    }

    /// # Errors
    ///
    /// Errors if fewer than `N` bytes are left.
    pub fn read_array<const N: usize>(&mut self) -> Result<[u8; N], BlobError> {
        let end = self.cursor.checked_add(N).ok_or(BlobError::Corrupt)?;
        let chunk = self.bytes.get(self.cursor..end).ok_or(BlobError::Corrupt)?;
        let mut out = [0u8; N];
        out.copy_from_slice(chunk);
        self.cursor = end;
        Ok(out)
    }

    /// # Errors
    ///
    /// Errors if the byte is neither zero nor one.
    pub fn read_bool(&mut self) -> Result<bool, BlobError> {
        match self.read_u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(BlobError::Corrupt),
        }
    }

    /// # Errors
    ///
    /// Errors if fewer than four bytes are left.
    pub fn read_f32(&mut self) -> Result<f32, BlobError> {
        Ok(f32::from_le_bytes(self.read_array::<4>()?))
    }

    /// # Errors
    ///
    /// Errors if fewer than eight bytes are left.
    pub fn read_f64(&mut self) -> Result<f64, BlobError> {
        Ok(f64::from_le_bytes(self.read_array::<8>()?))
    }

    /// Read a `u64` length prefix as a `usize`.
    ///
    /// # Errors
    ///
    /// Errors if the blob ends early or the length exceeds `usize`.
    pub fn read_len(&mut self) -> Result<usize, BlobError> {
        usize::try_from(self.read_u64()?).map_err(|_| BlobError::Corrupt)
    }

    /// # Errors
    ///
    /// Errors if the blob ends early or the presence flag is not a boolean.
    pub fn read_optional_u64(&mut self) -> Result<Option<u64>, BlobError> {
        let present = self.read_bool()?;
        let value = self.read_u64()?;
        Ok(present.then_some(value))
    }

    /// # Errors
    ///
    /// Errors if the blob ends before the section it announces.
    pub fn read_section(&mut self) -> Result<&'a [u8], BlobError> {
        let len = self.read_len()?;
        self.read_slice(len)
    }

    fn read_slice(&mut self, len: usize) -> Result<&'a [u8], BlobError> {
        let end = self.cursor.checked_add(len).ok_or(BlobError::Corrupt)?;
        let slice = self.bytes.get(self.cursor..end).ok_or(BlobError::Corrupt)?;
        self.cursor = end;
        Ok(slice)
    }

    /// # Errors
    ///
    /// Errors if the blob ends early or the bytes are not UTF-8.
    pub fn read_str(&mut self) -> Result<String, BlobError> {
        let len = usize::try_from(self.read_u32()?).map_err(|_| BlobError::Corrupt)?;
        let raw = self.read_slice(len)?;
        str::from_utf8(raw)
            .map(str::to_owned)
            .map_err(|_| BlobError::Corrupt)
    }

    /// # Errors
    ///
    /// Errors if fewer than four bytes are left.
    pub fn read_u32(&mut self) -> Result<u32, BlobError> {
        Ok(u32::from_le_bytes(self.read_array::<4>()?))
    }

    /// # Errors
    ///
    /// Errors if fewer than eight bytes are left.
    pub fn read_u64(&mut self) -> Result<u64, BlobError> {
        Ok(u64::from_le_bytes(self.read_array::<8>()?))
    }

    /// # Errors
    ///
    /// Errors if no byte is left.
    pub fn read_u8(&mut self) -> Result<u8, BlobError> {
        let value = self
            .bytes
            .get(self.cursor)
            .copied()
            .ok_or(BlobError::Corrupt)?;
        self.cursor += 1;
        Ok(value)
    }

    /// Read a length prefix, rejecting a count whose fixed-size items cannot
    /// fit in what is left. This is the bound that keeps a corrupt length from
    /// driving an unbounded read loop.
    ///
    /// # Errors
    ///
    /// Errors if the blob ends early or the count cannot fit in what is left.
    pub fn read_count(&mut self, item_bytes: usize) -> Result<usize, BlobError> {
        let count = self.read_len()?;
        if count.saturating_mul(item_bytes) > self.remaining() {
            return Err(BlobError::Corrupt);
        }
        Ok(count)
    }

    /// Read a length-prefixed `f32` series.
    ///
    /// # Errors
    ///
    /// Errors if the blob ends early or the count exceeds what is left.
    pub fn read_samples(&mut self) -> Result<Box<[f32]>, BlobError> {
        let count = self.read_count(size_of::<f32>())?;
        (0..count).map(|_| self.read_f32()).collect()
    }

    /// Read a `u64` that must be strictly greater than the previous one.
    ///
    /// # Errors
    ///
    /// Errors if the blob ends early or the value does not exceed `previous`.
    pub fn read_ordered(&mut self, previous: Option<u64>) -> Result<u64, BlobError> {
        let value = self.read_u64()?;
        if previous.is_some_and(|previous| previous >= value) {
            Err(BlobError::Corrupt)
        } else {
            Ok(value)
        }
    }

    /// Preallocate for `count` items without trusting an untrusted length.
    #[must_use]
    pub fn capacity_for(count: usize) -> usize {
        count.min(MAX_PREALLOC)
    }

    /// Bytes not yet consumed.
    #[must_use]
    pub const fn remaining(&self) -> usize {
        self.bytes.len() - self.cursor
    }
}
