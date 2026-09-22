use kithara_config::Config as _;
use kithara_test_utils::kithara;

mod private {
    #[kithara_config::config(default)]
    pub(crate) struct Timing {
        /// Frame distance between events.
        #[config(value)]
        #[builder(default = 3)]
        #[field(get(copy))]
        width: usize,
        #[cfg_attr(all(), cfg(any()))]
        #[config(value)]
        unavailable: TypeUnavailableInThisProfile,
    }

    #[kithara_config::config]
    pub(crate) struct Session<'a, T, const N: usize>
    where
        T: core::fmt::Debug,
    {
        #[config(skip = "construction resource")]
        #[field(get)]
        resource: T,
        #[config(skip = "borrowed preparation storage")]
        #[field(get)]
        storage: &'a mut [u8; N],
        /// Owned display label.
        #[config(value(String, self.label.to_owned()))]
        label: &'a str,
        #[config(nested)]
        timing: Timing,
    }
}

#[kithara::test(native, flash(false))]
fn external_consumer_reads_private_nested_values_without_cloning_resources() {
    #[derive(Debug)]
    struct Resource;
    let mut storage = [0; 4];
    let timing = private::Timing::default();
    assert_eq!(timing.width(), 3);
    let config = private::Session::builder()
        .resource(Resource)
        .storage(&mut storage)
        .label("input")
        .timing(timing)
        .build();
    let snapshot = config.values();
    assert_eq!(snapshot.label, "input");
    assert_eq!(snapshot.timing.width, 3);
    assert_eq!(config.storage().len(), 4);
    assert_eq!(format!("{:?}", config.resource()), "Resource");
    drop(config);
    assert_eq!(snapshot.label, "input");
}

struct Prepared {
    doubled: u32,
}

#[kithara_config::config]
impl Prepared {
    #[builder(start_fn = builder, finish_fn = build)]
    fn new(input: u32) -> Result<Self, &'static str> {
        let doubled = input.checked_mul(2).ok_or("overflow")?;
        Ok(Self { doubled })
    }
}

#[kithara_config::config]
fn length(input: &str) -> usize {
    input.len()
}

#[kithara::test(native, flash(false))]
fn wrapped_function_builders_preserve_preparation_and_errors() {
    assert_eq!(Prepared::builder().input(21).build().unwrap().doubled, 42);
    assert!(Prepared::builder().input(u32::MAX).build().is_err());
    assert_eq!(length().input("abc").call(), 3);
}
