use core::marker::PhantomData;
use core::panic;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};

use crate::runtime;

pub type CownPtr<T> = ArcCown<T>;

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
    pub fn new<Cowns, F>(cowns: Cowns, f: F) -> Arc<Self>
    where
        Cowns: CownList + Send + 'static,
        F: for<'l> FnOnce(Cowns::CownRefs<'l>) + Send + 'static,
    {
        let mut requests = cowns.requests();
        let count = requests.len() + 1;

        requests.sort();

        let routine = Box::new(move || f(cowns.get_mut()));
        Arc::new(Self {
            routine: Mutex::new(Some(routine)),
            count: AtomicUsize::new(count),
            requests: Mutex::new(Some(requests)),
        })
    }

    /// Schedules the behaviour.
    ///
    /// Performs two phase locking (2PL) over the enqueuing of the requests.
    /// This ensures that the overall effect of the enqueue is atomic.
    fn schedule(self: Arc<Self>) {
        {
            let requests = self.requests.lock().unwrap();
            let requests = requests.as_ref().unwrap();
            assert!(requests.is_sorted());

            requests.iter().for_each(|req| req.start_enqueue(&self));

            requests.iter().for_each(|req| req.finish_enqueue());
        }

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
        assert!(
            requests
                .iter()
                .all(|req| *req.state.lock().unwrap() == ResourceState::Owned)
        );
        runtime::spawn(move || {
            routine();
            requests.iter().for_each(|req| req.release());
        });
    }
}

// endregion:Behaviour

// region:Request

pub struct Request {
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
        assert_eq!(*self.state.lock().unwrap(), ResourceState::Initial);

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
                behaviour.resolve_one();
                return;
            }
            AcquireResult::WaitFor(prev_request) => prev_request,
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

        assert_eq!(res, Ok(false))
    }

    /// Release the cown to the next behaviour.
    ///
    /// This is called when the associated behaviour has completed, and thus can allow any waiting behaviour to run.
    /// If there is no next behaviour, then the cown's `last` pointer is set to null.
    fn release(self: &Arc<Self>) {
        assert_eq!(*self.state.lock().unwrap(), ResourceState::Owned);

        let last_request = self.resource.cown.last().lock().unwrap();
        if let Some(callback) = self.callback.lock().unwrap().take() {
            assert!(
                !last_request
                    .as_ref()
                    .expect("release on not acquired cown")
                    .ptr_eq(&Arc::downgrade(self))
            );
            drop(last_request);

            callback();
        } else {
            assert!(
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

impl Ord for Request {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.resource.cown.id().cmp(&other.resource.cown.id())
    }
}

impl PartialOrd for Request {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Request {
    fn eq(&self, other: &Self) -> bool {
        matches!(self.cmp(other), core::cmp::Ordering::Equal)
    }
}

impl Eq for Request {}

// endregion:Request

// region:Cown

type InteriorMutCell<T> = core::cell::UnsafeCell<T>;
type CownId = usize;

/// Cown that wraps a value.
///
/// The value should only be accessed inside a when() block.
struct Cown<T: ?Sized> {
    /// sort request
    id: CownId,

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

    /// sort requests
    fn id(&self) -> CownId;
}

impl<T: ?Sized + Send + 'static> CownTrait for Cown<T> {
    fn last(&self) -> &Mutex<Option<Weak<Request>>> {
        &self.last_request
    }

    fn id(&self) -> CownId {
        self.id
    }
}

/// Public class for create an `Cown`
pub struct ArcCown<T: ?Sized> {
    inner: Arc<Cown<T>>,
}

static ID_GEN: AtomicUsize = AtomicUsize::new(0);

/// `ArcCown` is a wrapper around an `Arc` containing a `Cown` struct.
/// It provides methods to create a new instance and to convert it into a dynamic trait object.
impl<T> ArcCown<T> {
    /// Creates a new `ArcCown` instance with the given value.
    pub fn new(value: T) -> Self {
        Self {
            inner: Arc::new(Cown {
                id: ID_GEN.fetch_add(1, Ordering::SeqCst),
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
pub trait CownList {
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

// region:when

/// Creates a `Behavior` and schedules it. Used by "When" block.
pub fn run_when<C, F>(cowns: C, f: F)
where
    C: CownList + Send + 'static,
    F: for<'l> Fn(C::CownRefs<'l>) + Send + 'static,
{
    Behaviour::new(cowns, f).schedule();
}

/// from <https://docs.rs/tuple_list/latest/tuple_list/>
#[macro_export]
macro_rules! tuple_list {
    () => ( () );

    // handling simple identifiers, for limited types and patterns support
    ($i:ident)  => ( ($i, ()) );
    ($i:ident,) => ( ($i, ()) );
    ($i:ident, $($e:ident),*)  => ( ($i, $crate::tuple_list!($($e),*)) );
    ($i:ident, $($e:ident),*,) => ( ($i, $crate::tuple_list!($($e),*)) );

    // handling complex expressions
    ($i:expr)  => ( ($i, ()) );
    ($i:expr,) => ( ($i, ()) );
    ($i:expr, $($e:expr),*)  => ( ($i, $crate::tuple_list!($($e),*)) );
    ($i:expr, $($e:expr),*,) => ( ($i, $crate::tuple_list!($($e),*)) );
}

#[macro_export]
macro_rules! tuple_list_mut {
    () => ( () );

    // handling simple identifiers, for limited types and patterns support
    ($i:ident)  => ( (mut $i, ()) );
    ($i:ident,) => ( (mut $i, ()) );
    ($i:ident, $($e:ident),*)  => ( (mut $i, $crate::tuple_list_mut!($($e),*)) );
    ($i:ident, $($e:ident),*,) => ( (mut $i, $crate::tuple_list_mut!($($e),*)) );

    // handling complex expressions
    ($i:expr)  => ( (mut $i, ()) );
    ($i:expr,) => ( (mut $i, ()) );
    ($i:expr, $($e:expr),*)  => ( (mut $i, $crate::tuple_list_mut!($($e),*)) );
    ($i:expr, $($e:expr),*,) => ( (mut $i, $crate::tuple_list_mut!($($e),*)) );
}

/// "When" block.
#[macro_export]
macro_rules! when {
    ( $( $cs:ident ),* ; $( $gs:ident ),* ; $thunk:expr_2021 ) => {{
        #[allow(unused_mut, reason = "macro expand")]
        run_when($crate::tuple_list!($($cs.clone()),*), move |$crate::tuple_list_mut!($($gs),*)| $thunk);
    }};
}

// endregion:when

// region:util

struct ResourceHolder {
    cown: Arc<dyn CownTrait>,
}

// SAFETY: We only use ResourceHolder to access Cown's metadata
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
            None => {
                *req.state.lock().unwrap() = ResourceState::Owned;

                AcquireResult::Success
            }
            Some(last) => {
                *req.state.lock().unwrap() = ResourceState::Certificate;

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
pub struct RefMut<'b, T: ?Sized + 'b> {
    value: NonNull<T>,
    holder: ArcCown<T>,
    _marker: PhantomData<&'b mut T>,
}

impl<T: ?Sized> core::ops::Deref for RefMut<'_, T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        // SAFETY: the value is accessible as long as we hold our borrow.
        unsafe { self.value.as_ref() }
    }
}

impl<T: ?Sized> core::ops::DerefMut for RefMut<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: the value is accessible as long as we hold our borrow.
        unsafe { self.value.as_mut() }
    }
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

#[cfg(test)]
mod tests {
    use crossbeam_channel::bounded;

    use crate::*;

    #[test]
    fn boc() {
        let c1 = CownPtr::new(0);
        let c2 = CownPtr::new(0);
        let c3 = CownPtr::new(false);
        let c2_ = c2.clone();
        let c3_ = c3.clone();

        let (finish_sender, finish_receiver) = bounded(0);

        when!(c1, c2; g1, g2; {
            // c3, c2 are moved into this thunk. There's no such thing as auto-cloning move closure.
            *g1 += 1;
            *g2 += 1;
            when!(c3, c2; g3, g2; {
                *g2 += 1;
                *g3 = true;
            });
        });

        when!(c1, c2_, c3_; g1, g2, g3; {
            assert_eq!(*g1, 1);
            assert_eq!(*g2, if *g3 { 2 } else { 1 });
            finish_sender.send(()).unwrap();
        });

        // wait for termination
        finish_receiver.recv().unwrap();
    }

    #[test]
    fn boc_vec() {
        let c1 = CownPtr::new(0);
        let c2 = CownPtr::new(0);
        let c3 = CownPtr::new(false);
        let c2_ = c2.clone();
        let c3_ = c3.clone();

        let (finish_sender, finish_receiver) = bounded(0);

        run_when(vec![c1.clone(), c2.clone()], move |mut x| {
            // c3, c2 are moved into this thunk. There's no such thing as auto-cloning move closure.
            *x[0] += 1;
            *x[1] += 1;
            when!(c3, c2; g3, g2; {
                *g2 += 1;
                *g3 = true;
            });
        });

        when!(c1, c2_, c3_; g1, g2, g3; {
            assert_eq!(*g1, 1);
            assert_eq!(*g2, if *g3 { 2 } else { 1 });
            finish_sender.send(()).unwrap();
        });

        // wait for termination
        finish_receiver.recv().unwrap();
    }

    #[test]
    fn concurrency() {
        let num_thread = 2;
        let count = 100_000_000u64;

        let (tx1, rx) = bounded(num_thread);
        let tx2 = tx1.clone();

        let c1 = CownPtr::new(0);
        let c2 = CownPtr::new(0);

        let calc = || core::hint::black_box(1);

        when!(c1; g1; {
            for _ in 0..core::hint::black_box(count) {
                *g1 += core::hint::black_box(calc());
            }
            tx1.send(*g1).unwrap();
        });

        when!(c2; g2; {
            for _ in 0..core::hint::black_box(count) {
                *g2 += core::hint::black_box(calc());
            }
            tx2.send(*g2).unwrap();
        });

        for _ in 0..num_thread {
            let ret = rx.recv().unwrap();
            assert_eq!(ret, count);
        }
    }
}
