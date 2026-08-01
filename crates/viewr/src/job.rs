//! Bounded completion ownership for one-result background jobs.
//!
//! The event loop owns [`OneShotJob`]. A worker owns the corresponding
//! [`JobCompletion`], which can publish at most one result. Dropping a completion
//! without publishing wakes the event loop so a closed completion endpoint is
//! observable instead of leaving the application permanently busy.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
use std::sync::{Arc, Mutex};

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

    /// Mutably borrow the event-loop-owned context while the job is active.
    pub(crate) const fn context_mut(&mut self) -> &mut C {
        &mut self.context
    }

    /// Consume the terminal job and return its event-loop-owned context.
    pub(crate) fn into_context(self) -> C {
        self.context
    }
}

const WAKE_PENDING: u8 = 0;
const WAKE_ARMED: u8 = 1;
const WAKE_SIGNALED: u8 = 2;
const WAKE_REJECTED: u8 = 3;
const WAKE_FIRED: u8 = 4;

/// Arms completion notification only after a bounded executor accepts work.
struct CompletionWake<N: FnOnce()> {
    state: AtomicU8,
    notify: Mutex<Option<N>>,
}

impl<N: FnOnce()> CompletionWake<N> {
    fn new(notify: N) -> Self {
        Self {
            state: AtomicU8::new(WAKE_PENDING),
            notify: Mutex::new(Some(notify)),
        }
    }

    fn signal(&self) {
        match self.state.compare_exchange(
            WAKE_PENDING,
            WAKE_SIGNALED,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) | Err(WAKE_SIGNALED | WAKE_REJECTED | WAKE_FIRED) => {}
            Err(WAKE_ARMED) => self.fire_from(WAKE_ARMED),
            Err(unexpected) => unreachable!("invalid completion wake state {unexpected}"),
        }
    }

    fn arm(&self) {
        match self.state.compare_exchange(
            WAKE_PENDING,
            WAKE_ARMED,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) | Err(WAKE_ARMED | WAKE_FIRED) => {}
            Err(WAKE_SIGNALED) => self.fire_from(WAKE_SIGNALED),
            Err(WAKE_REJECTED) => unreachable!("rejected completion wake cannot be armed"),
            Err(unexpected) => unreachable!("invalid completion wake state {unexpected}"),
        }
    }

    fn reject(&self) {
        self.state.store(WAKE_REJECTED, Ordering::Release);
        self.notify
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
    }

    fn fire_from(&self, expected: u8) {
        if self
            .state
            .compare_exchange(expected, WAKE_FIRED, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        let notify = self
            .notify
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(notify) = notify {
            notify();
        }
    }
}

/// Submit one one-shot job and install its owner only after executor acceptance.
///
/// Fast completion and accepted closure drop notify exactly once after `install`
/// has run. Rejected work installs no owner and stays silent, preventing bounded
/// queue saturation from turning into an event-loop retry spin.
pub(crate) fn try_schedule_one_shot<C, T, N, S, W, I>(
    context: C,
    notify: N,
    schedule: S,
    work: W,
    install: I,
) -> bool
where
    T: Send + 'static,
    N: FnOnce() + Send + 'static,
    S: FnOnce(Box<dyn FnOnce() + Send>) -> bool,
    W: FnOnce() -> T + Send + 'static,
    I: FnOnce(OneShotJob<C, T>),
{
    let wake = Arc::new(CompletionWake::new(notify));
    let completion_wake = Arc::clone(&wake);
    let (completion, owner) = OneShotJob::new(context, move || completion_wake.signal());
    let task = Box::new(move || {
        let _ = completion.complete(work());
    });
    if !schedule(task) {
        wake.reject();
        return false;
    }

    install(owner);
    wake.arm();
    true
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::{JobPoll, OneShotJob, try_schedule_one_shot};

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

    #[test]
    fn accepted_fast_work_installs_owner_before_exactly_one_notification() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let notify_events = Arc::clone(&events);
        let installed_events = Arc::clone(&events);
        let owner = RefCell::new(None);

        assert!(try_schedule_one_shot(
            "accepted",
            move || notify_events.lock().unwrap().push("notified"),
            |task| {
                task();
                true
            },
            || 7_u8,
            |job| {
                installed_events.lock().unwrap().push("installed");
                owner.replace(Some(job));
            },
        ));

        assert_eq!(*events.lock().unwrap(), ["installed", "notified"]);
        let job = owner.take().unwrap();
        assert_eq!(job.poll(), JobPoll::Ready(7));
    }

    #[test]
    fn accepted_delayed_work_notifies_exactly_once_after_arming() {
        let notifications = Arc::new(AtomicUsize::new(0));
        let notified = Arc::clone(&notifications);
        let queued = RefCell::new(None);
        let owner = RefCell::new(None);

        assert!(try_schedule_one_shot(
            "accepted",
            move || {
                notified.fetch_add(1, Ordering::AcqRel);
            },
            |task| {
                queued.replace(Some(task));
                true
            },
            || 7_u8,
            |job| {
                owner.replace(Some(job));
            },
        ));
        assert_eq!(notifications.load(Ordering::Acquire), 0);

        queued.take().unwrap()();

        assert_eq!(notifications.load(Ordering::Acquire), 1);
        assert_eq!(owner.take().unwrap().poll(), JobPoll::Ready(7));
    }

    #[test]
    fn accepted_closure_drop_installs_disconnect_and_notifies_once() {
        let notifications = Arc::new(AtomicUsize::new(0));
        let notified = Arc::clone(&notifications);
        let owner = RefCell::new(None);

        assert!(try_schedule_one_shot(
            "accepted",
            move || {
                notified.fetch_add(1, Ordering::AcqRel);
            },
            |task| {
                drop(task);
                true
            },
            || 7_u8,
            |job| {
                owner.replace(Some(job));
            },
        ));

        assert_eq!(notifications.load(Ordering::Acquire), 1);
        assert_eq!(owner.take().unwrap().poll(), JobPoll::Disconnected);
    }

    #[test]
    fn rejected_work_installs_nothing_and_never_notifies() {
        let notifications = Arc::new(AtomicUsize::new(0));
        let notified = Arc::clone(&notifications);
        let installed = Cell::new(false);

        assert!(!try_schedule_one_shot(
            "rejected",
            move || {
                notified.fetch_add(1, Ordering::AcqRel);
            },
            |task| {
                drop(task);
                false
            },
            || 7_u8,
            |_| installed.set(true),
        ));

        assert!(!installed.get());
        assert_eq!(notifications.load(Ordering::Acquire), 0);
    }
}
