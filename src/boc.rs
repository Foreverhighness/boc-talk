use core::fmt::{self, Debug};
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};

use crate::runtime;

struct ResourceHolder {
    cown: Arc<dyn CownTrait>,
}

// SAFETY: We only use ResourceHolder to acquire resource
unsafe impl Sync for ResourceHolder {}
unsafe impl Send for ResourceHolder {}

impl ResourceHolder {
    fn new<T: Send + 'static>(resource: CownPtr<T>) -> Self {
        Self {
            cown: Arc::new(resource),
        }
    }

    fn acquire_atomic(&self) -> AcquireResult {
        todo!()
    }
}

#[derive(Debug)]
pub enum AcquireResult {
    Success,
    WaitFor(Weak<Request>),
}

impl fmt::Debug for ResourceHolder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Resource").finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum ResourceState {
    Initial,
    Certificate,
    Owned,
}

struct Request {
    /// hold resource
    resource: ResourceHolder,

    /// order preserve
    done: AtomicBool,

    /// resource
    state: Mutex<ResourceState>,

    /// callback
    callback: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl Request {
    /// Create a new `Request`
    fn new<T: Send + 'static>(resource: CownPtr<T>) -> Self {
        Self {
            resource: ResourceHolder::new(resource),
            done: AtomicBool::new(false),
            state: Mutex::new(ResourceState::Initial),
            callback: Mutex::default(),
        }
    }

    /// `start_enqueue` can executed parallel, so we need shared ref on behavior
    /// but the self ref may be exclusive?
    fn start_enqueue(self: &Arc<Self>, behaviour: &Arc<Behaviour>) {
        debug_assert_eq!(*self.state.lock().unwrap(), ResourceState::Initial);

        let prev_request = match self.resource.acquire_atomic() {
            AcquireResult::Success => {
                *self.state.lock().unwrap() = ResourceState::Owned;
                behaviour.resolve_one();
                return;
            }
            AcquireResult::WaitFor(weak) => {
                *self.state.lock().unwrap() = ResourceState::Certificate;
                weak
            }
        };

        let prev_request = prev_request.upgrade().expect("logic error: prev_request non-exist");

        while !prev_request.done.load(Ordering::SeqCst) {
            core::hint::spin_loop();
        }

        let req = Arc::clone(self);
        let behaviour = Arc::clone(&behaviour);
        prev_request.register_callback(move || {
            *req.state.lock().unwrap() = ResourceState::Owned;
            behaviour.resolve_one()
        });
    }

    fn register_callback<F>(&self, callback: F)
    where
        F: FnOnce() + Send + 'static,
    {
        *self.callback.lock().unwrap() = Some(Box::new(callback));
    }

    fn release(&self) {
        debug_assert_eq!(*self.state.lock().unwrap(), ResourceState::Owned);

        if let Some(callback) = self.callback.lock().unwrap().take() {
            callback();
        }
    }
}

#[derive(Debug)]
struct CownPtr<T>(T);

trait CownTrait {}

impl<T> CownTrait for CownPtr<T> {}

// region:Behaviour

/// Behaviours are the unit of concurrent execution. They are spawned with a list of required cowns and a closure.
struct Behaviour {
    routine: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    count: AtomicUsize,
    requests: Mutex<Option<Vec<Arc<Request>>>>,
}

impl Behaviour {
    fn resolve_one(self: &Arc<Self>) {
        if self.count.fetch_sub(1, Ordering::SeqCst) > 1 {
            return;
        }
        assert_eq!(self.count.load(Ordering::SeqCst), 0);

        let routine = self.routine.lock().unwrap().take().expect("logic error: take twice");
        let requests = self.requests.lock().unwrap().take().expect("logic error: take twice");

        runtime::spawn(move || {
            routine();
            requests.iter().for_each(|req| req.release());
        });
    }
}

// endregion:Behaviour

type InteriorMutCell<T> = core::cell::UnsafeCell<T>;

/// Common part of a cown that is independent of the data the cown is storing.
///
/// Just contains a pointer to the last behaviour's request for this cown.
struct CownBase {
    /// Points to the end of the queue of requests for this cown.
    ///
    /// If it is null, then the cown is not currently in use by
    /// any behaviour.
    tail: Mutex<Option<Weak<Request>>>,
}

struct Cown<T: ?Sized> {
    /// The value of this Cown
    resource: InteriorMutCell<T>,
}
