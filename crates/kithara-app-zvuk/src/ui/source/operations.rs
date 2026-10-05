use kithara_net::Net;
use kithara_platform::{
    CancelToken,
    maybe_send::MaybeSend,
    time::Instant,
    tokio::{self, task},
};

use super::core::{Active, Operation, Source, accept_playlists};
use crate::{
    Error,
    ui::{
        consts,
        request::{Batch, Completed},
    },
};

impl<N: Net + Clone + 'static> Source<N> {
    pub(super) fn start(&mut self) {
        if self.cancel.is_cancelled()
            || self.active.is_some()
            || !self.pending.is_some_and(|due| due <= Instant::now())
        {
            return;
        }
        let Some((mode, query)) = self.catalogue.request() else {
            return;
        };
        self.pending = None;
        let generation = self.generation;
        let cancel = self.cancel.child();
        self.active = Some(Active {
            generation,
            cancel: cancel.clone(),
        });
        let client = self.client.clone();
        self.spawn(
            cancel,
            async move { mode.load(&client, &query).await },
            move |result| Completed::Catalogue(generation, result),
        );
    }

    /// Runs `work` until `cancel` fires and reports the outcome through `done`.
    fn spawn<T, F, D>(&self, cancel: CancelToken, work: F, done: D)
    where
        T: MaybeSend + 'static,
        F: Future<Output = T> + MaybeSend + 'static,
        D: FnOnce(Option<T>) -> Completed + MaybeSend + 'static,
    {
        let found = self.found.clone();
        drop(task::spawn_on(&self.runtime, async move {
            let result = tokio::select! { biased; _ = cancel.cancelled() => None, result = work => Some(result) };
            let _ = found.send(done(result));
        }));
    }

    fn fail(&mut self, operation: Operation, error: &Error) {
        self.faults.set(operation, Some(error.to_string()));
    }

    /// Stops every request of the source until restart; its page shows the
    /// rejection whichever operation met it.
    fn reject_authentication(&mut self) {
        self.cancel.cancel();
        self.pending = None;
        self.active = None;
        self.fail(Operation::Page, &Error::AuthenticationRejected);
    }

    fn accept(&mut self, batch: Batch) {
        let streams = match batch.streams {
            Ok(streams) => {
                self.faults.set(Operation::Page, None);
                streams
            }
            Err(error) => {
                self.fail(Operation::Page, &error);
                Vec::new()
            }
        };
        self.catalogue.accept(batch.page, streams);
    }

    pub(super) fn list_playlists(&mut self, node: &str) {
        if node != consts::PLAYLISTS
            || !self
                .branch
                .children
                .iter()
                .any(|child| child.key == consts::PLAYLISTS && child.unlisted)
            || self.playlists_active
            || self.cancel.is_cancelled()
        {
            return;
        }
        let cancel = self.cancel.child();
        self.playlists_active = true;
        let client = self.client.clone();
        self.spawn(
            cancel,
            async move { client.playlists().await },
            Completed::Playlists,
        );
    }

    pub(super) fn like(&mut self, id: &str) {
        if self.cancel.is_cancelled() {
            return;
        }
        let Some((id, liked)) = self.catalogue.reaction(id) else {
            return;
        };
        if !self.likes.insert(id.clone()) {
            return;
        }
        let cancel = self.cancel.child();
        let client = self.client.clone();
        let track = id.clone();
        self.spawn(
            cancel,
            async move { client.set_liked(&track, liked).await },
            move |result| Completed::Like(id, liked, result),
        );
    }

    pub(super) fn complete(&mut self, completion: Completed) {
        if completion.rejects_authentication() {
            self.reject_authentication();
            return;
        }
        match completion {
            Completed::Catalogue(generation, result) => {
                if self
                    .active
                    .as_ref()
                    .is_some_and(|active| active.generation == generation)
                {
                    self.active = None;
                }
                if generation != self.generation {
                    return;
                }
                match result {
                    Some(Ok(batch)) => self.accept(batch),
                    Some(Err(error)) => self.fail(Operation::Page, &error),
                    None => {}
                }
            }
            Completed::Playlists(result) => {
                self.playlists_active = false;
                match result {
                    Some(Ok(playlists)) => {
                        self.faults.set(Operation::Playlists, None);
                        accept_playlists(&mut self.branch, playlists);
                    }
                    Some(Err(error)) => self.fail(Operation::Playlists, &error),
                    None => {}
                }
            }
            Completed::Like(id, liked, result) => {
                self.likes.remove(&id);
                match result {
                    Some(Ok(())) => {
                        self.faults.set(Operation::Like, None);
                        let reload_liked = self.catalogue.confirm_reaction(&id, liked);
                        if reload_liked || self.active.is_some() {
                            let due = self.pending.unwrap_or_else(Instant::now);
                            self.queue(due);
                        }
                    }
                    Some(Err(error)) => self.fail(Operation::Like, &error),
                    None => {}
                }
            }
        }
    }
}
