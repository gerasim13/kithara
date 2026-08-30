use crate::{PoolKey, region::PoolSlot};

/// Compile-time evidence that a schema contains `K`.
pub trait HasPool<K>
where
    K: PoolKey,
{
    /// Return the generated opaque slot for static facade dispatch.
    #[doc(hidden)]
    fn __slot(&self) -> &PoolSlot<K>;
}

/// Declare a closed set of explicitly configured typed pools.
///
/// Every generated setter is required. Bon's typestate builder makes missing,
/// duplicate, and unknown registrations compile-time errors.
///
/// ```
/// use kithara_bufpool::{OverallBudget, PoolConfig, pool_schema};
///
/// pool_schema! {
///     pub ExamplePools {
///         bytes: u8,
///         samples: f32,
///     }
/// }
///
/// let config = || PoolConfig::builder().max_buffers(32).build();
/// let pools = ExamplePools::builder(OverallBudget(1024))
///     .bytes(config())
///     .samples(config())
///     .build()
///     .unwrap();
/// assert_eq!(pools.get::<u8>().len(), 0);
/// assert_eq!(pools.get_with_len::<f32>(4).unwrap().len(), 4);
/// ```
///
/// ```compile_fail
/// use kithara_bufpool::{OverallBudget, PoolConfig, pool_schema};
/// pool_schema! { pub MissingPools { bytes: u8, samples: f32 } }
/// let config = PoolConfig::builder().max_buffers(8).build();
/// let _ = MissingPools::builder(OverallBudget(64)).bytes(config).build();
/// ```
///
/// ```compile_fail
/// use kithara_bufpool::{OverallBudget, PoolConfig, pool_schema};
/// pool_schema! { pub DuplicatePools { bytes: u8 } }
/// let config = || PoolConfig::builder().max_buffers(8).build();
/// let _ = DuplicatePools::builder(OverallBudget(64))
///     .bytes(config())
///     .bytes(config());
/// ```
///
/// ```compile_fail
/// use kithara_bufpool::{OverallBudget, PoolConfig, pool_schema};
/// pool_schema! { pub KnownPools { bytes: u8 } }
/// let config = PoolConfig::builder().max_buffers(8).build();
/// let _ = KnownPools::builder(OverallBudget(64)).samples(config);
/// ```
///
/// ```compile_fail
/// use kithara_bufpool::{OverallBudget, PoolConfig, pool_schema};
/// pool_schema! { pub BytePools { bytes: u8 } }
/// let pools = BytePools::builder(OverallBudget(64))
///     .bytes(PoolConfig::builder().max_buffers(8).build())
///     .build()
///     .unwrap();
/// let _ = pools.get::<f32>();
/// ```
#[macro_export]
macro_rules! pool_schema {
    (
        $(#[$meta:meta])*
        $vis:vis $name:ident {
            $($field:ident: $key:ty),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        $vis struct $name {
            $(
                $field: $crate::__private::PoolSlot<$key>,
            )+
        }

        #[$crate::__private::bon::bon(
            crate = $crate::__private::bon
        )]
        impl $name {
            #[builder(
                builder_type(vis = ""),
                start_fn(name = builder, vis = ""),
                finish_fn(name = build, vis = "")
            )]
            #[doc(hidden)]
            fn __build_region(
                #[builder(start_fn)]
                overall_budget: $crate::OverallBudget,
                $(
                    $field: $crate::PoolConfig,
                )+
            ) -> Result<$crate::PoolRegion<Self>, $crate::PoolError> {
                $crate::PoolRegion::__build(overall_budget, |context| {
                    $(
                        let $field = context.slot::<$key>($field)?;
                    )+
                    Ok(Self {
                        $($field),+
                    })
                })
            }
        }

        $(
            impl $crate::HasPool<$key> for $name {
                fn __slot(&self) -> &$crate::__private::PoolSlot<$key> {
                    &self.$field
                }
            }
        )+
    };
}
