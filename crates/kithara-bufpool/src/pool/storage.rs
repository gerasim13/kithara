pub(crate) trait Storage: Default {
    fn bytes_for_capacity(capacity: usize) -> Option<usize>;
    fn capacity(&self) -> usize;
    fn clear(&mut self);
    fn move_from(&mut self, other: &mut Self);
    fn shrink_to(&mut self, min_capacity: usize);
    fn try_with_capacity(capacity: usize) -> Result<Self, ()>;
}

impl<T> Storage for Vec<T> {
    fn bytes_for_capacity(capacity: usize) -> Option<usize> {
        capacity.checked_mul(size_of::<T>())
    }

    fn try_with_capacity(capacity: usize) -> Result<Self, ()> {
        let mut value = Self::new();
        value.try_reserve_exact(capacity).map_err(|_| ())?;
        Ok(value)
    }

    delegate::delegate! {
        to self {
            fn capacity(&self) -> usize;
            fn clear(&mut self);
            #[call(append)]
            fn move_from(&mut self, other: &mut Self);
            fn shrink_to(&mut self, min_capacity: usize);
        }
    }
}

impl Storage for String {
    fn bytes_for_capacity(capacity: usize) -> Option<usize> {
        Some(capacity)
    }

    fn move_from(&mut self, other: &mut Self) {
        self.push_str(other);
        other.clear();
    }

    fn try_with_capacity(capacity: usize) -> Result<Self, ()> {
        let mut value = Self::new();
        value.try_reserve_exact(capacity).map_err(|_| ())?;
        Ok(value)
    }

    delegate::delegate! {
        to self {
            fn capacity(&self) -> usize;
            fn clear(&mut self);
            fn shrink_to(&mut self, min_capacity: usize);
        }
    }
}
