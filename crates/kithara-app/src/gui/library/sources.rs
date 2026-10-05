use std::collections::BTreeMap;

use kithara::{
    net::HttpClient,
    platform::{CancelToken, tokio::runtime::Handle},
};
use kithara_app_library::{Context, Factory, Registration, SectionError};
use serde_yaml_ng::Value;

pub(in crate::gui) use self::consts::FACTORIES;

mod consts {
    use kithara::net::HttpClient;
    use kithara_app_library::Factory;

    /// Every library source this build can mount.
    pub(in crate::gui) const FACTORIES: &[Factory<HttpClient>] = &[
        #[cfg(feature = "zvuk")]
        kithara_app_zvuk::Source::FACTORY,
    ];
}

/// The sources of `factories` the document configures: each one whose entry
/// is present and not null, built over the shared client with its own
/// cancellation.
pub(in crate::gui) fn configured(
    factories: &[Factory<HttpClient>],
    sections: &BTreeMap<String, Value>,
    net: &HttpClient,
    runtime: &Handle,
    shutdown: &CancelToken,
) -> Result<Vec<Registration>, SectionError> {
    factories
        .iter()
        .filter_map(|factory| {
            let section = sections
                .get(factory.id)
                .filter(|section| !section.is_null())?;
            Some((factory.register)(Context::new(
                factory.id,
                net.clone(),
                runtime.clone(),
                shutdown.child(),
                section.clone(),
            )))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use kithara_test_utils::{cancel_token, kithara};

    use super::*;
    use crate::gui::{library::StartupSource, test_fixture};

    thread_local! {
        static REGISTERED: RefCell<Vec<CancelToken>> = const { RefCell::new(Vec::new()) };
    }

    fn register(context: Context<HttpClient>) -> Result<Registration, SectionError> {
        context.section::<BTreeMap<String, String>>()?;
        REGISTERED.with_borrow_mut(|registered| registered.push(context.cancel));
        Ok(StartupSource::registered(Vec::new()))
    }

    #[kithara::test]
    fn configured_entries_register_with_cancellations_of_their_own(cancel_token: CancelToken) {
        let runtime = test_fixture::runtime();
        let net = test_fixture::config().net;
        let entry = |yaml| serde_yaml_ng::from_str::<Value>(yaml).expect("entry parses");
        let sections = BTreeMap::from([
            ("first".to_owned(), entry("name: one")),
            ("second".to_owned(), entry("name: two")),
            ("null".to_owned(), Value::Null),
        ]);
        let mismatched = BTreeMap::from([("first".to_owned(), entry("one"))]);
        let probes = ["first", "second", "null", "absent"].map(|id| Factory { id, register });

        let registered = configured(&probes, &sections, &net, runtime.handle(), &cancel_token)
            .expect("both entries match the schema");
        let refused = configured(&probes, &mismatched, &net, runtime.handle(), &cancel_token);

        assert_eq!(registered.len(), 2);
        assert!(refused.is_err(), "a mismatched entry is refused");
        let [first, second] = REGISTERED
            .take()
            .try_into()
            .expect("two sources registered");
        first.cancel();
        assert!(!second.is_cancelled() && !cancel_token.is_cancelled());
        cancel_token.cancel();
        assert!(second.is_cancelled());
    }
}
