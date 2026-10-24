use futures::future::poll_fn;
use kithara_platform::{CancelToken, sync::Arc, tokio::select};

use super::{
    call::{Call, CallState, HostBodyStream},
    slot,
    transport::{HostEvents, HostRequest},
};
use crate::{backend::pooled::ByteBuffers, error::NetError};

pub(super) struct Opened {
    pub(super) call: Call,
    pub(super) headers: Vec<(String, String)>,
    pub(super) status: u16,
}

#[derive(Clone)]
pub(super) struct Exchange {
    pub(super) buffers: ByteBuffers,
    pub(super) cancel: CancelToken,
}

impl Exchange {
    pub(super) fn body(&self, call: Call) -> HostBodyStream {
        HostBodyStream::new(call, self.buffers.clone(), self.cancel.clone())
    }

    pub(super) async fn open(&self, request: HostRequest) -> Result<Opened, NetError> {
        let transport = slot::installed()?;
        let state = Arc::new(CallState::default());
        let call = transport.start(request, HostEvents(Arc::clone(&state)))?;
        let call = Call::new(call, state);

        let response = select! {
            biased;
            () = self.cancel.cancelled() => Err(NetError::Cancelled),
            response = poll_fn(|cx| call.state().poll_response(cx)) => response,
        }?;
        Ok(Opened {
            call,
            headers: response.headers,
            status: response.status,
        })
    }
}
