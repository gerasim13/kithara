/// Failure to construct a pool region or grow one of its buffers.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum PoolError {
    /// The region-wide hard byte limit would be exceeded.
    #[error(
        "buffer region budget exceeded: requested {additional_bytes} additional bytes with \
         {allocated_bytes} of {max_bytes} already tracked"
    )]
    OverallBudgetExceeded {
        /// Additional bytes requested by the operation.
        additional_bytes: usize,
        /// Bytes tracked when the request was rejected.
        allocated_bytes: usize,
        /// Region-wide hard byte limit.
        max_bytes: usize,
    },
    /// The selected pool's hard byte limit would be exceeded.
    #[error(
        "typed pool budget exceeded: requested {additional_bytes} additional bytes with \
         {allocated_bytes} of {max_bytes} already tracked"
    )]
    PoolBudgetExceeded {
        /// Additional bytes requested by the operation.
        additional_bytes: usize,
        /// Bytes tracked by the selected pool when rejected.
        allocated_bytes: usize,
        /// Selected pool's hard byte limit.
        max_bytes: usize,
    },
    /// The allocator rejected the requested capacity.
    #[error(
        "buffer allocation failed: requested {additional_bytes} additional bytes with \
         {allocated_bytes} of {max_bytes} region bytes tracked"
    )]
    AllocationFailed {
        /// Additional bytes requested by the operation.
        additional_bytes: usize,
        /// Region bytes tracked while the allocation was attempted.
        allocated_bytes: usize,
        /// Region-wide hard byte limit.
        max_bytes: usize,
    },
    /// An element count cannot be represented as a byte capacity.
    #[error("buffer capacity overflows usize: {elements} elements of {element_size} bytes")]
    CapacityOverflow {
        /// Requested element count.
        elements: usize,
        /// Size of one element in bytes.
        element_size: usize,
    },
    /// A pool configuration cannot satisfy its declared policy.
    #[error("invalid pool configuration for {field}: {reason}")]
    InvalidConfig {
        /// Invalid configuration field.
        field: &'static str,
        /// Stable explanation of the invalid value or combination.
        reason: &'static str,
    },
}
