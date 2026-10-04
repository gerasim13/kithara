use std::{collections::HashMap, mem, num::NonZeroU32};

use kithara_bufpool::HasPool;
use kithara_command::Live;
use kithara_platform::sync::{Arc, Mutex};
use kithara_play::{PlayError, player::PlayerControlSource};
use kithara_sync::{
    GroupState, SyncAdmission, SyncCapability, SyncError, SyncOperation, SyncRejected,
};
use kithara_warp::BeatGridId;

use super::super::{HeldPlayer, Host, HostOwned, owner::SessionRuntime};
use crate::{
    HostSettings, PlayerMember,
    rt::SessionOutput,
    session::{HostDispatcher, HostProtocol, RootView, web::WebSessionState},
    wasm::HostRoute,
};
type Resident = Box<dyn FnMut() -> Result<(), PlayError>>;
type StartedPlatform<S> = (Arc<dyn HostDispatcher<S>>, Platform<S>);

pub(in crate::host) struct Platform<S> {
    remote_routes: Mutex<Vec<Arc<HostRoute<S>>>>,
    remote_residents: Option<HashMap<BeatGridId, Resident>>,
    web_state: Option<WebSessionState<S>>,
}

impl<S> Platform<S> {
    pub(in crate::host) fn close(platform: &mut Self, host_id: BeatGridId) {
        for route in mem::take(&mut *platform.remote_routes.lock()) {
            route.close();
        }
        if let Some(residents) = platform.remote_residents.take()
            && !residents.is_empty()
        {
            let resident_count = residents.len();
            for resident in residents.into_values() {
                mem::forget(resident);
            }
            tracing::error!(
                ?host_id,
                resident_count,
                "remote wasm Host dropped before its players detached; retaining residents"
            );
        }
    }

    fn close_resident(&mut self, id: BeatGridId) -> Result<(), PlayError> {
        let resident = self
            .remote_residents
            .as_mut()
            .and_then(|residents| residents.get_mut(&id))
            .ok_or_else(|| PlayError::Internal("attached wasm player lost its owner".into()))?;
        resident()
    }

    fn insert_resident(
        &mut self,
        id: BeatGridId,
        resident: Resident,
    ) -> Result<Option<Resident>, PlayError> {
        self.remote_residents
            .as_mut()
            .ok_or_else(|| {
                PlayError::Internal("wasm Worker resident registry is unavailable".into())
            })
            .map(|residents| residents.insert(id, resident))
    }

    #[cfg(feature = "offline")]
    pub(in crate::host) fn offline() -> Result<Self, PlayError> {
        if kithara_platform::thread::is_main_thread() {
            return Err(PlayError::SessionCategoryUnsupported {
                reason: "offline Host must run in a Web Worker".to_owned(),
            });
        }
        Ok(Self::remote())
    }

    pub(in crate::host) fn owner(web_state: WebSessionState<S>) -> Self {
        Self {
            web_state: Some(web_state),
            remote_routes: Mutex::default(),
            remote_residents: None,
        }
    }

    pub(in crate::host) fn realtime(
        group: GroupState<PlayerMember>,
        view: RootView,
        _output_block_frames: Option<NonZeroU32>,
        output: SessionOutput,
        settings: Live<HostSettings, HostProtocol>,
    ) -> Result<StartedPlatform<S>, PlayError>
    where
        S: HasPool<f32> + Send + Sync + 'static,
    {
        let (dispatcher, web_state) =
            crate::session::web::spawn::<S>(group, view, output, settings)?;
        Ok((dispatcher, Self::owner(web_state)))
    }

    fn release_on_session_gone<T>(
        &mut self,
        id: BeatGridId,
        result: Result<T, PlayError>,
    ) -> Result<T, PlayError> {
        match result {
            Err(error @ PlayError::SessionGone { .. }) => {
                self.release_resident(id)?;
                Err(error)
            }
            result => result,
        }
    }

    fn release_resident(&mut self, id: BeatGridId) -> Result<(), PlayError> {
        let resident = self
            .remote_residents
            .as_mut()
            .and_then(|residents| residents.remove(&id))
            .ok_or_else(|| {
                PlayError::Internal("detached wasm player lost its Worker resident".into())
            })?;
        drop(resident);
        Ok(())
    }

    fn remote() -> Self {
        Self {
            web_state: None,
            remote_routes: Mutex::default(),
            remote_residents: Some(HashMap::new()),
        }
    }

    fn require_remote(&self) -> Result<(), PlayError> {
        if self.remote_residents.is_some() {
            return Ok(());
        }
        Err(PlayError::Internal(
            "wasm players must be inserted from their owning Worker".into(),
        ))
    }

    pub(in crate::host) fn transact(
        _platform: &Self,
        dispatcher: &Arc<dyn HostDispatcher<S>>,
        operation: SyncOperation<PlayerMember>,
    ) -> Result<SyncAdmission, SyncRejected<PlayerMember>> {
        if matches!(&operation, SyncOperation::Topology { .. }) {
            return Err(SyncRejected::new(
                SyncError::CapabilityUnavailable {
                    capability: SyncCapability::Topology,
                },
                operation,
            ));
        }
        dispatcher.transact(operation)
    }
}

impl<S> Host<S>
where
    S: HasPool<f32> + Send + Sync + 'static,
{
    /// Attaches and transfers one fully configured player or decorator into
    /// this Host, then prepares its graph and initial slot before returning.
    /// Audio-device setup may block; musical playback remains stopped.
    ///
    /// # Errors
    /// Returns an error when binding, attachment, or graph preparation fails.
    pub fn insert<P>(&mut self, mut player: P) -> Result<HostOwned<P>, PlayError>
    where
        P: PlayerControlSource<Schema = S>,
    {
        self.session.platform().require_remote()?;
        let (attachment, control) = self.bind_player(&mut player)?;
        let grid_id = attachment.id();
        self.attach_member(PlayerMember::new(
            attachment,
            HeldPlayer::new(player.host_level()),
        ))?;
        let resident: Resident = Box::new(move || player.close());
        if let Some(replaced) = self
            .session
            .platform_mut()
            .insert_resident(grid_id, resident)?
        {
            mem::forget(replaced);
            return Err(PlayError::Internal(
                "wasm player residence changed during insertion".into(),
            ));
        }
        let owned = self.owned::<P>(grid_id, control);
        if let Err(error) = P::prepare_control(owned.control()) {
            self.remove(&owned)?;
            return Err(error);
        }
        Ok(owned)
    }

    pub(crate) fn register_remote_route(&self, route: Arc<HostRoute<S>>) {
        self.session.platform().remote_routes.lock().push(route);
    }

    pub(crate) fn remote(
        id: BeatGridId,
        root_view: RootView,
        dispatcher: Arc<dyn HostDispatcher<S>>,
    ) -> Self {
        Self {
            id,
            root_view,
            dispatcher,
            owns_session: false,
            session: SessionRuntime::realtime(Platform::remote()),
        }
    }

    pub(crate) fn remote_identity(&self) -> (BeatGridId, RootView) {
        (self.id, self.root_view.clone())
    }

    /// Closes the lower runtime on the caller thread, then detaches its
    /// canonical member after graph unregistration has completed.
    ///
    /// # Errors
    /// Returns an error when close or canonical detachment fails.
    pub fn remove<P>(&mut self, player: &HostOwned<P>) -> Result<(), PlayError>
    where
        P: PlayerControlSource<Schema = S>,
    {
        self.validate_removal(player)?;
        self.remove_resident(player.id())
    }

    fn remove_resident(&mut self, id: BeatGridId) -> Result<(), PlayError> {
        let close_result = self.session.platform_mut().close_resident(id);
        self.session
            .platform_mut()
            .release_on_session_gone(id, close_result)?;
        let detach_result = self.detach_member(id);
        self.session
            .platform_mut()
            .release_on_session_gone(id, detach_result)?;
        self.session.platform_mut().release_resident(id)
    }

    pub(crate) fn web_state(&self) -> Option<&WebSessionState<S>> {
        self.session.platform().web_state.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, num::NonZeroU32, rc::Rc};

    use delegate::delegate;
    use kithara_audio::ConsumerWakeMode;
    use kithara_platform::sync::Arc;
    use kithara_play::{PlayError, SessionDispatcher};
    use kithara_sync::{
        GroupState, SyncAdmission, SyncGroup, SyncMember, SyncOperation, TopologyOperation,
    };
    use kithara_test_utils::{bufpool::TestPools, kithara};
    use kithara_warp::BeatGridId;

    use super::{Host, Platform, Resident, SessionRuntime};
    use crate::{
        PlayerMember,
        host::owner::SessionRoot,
        session::{
            HostCmd, HostDispatcher, HostReply, Reply,
            protocol::{HostDispatchError, SyncCmd},
            tests::graph::{FixtureSession, fixture_member},
        },
    };

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Outcome {
        Ok,
        SessionGone,
        OtherError,
    }

    struct ResidentProbe {
        close: Outcome,
        drops: Rc<RefCell<usize>>,
    }

    impl Drop for ResidentProbe {
        fn drop(&mut self) {
            *self.drops.borrow_mut() += 1;
        }
    }

    impl ResidentProbe {
        fn close(&mut self) -> Result<(), PlayError> {
            match self.close {
                Outcome::Ok => Ok(()),
                Outcome::SessionGone => Err(PlayError::SessionGone {
                    reason: "fixture resident close",
                }),
                Outcome::OtherError => {
                    Err(PlayError::Internal("fixture resident close failed".into()))
                }
            }
        }
    }

    fn resident(close: Outcome, drops: Rc<RefCell<usize>>) -> Resident {
        let mut probe = ResidentProbe { close, drops };
        Box::new(move || probe.close())
    }

    struct Dispatcher {
        session: FixtureSession,
        detach: Outcome,
        root: RefCell<GroupState<PlayerMember>>,
    }

    impl SessionDispatcher<TestPools> for Dispatcher {
        delegate! {
            to &self.session {
                #[through(SessionDispatcher::<TestPools>)]
                fn exec(&self, cmd: kithara_play::Cmd<TestPools>) -> Result<Reply, PlayError>;
                #[through(SessionDispatcher::<TestPools>)]
                fn consumer_wake_mode(&self) -> ConsumerWakeMode;
            }
        }
    }

    impl HostDispatcher<TestPools> for Dispatcher {
        fn exec_host(
            &self,
            cmd: HostCmd<TestPools>,
        ) -> Result<HostReply, HostDispatchError<TestPools>> {
            let HostCmd::Sync(SyncCmd::TransactCurrent(operations)) = cmd else {
                panic!("unexpected fixture Host command")
            };
            match self.detach {
                Outcome::SessionGone => Err(HostDispatchError::before_send(
                    PlayError::SessionGone {
                        reason: "fixture detach",
                    },
                    HostCmd::Sync(SyncCmd::TransactCurrent(operations)),
                )),
                Outcome::OtherError => Ok(HostReply::Err(PlayError::Internal(
                    "fixture detach failed".into(),
                ))),
                Outcome::Ok => {
                    let mut root = self.root.borrow_mut();
                    let base = root.topology().expect("fixture topology").stamp();
                    Ok(HostReply::Admission(
                        root.transact(SyncOperation::Topology { base, operations }),
                    ))
                }
            }
        }
    }

    fn fixture(
        close: Outcome,
        detach: Outcome,
    ) -> (Host<TestPools>, BeatGridId, Rc<RefCell<usize>>) {
        let sample_rate = NonZeroU32::new(44_100).expect("fixture sample rate");
        let SessionRoot {
            id: host_id,
            group: mut root,
            view: root_view,
        } = Host::<TestPools>::session_root(sample_rate).expect("fixture Host session");
        let resident_id = BeatGridId::allocate().expect("fixture resident grid id");
        let base = root.topology().expect("fixture root topology").stamp();
        let admission = root
            .transact(SyncOperation::Topology {
                base,
                operations: Box::new([TopologyOperation::Attach {
                    member: SyncMember::Group {
                        alignment: None,
                        group: Box::new(fixture_member(resident_id, sample_rate)),
                    },
                }]),
            })
            .expect("fixture resident attachment");
        assert!(matches!(admission, SyncAdmission::TopologyChanged { .. }));

        let dispatcher: Arc<dyn HostDispatcher<TestPools>> = Arc::new(Dispatcher {
            detach,
            session: FixtureSession,
            root: RefCell::new(root),
        });
        let drops = Rc::new(RefCell::new(0));
        let mut platform = Platform::remote();
        let replaced = platform
            .insert_resident(resident_id, resident(close, Rc::clone(&drops)))
            .expect("fixture resident registry");
        assert!(replaced.is_none());
        let host = Host {
            root_view,
            dispatcher,
            id: host_id,
            owns_session: false,
            session: SessionRuntime::realtime(platform),
        };
        (host, resident_id, drops)
    }

    #[kithara::test(wasm, flash(false))]
    fn successful_remove_releases_resident() {
        let (mut host, resident, drops) = fixture(Outcome::Ok, Outcome::Ok);

        host.remove_resident(resident).expect("remove resident");

        assert_eq!(*drops.borrow(), 1);
    }

    #[kithara::test(wasm, flash(false))]
    #[case::closing(Outcome::SessionGone, Outcome::Ok)]
    #[case::detaching(Outcome::Ok, Outcome::SessionGone)]
    fn session_gone_releases_resident(#[case] close: Outcome, #[case] detach: Outcome) {
        let (mut host, resident, drops) = fixture(close, detach);

        assert!(matches!(
            host.remove_resident(resident),
            Err(PlayError::SessionGone { .. })
        ));
        assert_eq!(*drops.borrow(), 1);
    }

    #[kithara::test(wasm, flash(false))]
    fn other_errors_retain_resident() {
        for (close, detach) in [
            (Outcome::OtherError, Outcome::Ok),
            (Outcome::Ok, Outcome::OtherError),
        ] {
            let (mut host, resident, drops) = fixture(close, detach);

            assert!(matches!(
                host.remove_resident(resident),
                Err(PlayError::Internal(_))
            ));
            assert_eq!(*drops.borrow(), 0);
            assert!(
                host.session
                    .platform()
                    .remote_residents
                    .as_ref()
                    .is_some_and(|residents| residents.contains_key(&resident))
            );
            drop(host);
            assert_eq!(*drops.borrow(), 0);
        }
    }
}
