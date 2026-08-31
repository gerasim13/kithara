use kithara_platform::sync::{Arc, Mutex};
use kithara_warp::BeatGridId;

use super::Host;
use crate::session::{HostDispatcher, RootView, web::WebSessionState};

impl<S> Host<S> {
    pub(crate) fn remote(
        id: BeatGridId,
        root_view: RootView,
        dispatcher: Arc<dyn HostDispatcher<S>>,
    ) -> Self {
        Self {
            id,
            owns_session: false,
            root_view,
            dispatcher,
            web_state: None,
            remote_routes: Mutex::default(),
        }
    }

    pub(crate) fn remote_identity(&self) -> (BeatGridId, RootView) {
        (self.id, self.root_view.clone())
    }

    pub(crate) fn register_remote_route(&self, route: Arc<crate::wasm::HostRoute<S>>) {
        self.remote_routes.lock().push(route);
    }

    pub(crate) fn web_state(&self) -> Option<&WebSessionState<S>> {
        self.web_state.as_ref()
    }
}
