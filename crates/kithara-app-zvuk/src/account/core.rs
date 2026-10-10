use kithara_app_library::{AccessToken, OpenUrlError, Secrets};
use kithara_net::Net;
use kithara_platform::{
    CancelToken,
    maybe_send::{MaybeSend, MaybeSync},
    sync::Arc,
    tokio::{
        runtime::Handle,
        sync::{
            mpsc::{self, UnboundedSender},
            watch,
        },
        task,
    },
};
use url::Url;

use super::{auth::Auth, task::Task};
use crate::Client;

/// Opens a URL in the browser.
pub trait Opener: Fn(&Url) -> Result<(), OpenUrlError> + MaybeSend + MaybeSync {}

impl<F> Opener for F where F: Fn(&Url) -> Result<(), OpenUrlError> + MaybeSend + MaybeSync {}

/// What the account row shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum State {
    SignedOut,
    /// A device code waits for confirmation in the browser.
    Waiting,
    Connected {
        label: Option<String>,
    },
}

/// The account row as published.
#[derive(Clone, Debug)]
pub(crate) struct Row {
    pub(crate) state: State,
    pub(crate) fault: Option<AccountError>,
}

#[derive(Debug)]
pub(crate) enum Command {
    Connect,
    Cancel,
    Disconnect,
    /// Zvuk refused this token on a catalogue request.
    Rejected(AccessToken),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AccountError {
    Session,
    Browser,
    Confirmation,
    Store,
    Rejected,
}

/// A handle on the account task, which ends with its cancellation.
pub(crate) struct Account {
    pub(crate) commands: UnboundedSender<Command>,
    pub(crate) row: watch::Receiver<Row>,
    pub(crate) token: watch::Receiver<Option<AccessToken>>,
}

impl Account {
    /// Starts the account from the stored token, if any.
    pub(crate) fn spawn<N>(
        client: Client<N>,
        secrets: Secrets,
        open: Arc<dyn Opener>,
        runtime: &Handle,
        cancel: CancelToken,
    ) -> Self
    where
        N: Net + Clone + 'static,
    {
        let (row_sender, row) = watch::channel(Row {
            state: State::SignedOut,
            fault: None,
        });
        let (token_sender, token) = watch::channel(None);
        let (commands, received) = mpsc::unbounded_channel();
        let task = Task {
            auth: Auth::new(client),
            secrets,
            open,
            runtime: runtime.clone(),
            cancel,
            row: row_sender,
            token: token_sender,
            flow: None,
            label: None,
        };
        drop(task::spawn_on(runtime, task.run(received)));
        Self {
            commands,
            row,
            token,
        }
    }
}
