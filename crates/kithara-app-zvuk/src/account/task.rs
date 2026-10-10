use std::future;

use kithara_app_library::{AccessToken, SecretError, Secrets};
use kithara_net::Net;
use kithara_platform::{
    CancelToken,
    maybe_send::BoxFuture,
    sync::Arc,
    time::{self, Duration, Instant},
    tokio::{
        self,
        runtime::Handle,
        sync::{mpsc::UnboundedReceiver, watch},
        task,
    },
};
use kithara_test_utils::kithara;

use super::{
    AccountError, Command, Opener, Row, State,
    auth::{Auth, Poll},
};
use crate::consts;

/// A device flow's confirmed token, or the fault that ended it; none when
/// the code expired.
type Outcome = Result<AccessToken, Option<AccountError>>;

/// The single writer of the account's row and token.
pub(super) struct Task<N> {
    pub(super) auth: Auth<N>,
    pub(super) secrets: Secrets,
    pub(super) open: Arc<dyn Opener>,
    pub(super) runtime: Handle,
    pub(super) cancel: CancelToken,
    pub(super) row: watch::Sender<Row>,
    pub(super) token: watch::Sender<Option<AccessToken>>,
    /// The device flow in progress; dropping it ends the sign-in.
    pub(super) flow: Option<BoxFuture<'static, Outcome>>,
    /// The profile name of a token.
    pub(super) label: Option<BoxFuture<'static, (AccessToken, Option<String>)>>,
}

impl<N> Task<N>
where
    N: Net + Clone + 'static,
{
    pub(super) async fn run(mut self, commands: UnboundedReceiver<Command>) {
        let cancel = self.cancel.clone();
        tokio::select! {
            biased;
            () = cancel.cancelled() => {}
            () = self.serve(commands) => {}
        }
    }

    async fn serve(&mut self, mut commands: UnboundedReceiver<Command>) {
        self.restore().await;
        loop {
            tokio::select! {
                biased;
                command = commands.recv() => match command {
                    Some(command) => self.command(command).await,
                    None => break,
                },
                outcome = next(&mut self.flow) => match outcome {
                    Ok(token) => self.confirmed(token).await,
                    Err(fault) => self.publish(State::SignedOut, fault),
                },
                (token, label) = next(&mut self.label) => self.labelled(&token, label),
            }
        }
    }

    async fn restore(&mut self) {
        match self.store(|secrets| secrets.get(consts::STORE_KEY)).await {
            Ok(Some(value)) => self.connected(AccessToken::new(value)),
            Ok(None) => {}
            Err(fault) => self.publish(State::SignedOut, Some(fault)),
        }
    }

    async fn command(&mut self, command: Command) {
        let state = self.row.borrow().state.clone();
        match (command, state) {
            (Command::Connect, State::SignedOut) => self.connect(),
            (Command::Cancel, State::Waiting) => {
                self.flow = None;
                self.publish(State::SignedOut, None);
            }
            (Command::Disconnect, State::Connected { .. }) => self.disconnect().await,
            (Command::Rejected(token), State::Connected { .. }) if self.holds(&token) => {
                self.token.send_replace(None);
                self.publish(State::SignedOut, Some(AccountError::Rejected));
                self.forget().await;
            }
            _ => {}
        }
    }

    fn connect(&mut self) {
        self.publish(State::Waiting, None);
        let auth = self.auth.clone();
        let open = Arc::clone(&self.open);
        self.flow = Some(Box::pin(async move { sign_in(&auth, open).await }));
    }

    async fn disconnect(&mut self) {
        let Some(token) = self.token.send_replace(None) else {
            return;
        };
        self.publish(State::SignedOut, None);
        self.revoke(token);
        self.forget().await;
    }

    /// Deletes the stored token; a failure shows on the signed-out row.
    async fn forget(&mut self) {
        if let Err(fault) = self
            .store(|secrets| secrets.delete(consts::STORE_KEY))
            .await
        {
            self.publish(State::SignedOut, Some(fault));
        }
    }

    async fn confirmed(&mut self, token: AccessToken) {
        let value = token.expose().to_owned();
        match self
            .store(move |secrets| secrets.set(consts::STORE_KEY, &value))
            .await
        {
            Ok(()) => self.connected(token),
            Err(fault) => {
                self.publish(State::SignedOut, Some(fault));
                self.revoke(token);
            }
        }
    }

    fn connected(&mut self, token: AccessToken) {
        self.token.send_replace(Some(token.clone()));
        self.publish(State::Connected { label: None }, None);
        self.label(token);
    }

    /// Fetches the profile name of `token`.
    fn label(&mut self, token: AccessToken) {
        let auth = self.auth.clone();
        self.label = Some(Box::pin(async move {
            let label = auth.label(&token).await;
            (token, label)
        }));
    }

    fn labelled(&self, token: &AccessToken, label: Option<String>) {
        if !self.holds(token) {
            return;
        }
        let mut row = self.row.borrow().clone();
        if let State::Connected { label: shown } = &mut row.state {
            *shown = label;
            self.row.send_replace(row);
        }
    }

    /// Asks Zvuk to revoke `token` while the account runs.
    fn revoke(&self, token: AccessToken) {
        let auth = self.auth.clone();
        let cancel = self.cancel.clone();
        drop(task::spawn_on(&self.runtime, async move {
            tokio::select! {
                biased;
                () = cancel.cancelled() => {}
                () = auth.logout(&token) => {}
            }
        }));
    }

    /// Whether the account publishes `token`.
    fn holds(&self, token: &AccessToken) -> bool {
        self.token.borrow().as_ref() == Some(token)
    }

    fn publish(&self, state: State, fault: Option<AccountError>) {
        self.row.send_replace(Row { state, fault });
    }

    /// Runs a secret store call off the async workers.
    fn store<T, F>(&self, call: F) -> impl Future<Output = Result<T, AccountError>> + use<N, T, F>
    where
        T: Send + 'static,
        F: FnOnce(&Secrets) -> Result<T, SecretError> + Send + 'static,
    {
        let secrets = self.secrets.clone();
        let call = task::spawn_blocking_on(&self.runtime, move || call(&secrets));
        async move {
            match call.await {
                Ok(Ok(value)) => Ok(value),
                Ok(Err(_)) | Err(_) => Err(AccountError::Store),
            }
        }
    }
}

/// The output of the future in `slot`, which then empties; an empty slot
/// never yields.
async fn next<T>(slot: &mut Option<BoxFuture<'static, T>>) -> T {
    let Some(future) = slot else {
        return future::pending().await;
    };
    let output = future.await;
    *slot = None;
    output
}

/// Starts a device authorization, opens its page and polls until the code
/// is confirmed or expires.
#[kithara::flash(true)]
async fn sign_in<N: Net>(auth: &Auth<N>, open: Arc<dyn Opener>) -> Outcome {
    let session = auth.session().await.ok_or(AccountError::Session)?;
    let url = session.url.clone();
    if !matches!(task::spawn_sync(move || open(&url)).await, Ok(Ok(()))) {
        return Err(Some(AccountError::Browser));
    }
    let deadline = Instant::now() + Duration::from_secs(session.expires_in);
    loop {
        time::sleep(Duration::from_millis(consts::POLL_INTERVAL_MS)).await;
        if Instant::now() >= deadline {
            return Err(None);
        }
        match auth
            .poll(&session.device_code)
            .await
            .ok_or(AccountError::Confirmation)?
        {
            Poll::Pending {} => {}
            Poll::Success { access_token } => return Ok(AccessToken::new(access_token)),
        }
    }
}
