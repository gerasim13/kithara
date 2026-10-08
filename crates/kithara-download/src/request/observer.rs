use kithara_events::EventBus;
use kithara_net::{NetError, NetObserver};
use kithara_platform::time::Duration;

use crate::{DownloaderEvent, RequestId};

pub(super) struct RequestObserver {
    pub(super) bus: EventBus,
    pub(super) request_id: RequestId,
}

impl NetObserver for RequestObserver {
    fn body_resumed(&self, resume_number: u32, from_offset: u64, honoured_range: bool) {
        self.bus.publish(DownloaderEvent::BodyResumed {
            resume_number,
            from_offset,
            honoured_range,
            request_id: self.request_id,
        });
    }

    fn body_stalled(&self, consumed: u64, expected: Option<u64>, stall: Duration) {
        self.bus.publish(DownloaderEvent::BodyStalled {
            consumed,
            expected,
            stall,
            request_id: self.request_id,
        });
    }

    fn first_byte(&self, ttfb: Duration, status: u16, partial: bool) {
        self.bus.publish(DownloaderEvent::FirstByte {
            ttfb,
            status,
            partial,
            request_id: self.request_id,
        });
    }

    fn retry_exhausted(&self, max_retries: u32, consumed: u64, error: &NetError) {
        self.bus.publish(DownloaderEvent::RetryExhausted {
            max_retries,
            consumed,
            request_id: self.request_id,
            error: error.clone(),
        });
    }

    fn retrying(&self, attempt: u32, max_retries: u32, error: &NetError, backoff: Duration) {
        self.bus.publish(DownloaderEvent::RequestRetrying {
            attempt,
            max_retries,
            backoff,
            request_id: self.request_id,
            error: error.clone(),
        });
    }
}
