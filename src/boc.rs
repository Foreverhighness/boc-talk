use core::fmt::Debug;
use core::marker::PhantomData;
use core::panic;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};

use crate::runtime;

// region:Behaviour

/// Behaviour that captures the content of a when body.
///
/// It contains all the state required to run the body, and release the
/// cowns when the body has finished.
///
/// Behaviours are the unit of concurrent execution.
/// They are spawned with a list of required cowns and a closure.
struct Behaviour {
    /// The body of the behaviour.
    routine: Mutex<Option<Box<dyn FnOnce() + Send>>>,

    /// How many requests are outstanding for the behaviour.
    count: AtomicUsize,

    /// The set of requests for this behaviour.
    ///
    /// This is used to release the cowns to the subsequent behaviours.
    requests: Mutex<Option<Vec<Arc<Request>>>>,
}

impl Behaviour {
    /// Creates a new Behaviour but not scheduled.
    pub fn new<Cowns, F>(cowns: Cowns, f: F) -> Self
    where
        Cowns: CownTrait + Send + 'static,
        F: FnOnce(&dyn CownTrait) + Send + 'static,
    {
        let routine = todo!();
        let requests: Vec<Arc<Request>> = todo!();
        let len = requests.len();
        Self {
            routine: Mutex::new(Some(routine)),
            count: AtomicUsize::new(len),
            requests: Mutex::new(Some(requests)),
        }
    }

    /// Schedules the behaviour.
    ///
    /// Performs two phase locking (2PL) over the enqueuing of the requests.
    /// This ensures that the overall effect of the enqueue is atomic.
    fn schedule(self: Arc<Self>) {
        let requests = self.requests.lock().unwrap();
        let requests = requests.as_ref().unwrap();

        requests.iter().for_each(|req| req.start_enqueue(&self));

        requests.iter().for_each(|req| req.finish_enqueue());

        self.resolve_one();
    }

    /// Resolves a single outstanding request for this behaviour.
    ///
    /// Called when a request is at the head of the queue for a particular cown.
    /// If this is the last request, then the thunk is scheduled.
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

// region:Request

struct Request {
    /// The cown that this request is for.
    resource: ResourceHolder,

    /// Flag to indicate the associated behaviour to this request has been scheduled
    scheduled: AtomicBool,

    /// Resource state
    state: Mutex<ResourceState>,

    /// Notify the next behaviour in the queue
    callback: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl Request {
    /// Create a new `Request`
    fn new<T>(resource: ArcCown<T>) -> Arc<Self>
    where
        T: Send + 'static,
    {
        Arc::new(Self {
            resource: ResourceHolder::new(resource),
            scheduled: AtomicBool::new(false),
            state: Mutex::new(ResourceState::Initial),
            callback: Mutex::default(),
        })
    }

    /// Start the first phase of the 2PL enqueue operation.
    ///
    /// This enqueues the request onto the cown.
    /// It will only return once any previous behaviour on this cown has finished enqueueing on all its required cowns.
    /// This ensures that the 2PL is obeyed.
    fn start_enqueue(self: &Arc<Self>, behaviour: &Arc<Behaviour>) {
        debug_assert_eq!(*self.state.lock().unwrap(), ResourceState::Initial);

        let f = || {
            let req = Arc::clone(self);
            let behaviour = Arc::clone(behaviour);
            move || {
                *req.state.lock().unwrap() = ResourceState::Owned;
                behaviour.resolve_one()
            }
        };

        let prev_request = match self.resource.acquire_or_register_callback(self, f) {
            AcquireResult::Success => {
                *self.state.lock().unwrap() = ResourceState::Owned;
                behaviour.resolve_one();
                return;
            }
            AcquireResult::WaitFor(prev_request) => {
                *self.state.lock().unwrap() = ResourceState::Certificate;
                prev_request
            }
        };

        // Keep ordering
        while !prev_request.scheduled.load(Ordering::SeqCst) {
            core::hint::spin_loop();
        }
    }

    /// Notify the next behaviour in the queue
    fn register_callback<F>(&self, callback: F)
    where
        F: FnOnce() + Send + 'static,
    {
        *self.callback.lock().unwrap() = Some(Box::new(callback));
    }

    /// Finish the second phase of the 2PL enqueue operation.
    ///
    /// This will set the `done` flag, so subsequent behaviours on this
    /// cown can continue the 2PL enqueue.
    fn finish_enqueue(&self) {
        let res = self
            .scheduled
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst);

        debug_assert_eq!(res, Ok(false))
    }

    /// Release the cown to the next behaviour.
    ///
    /// This is called when the associated behaviour has completed, and thus can allow any waiting behaviour to run.
    /// If there is no next behaviour, then the cown's `last` pointer is set to null.
    fn release(self: &Arc<Self>) {
        debug_assert_eq!(*self.state.lock().unwrap(), ResourceState::Owned);

        let last_request = self.resource.cown.last().lock().unwrap();
        if let Some(callback) = self.callback.lock().unwrap().take() {
            debug_assert!(
                !last_request
                    .as_ref()
                    .expect("release on not acquired cown")
                    .ptr_eq(&Arc::downgrade(self))
            );
            drop(last_request);

            callback();
        } else {
            debug_assert!(
                last_request
                    .as_ref()
                    .expect("release on not acquired cown")
                    .ptr_eq(&Arc::downgrade(self))
            );
            let mut last_request = last_request;

            *last_request = None;
        }
    }
}

// endregion:Request

// region:Cown

type InteriorMutCell<T> = core::cell::UnsafeCell<T>;

/// Cown that wraps a value.
///
/// The value should only be accessed inside a when() block.
struct Cown<T: ?Sized> {
    /// Points to the end of the queue of requests for this cown.
    ///
    /// If it is none, then the cown is not currently in use by
    /// any behaviour.
    last_request: Mutex<Option<Weak<Request>>>,

    /// correctness checker
    borrow_at: Mutex<Option<&'static core::panic::Location<'static>>>,

    /// The value of this Cown
    resource: InteriorMutCell<T>,
}

/// SAFETY: The value should access exclusively
unsafe impl<T: ?Sized + Send> Sync for Cown<T> {}
// unsafe impl<T: ?Sized + Send> Send for Cown<T> {}

impl<T: ?Sized> ArcCown<T> {
    #[track_caller]
    fn get_mut<'l>(self) -> RefMut<'l, T> {
        let mut borrow_at = self.inner.borrow_at.try_lock().expect("logic error: lock contention");
        if let Some(loc) = *borrow_at {
            panic!("logic error: borrow twice, prev access at {loc}")
        }
        *borrow_at = Some(core::panic::Location::caller());
        drop(borrow_at);

        // SAFETY: `borrow_at` guarantees unique access.
        let value = unsafe { NonNull::new_unchecked(self.inner.resource.get()) };

        RefMut {
            value,
            holder: self,
            _marker: PhantomData,
        }
    }
}

/// Cown trait, use `Arc<dyn CownTrait>` to hold data part.
///
/// `last` method to access metadata
trait CownTrait: Sync + 'static {
    /// Register new Request on this cown
    fn last(&self) -> &Mutex<Option<Weak<Request>>>;
}

impl<T: ?Sized + Send + 'static> CownTrait for Cown<T> {
    fn last(&self) -> &Mutex<Option<Weak<Request>>> {
        &self.last_request
    }
}

/// Public class for create an `Cown`
pub struct ArcCown<T: ?Sized> {
    inner: Arc<Cown<T>>,
}

/// `ArcCown` is a wrapper around an `Arc` containing a `Cown` struct.
/// It provides methods to create a new instance and to convert it into a dynamic trait object.
impl<T> ArcCown<T> {
    /// Creates a new `ArcCown` instance with the given value.
    pub fn new(value: T) -> Self {
        Self {
            inner: Arc::new(Cown {
                last_request: Mutex::default(),
                borrow_at: Mutex::default(),
                resource: InteriorMutCell::new(value),
            }),
        }
    }

    /// Converts the `ArcCown` into a dynamic trait object (`Arc<dyn CownTrait>`).
    fn into_dyn(self) -> Arc<dyn CownTrait>
    where
        T: Send + 'static,
    {
        self.inner
    }
}

impl<T: 'static> Clone for ArcCown<T> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

// endregion:Cown

// region:CownList

/// Trait for a collection of `CownPtr`s.
///
/// Users pass `CownPtrs` to `when!` clause to specify a collection of shared resources, and
/// such resources can be accessed via `CownRefs` inside the thunk.
trait CownList {
    /// Types for references corresponding to `CownPtrs`.
    type CownRefs<'l>
    where
        Self: 'l;

    /// Returns a collection of `Request`.
    fn requests(&self) -> Vec<Arc<Request>>;

    /// Returns mutable references of type `CownRefs`.
    fn get_mut<'l>(self) -> Self::CownRefs<'l>;
}

impl CownList for () {
    type CownRefs<'l>
        = ()
    where
        Self: 'l;

    fn requests(&self) -> Vec<Arc<Request>> {
        Vec::new()
    }

    fn get_mut<'l>(self) -> Self::CownRefs<'l> {
        ()
    }
}

impl<T: Send + 'static, Others: CownList> CownList for (ArcCown<T>, Others) {
    type CownRefs<'l>
        = (RefMut<'l, T>, Others::CownRefs<'l>)
    where
        Self: 'l;

    fn requests(&self) -> Vec<Arc<Request>> {
        let mut rs = self.1.requests();
        rs.push(Request::new(ArcCown::clone(&self.0)));
        rs
    }

    fn get_mut<'l>(self) -> Self::CownRefs<'l> {
        (self.0.get_mut(), self.1.get_mut())
    }
}

impl<T: Send + 'static> CownList for Vec<ArcCown<T>> {
    type CownRefs<'l>
        = Vec<RefMut<'l, T>>
    where
        Self: 'l;

    fn requests(&self) -> Vec<Arc<Request>> {
        self.iter().map(|ptr| Request::new(ArcCown::clone(ptr))).collect()
    }

    fn get_mut<'l>(self) -> Self::CownRefs<'l> {
        self.into_iter().map(|ptr| ptr.get_mut()).collect()
    }
}

// endregion:CownList

// region:util

struct ResourceHolder {
    cown: Arc<dyn CownTrait>,
}

// SAFETY: We only use ResourceHolder to acquire resource
unsafe impl Sync for ResourceHolder {}
unsafe impl Send for ResourceHolder {}

impl ResourceHolder {
    fn new<T: Send + 'static>(resource: ArcCown<T>) -> Self {
        Self {
            cown: resource.into_dyn(),
        }
    }

    fn acquire_or_register_callback<Callback, F>(&self, req: &Arc<Request>, f: F) -> AcquireResult
    where
        F: FnOnce() -> Callback,
        Callback: FnOnce() + Send + 'static,
    {
        let mut last_request = self.cown.last().lock().unwrap();
        let result = match last_request.as_ref() {
            None => AcquireResult::Success,
            Some(last) => {
                let prev_request = last.upgrade().expect("logic error: prev_request non-exist");

                prev_request.register_callback(f());
                AcquireResult::WaitFor(prev_request)
            }
        };

        *last_request = Some(Arc::downgrade(req));

        result
    }
}

enum AcquireResult {
    Success,
    WaitFor(Arc<Request>),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum ResourceState {
    Initial,
    Certificate,
    Owned,
}

/// reference to [`core::cell::RefMut`]
struct RefMut<'b, T: ?Sized + 'b> {
    value: NonNull<T>,
    holder: ArcCown<T>,
    _marker: PhantomData<&'b mut T>,
}

impl<T: ?Sized> Drop for RefMut<'_, T> {
    fn drop(&mut self) {
        let mut borrow_at = self
            .holder
            .inner
            .borrow_at
            .try_lock()
            .expect("logic error: lock contention");
        *borrow_at = None;
    }
}

// endregion:util
