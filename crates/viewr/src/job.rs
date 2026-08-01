//! Bounded completion ownership for one-result background jobs.
//!
//! The event loop owns [`OneShotJob`]. A worker owns the corresponding
//! [`JobCompletion`], which can publish at most one result. Dropping a completion
//! without publishing wakes the event loop so a closed completion endpoint is
//! observable instead of leaving the application permanently busy.

use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};

/// The non-blocking state of a one-result background job.
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum JobPoll<T> {
    /// The worker still owns its completion endpoint.
    Pending,
    /// The worker published its only result.
    Ready(T),
    /// The completion endpoint disappeared without publishing a result.
    Disconnected,
}

/// The worker-owned endpoint for exactly one bounded result.
pub(crate) struct JobCompletion<T, N: FnOnce()> {
    sender: Option<SyncSender<T>>,
    notify: Option<N>,
}

impl<T, N: FnOnce()> JobCompletion<T, N> {
    /// Publish the only result and notify the event-loop owner.
    ///
    /// Returns `false` when the owner has already discarded or replaced the
    /// job. The consuming receiver and single-capacity channel make this send
    /// non-blocking in practice: no other producer can occupy the slot.
    pub(crate) fn complete(mut self, result: T) -> bool {
        let sender = self
            .sender
            .take()
            .expect("an owned job completion retains its sender");
        let delivered = match sender.try_send(result) {
            Ok(()) => true,
            Err(TrySendError::Disconnected(_)) => false,
            Err(TrySendError::Full(_)) => {
                unreachable!("a non-cloneable one-shot producer cannot fill its own channel")
            }
        };
        drop(sender);
        if delivered {
            if let Some(notify) = self.notify.take() {
                notify();
            }
        } else {
            self.notify = None;
        }
        delivered
    }
}

impl<T, N: FnOnce()> Drop for JobCompletion<T, N> {
    fn drop(&mut self) {
        let Some(sender) = self.sender.take() else {
            return;
        };
        drop(sender);
        if let Some(notify) = self.notify.take() {
            notify();
        }
    }
}

/// Event-loop-owned context and receiver for one bounded background result.
pub(crate) struct OneShotJob<C, T> {
    context: C,
    receiver: Receiver<T>,
}

impl<C, T> OneShotJob<C, T> {
    /// Create a one-result job and its non-cloneable worker completion endpoint.
    pub(crate) fn new<N: FnOnce()>(context: C, notify: N) -> (JobCompletion<T, N>, Self) {
        let (sender, receiver) = mpsc::sync_channel(1);
        (
            JobCompletion {
                sender: Some(sender),
                notify: Some(notify),
            },
            Self { context, receiver },
        )
    }

    /// Poll without blocking the event loop.
    pub(crate) fn poll(&self) -> JobPoll<T> {
        match self.receiver.try_recv() {
            Ok(result) => JobPoll::Ready(result),
            Err(TryRecvError::Empty) => JobPoll::Pending,
            Err(TryRecvError::Disconnected) => JobPoll::Disconnected,
        }
    }

    /// Borrow the event-loop-owned context while the job is still active.
    pub(crate) const fn context(&self) -> &C {
        &self.context
    }

    /// Consume the terminal job and return its event-loop-owned context.
    pub(crate) fn into_context(self) -> C {
        self.context
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::{JobPoll, OneShotJob};

    fn notification_counter() -> (Arc<AtomicUsize>, impl FnOnce()) {
        let notifications = Arc::new(AtomicUsize::new(0));
        let worker_counter = Arc::clone(&notifications);
        let notify = move || {
            worker_counter.fetch_add(1, Ordering::AcqRel);
        };
        (notifications, notify)
    }

    #[test]
    fn completed_job_delivers_once_and_notifies_once() {
        let (notifications, notify) = notification_counter();
        let (completion, job) = OneShotJob::new("current", notify);

        assert_eq!(job.poll(), JobPoll::Pending);
        assert!(completion.complete(7));
        assert_eq!(notifications.load(Ordering::Acquire), 1);
        assert_eq!(job.poll(), JobPoll::Ready(7));
        assert_eq!(job.into_context(), "current");
    }

    #[test]
    fn dropped_completion_reports_disconnect_and_notifies_once() {
        let (notifications, notify) = notification_counter();
        let (completion, job) = OneShotJob::<_, u8>::new("current", notify);

        drop(completion);

        assert_eq!(notifications.load(Ordering::Acquire), 1);
        assert_eq!(job.poll(), JobPoll::Disconnected);
    }

    #[test]
    fn discarded_job_rejects_late_completion_without_notification() {
        let (notifications, notify) = notification_counter();
        let (completion, job) = OneShotJob::new("stale", notify);

        drop(job);

        assert!(!completion.complete(7));
        assert_eq!(notifications.load(Ordering::Acquire), 0);
    }

    #[test]
    fn context_can_coordinate_cancellation_without_consuming_the_job() {
        let (completion, job) = OneShotJob::<_, u8>::new(AtomicUsize::new(0), || {});

        job.context().store(1, Ordering::Release);

        assert_eq!(job.context().load(Ordering::Acquire), 1);
        assert_eq!(job.poll(), JobPoll::Pending);
        drop(completion);
        assert_eq!(job.poll(), JobPoll::Disconnected);
    }
}
