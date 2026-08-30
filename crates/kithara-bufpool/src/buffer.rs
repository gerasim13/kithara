mod byte;
mod owned;
mod sample;

pub use byte::ByteBuffer;
pub(crate) use owned::OwnedBuffer;
pub use sample::SampleBuffer;

macro_rules! impl_buffer_traits {
    ($buffer:ty, $item:ty) => {
        impl std::ops::Deref for $buffer {
            type Target = [$item];

            fn deref(&self) -> &Self::Target {
                &self.0.value
            }
        }

        impl std::ops::DerefMut for $buffer {
            fn deref_mut(&mut self) -> &mut Self::Target {
                &mut self.0.value
            }
        }

        impl std::fmt::Debug for $buffer {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.value.fmt(f)
            }
        }
    };
}

impl_buffer_traits!(ByteBuffer, u8);
impl_buffer_traits!(SampleBuffer, f32);
