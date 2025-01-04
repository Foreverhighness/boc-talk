#![allow(unused, reason = "syntax highlighting")]

use core::fmt::Debug;
use core::hint;
use core::marker::PhantomData;
use core::mem::ManuallyDrop;
use core::pin::Pin;
use core::ptr::{self, NonNull};
use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering};
use std::sync::Arc;

use crate::runtime;

// region:Cown

/// A trait representing a `Cown`.
///
/// Instead of directly using a `Cown<T>`, which fixes _a single_ `T` we use a trait object to
/// allow multiple requests with different `T`s to be used with the same cown.
trait CownTrait {
    /// this method restrict that Request should not contains any generic?
    fn last(&self) -> &AtomicPtr<Request>;

    /// substitute last method to not expose `AtomicPtr`
    fn last_swap(&self, ptr: *mut Request, order: Ordering) -> *mut Request {
        self.last().swap(ptr, order)
    }

    /// substitute last method to not expose `AtomicPtr`
    fn last_compare_exchange(
        &self,
        current: *mut Request,
        new: *mut Request,
        success: Ordering,
        failure: Ordering,
    ) -> Result<*mut Request, *mut Request> {
        self.last().compare_exchange(current, new, success, failure)
    }
}

// /// Use RefCell to check correctness, but failed to pass borrow checker
// type InteriorMutCell<T> = core::cell::RefCell<T>;
// type CownRefMut<'l, T> = core::cell::RefMut<'l, T>;

type InteriorMutCell<T> = core::cell::UnsafeCell<T>;
// type CownRefMut<'l, T> = &'l mut T;

/// Concurrency Owned Resource
/// The value should only be accessed inside a `when!` block.
#[derive(Debug)]
struct Cown<T: ?Sized + 'static> {
    /// MCS lock tail.
    ///
    /// When a new node is enqueued, the enqueuer of the previous tail node will wait until the
    /// current enqueuer sets that node's `.next`.
    last: AtomicPtr<Request>,

    /// The value of this cown.
    resource: InteriorMutCell<T>,
}

impl<T> Cown<T> {
    const fn new(value: T) -> Self {
        Self {
            last: AtomicPtr::new(ptr::null_mut()),
            resource: InteriorMutCell::new(value),
        }
    }
}

/// Send bound is copied from `std::sync::Mutex`
/// SAFETY: Behavior must ensure there is only one thread modify Cown
unsafe impl<T: Send> Sync for Cown<T> {}

impl<T> CownTrait for Cown<T> {
    fn last(&self) -> &AtomicPtr<Request> {
        &self.last
    }
}

/// Hold Cown to access last
struct CownHolder {
    /// use to access last ptr, but need extend resource's lifetime
    inner: Arc<dyn CownTrait>,

    /// effectively we only concern about this
    _last: PhantomData<Arc<AtomicPtr<Behavior>>>,
}

/// Safety: We only use `CownHolder` to access `AtomicPtr`
unsafe impl Send for CownHolder {}

impl Debug for CownHolder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CownHolder")
            .field("cown", &Arc::as_ptr(&self.inner))
            .field("last", &self.inner.last())
            .finish()
    }
}
impl CownTrait for CownHolder {
    fn last(&self) -> &AtomicPtr<Request> {
        self.inner.last()
    }
}

impl CownHolder {
    fn new<T: 'static + Send>(cown: CownPtr<T>) -> Self {
        Self {
            inner: cown.inner,
            _last: PhantomData,
        }
    }
}

/// Public interface to Cown.
#[derive(Debug)]
pub struct CownPtr<T: 'static> {
    inner: Arc<Cown<T>>,
}

impl<T: 'static> CownPtr<T> {
    pub fn new(value: T) -> Self {
        Self {
            inner: Arc::new(Cown::new(value)),
        }
    }
}

impl<T: Send + 'static> Clone for CownPtr<T> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

// region:ArrayOfCownPtr

/// Trait for a collection of `CownPtr`s.
///
/// Users pass `CownPtrs` to `when!` clause to specify a collection of shared resources, and
/// such resources can be accessed via `CownRefs` inside the thunk.
pub trait CownPtrs {
    /// Types for references corresponding to `CownPtrs`.
    type CownRefs<'l>
    where
        Self: 'l;

    // This could return a `Box<[Request]>`, but we use a `Vec` to avoid possible reallocation
    // in the implementation.
    /// Returns a collection of `Request`.
    fn requests(&self) -> Vec<Request>;

    /// Returns mutable references of type `CownRefs`.
    fn get_mut<'l>(self) -> Self::CownRefs<'l>;
}

impl CownPtrs for () {
    type CownRefs<'l>
        = ()
    where
        Self: 'l;

    fn requests(&self) -> Vec<Request> {
        Vec::new()
    }

    fn get_mut<'l>(self) -> Self::CownRefs<'l> {}
}

impl<T: Send + 'static, Ts: CownPtrs> CownPtrs for (CownPtr<T>, Ts) {
    type CownRefs<'l>
        = (&'l mut T, Ts::CownRefs<'l>)
    where
        Self: 'l;

    fn requests(&self) -> Vec<Request> {
        let mut rs = self.1.requests();
        rs.push(Request::new(CownPtr::clone(&self.0)));
        rs
    }

    fn get_mut<'l>(self) -> Self::CownRefs<'l> {
        // SAFETY: `Self: 'l` guarantees that reference is valid.
        unsafe { (&mut *self.0.inner.resource.get(), self.1.get_mut()) }
    }
}

impl<T: Send + 'static> CownPtrs for Vec<CownPtr<T>> {
    type CownRefs<'l>
        = Vec<&'l mut T>
    where
        Self: 'l;

    fn requests(&self) -> Vec<Request> {
        self.iter().map(|x| Request::new(CownPtr::clone(x))).collect()
    }

    fn get_mut<'l>(self) -> Self::CownRefs<'l> {
        self.iter()
            // SAFETY: `Self: 'l` guarantees that reference is valid.
            .map(|x| unsafe { &mut *x.inner.resource.get() })
            .collect()
    }
}

// endregion:ArrayOfCownPtr

// endregion:Cown

// region:Request

#[derive(Debug)]
pub struct Request {
    /// use to call next `resolve_one()`
    next: AtomicPtr<Behavior>,

    /// two phase locking
    scheduled: AtomicBool,

    /// access cown last, ensure cown is valid
    target: CownHolder,
}

impl Request {
    /// Creates a new Request.
    fn new<T: Send + 'static>(target: CownPtr<T>) -> Self {
        Self {
            next: AtomicPtr::new(ptr::null_mut()),
            scheduled: AtomicBool::new(false),
            target: CownHolder::new(target),
        }
    }

    /// `start_enqueue` can executed parallel, so we need shared ref on behavior
    /// but the self ref may be exclusive?
    fn start_enqueue(&self, behavior: &Behavior) {
        let prev_req = self.target.last_swap((&raw const *self).cast_mut(), Ordering::SeqCst);

        // SAFETY: `prev_req` must contains valid pointer.
        let Some(prev_req) = (unsafe { prev_req.as_ref() }) else {
            // prev_req is null
            behavior.resolve_one();
            return;
        };

        while !prev_req.scheduled.load(Ordering::SeqCst) {
            hint::spin_loop();
        }

        debug_assert!(prev_req.next.load(Ordering::SeqCst).is_null());
        prev_req.next.store((&raw const *behavior).cast_mut(), Ordering::SeqCst);
    }

    /// Finish the second phase of the 2PL enqueue operation.
    ///
    /// Sets the scheduled flag so that subsequent behaviors can continue the 2PL enqueue.
    fn finish_enqueue(&self) {
        self.scheduled.store(true, Ordering::SeqCst);
    }

    fn release(&self) {
        let mut behavior = self.next.load(Ordering::SeqCst);
        if behavior.is_null() {
            if self
                .target
                .last_compare_exchange(
                    (&raw const *self).cast_mut(),
                    ptr::null_mut(),
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                )
                .is_ok()
            {
                return;
            }

            loop {
                behavior = self.next.load(Ordering::SeqCst);
                if !behavior.is_null() {
                    break;
                }
                core::hint::spin_loop();
            }
        }

        // SAFETY: `behavior` is valid pointer.
        let behavior = unsafe { behavior.as_ref().unwrap() };
        behavior.resolve_one();

        self.next.store(ptr::null_mut(), Ordering::SeqCst);
    }
}

impl Ord for Request {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        Arc::as_ptr(&self.target.inner)
            .addr()
            .cmp(&Arc::as_ptr(&other.target.inner).addr())
    }
}
impl PartialOrd for Request {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
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

// region:Behavior

pub struct Behavior {
    routine: Box<dyn FnOnce() + Send>,
    count: AtomicUsize,
    requests: Pin<Box<[Request]>>,
}

impl Debug for Behavior {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Behavior")
            .field("count", &self.count)
            .field("requests", &self.requests)
            .finish_non_exhaustive()
    }
}

impl Behavior {
    fn schedule(&self) {
        // eprintln!("schedule start: addr {:?} {self:?}", &raw const *self);
        debug_assert!(self.requests.is_sorted());

        self.requests.iter().for_each(|req| req.start_enqueue(self));

        self.requests.iter().for_each(Request::finish_enqueue);

        self.resolve_one();
        // eprintln!("schedule over: addr {:?} {self:?}", &raw const *self);
    }

    pub fn resolve_one(&self) {
        if self.count.fetch_sub(1, Ordering::SeqCst) > 1 {
            return;
        }
        debug_assert_eq!(self.count.load(Ordering::SeqCst), 0);

        // SAFETY: `self` is not null, and is valid
        unsafe { PinnedBehavior::from_inner(NonNull::from_ref(self)) }.run();
    }
}

pub struct PinnedBehavior(Pin<Box<Behavior>>);

impl PinnedBehavior {
    /// new Behavior at heap
    pub fn new<Cs, F>(cowns: Cs, f: F) -> Self
    where
        Cs: CownPtrs + Send + 'static,
        F: for<'l> FnOnce(Cs::CownRefs<'l>) + Send + 'static,
    {
        let mut requests = cowns.requests();
        requests.sort();

        let requests = Pin::new(requests.into_boxed_slice());
        let count = AtomicUsize::new(requests.len() + 1);

        let routine = Box::new(move || f(cowns.get_mut()));
        Self(Box::pin(Behavior {
            routine,
            count,
            requests,
        }))
    }

    /// schedule a behavior
    pub fn schedule(self) -> ManuallyDrop<Self> {
        self.0.schedule();

        ManuallyDrop::new(self)
    }

    /// submit behavior to runtime
    fn run(self) {
        // eprintln!("start running");
        let behavior = Pin::into_inner(self.0);
        runtime::spawn(move || {
            (behavior.routine)();

            behavior.requests.iter().for_each(Request::release);
        });
    }

    unsafe fn from_inner(ptr: NonNull<Behavior>) -> Self {
        // SAFETY: caller holder
        Self(Box::into_pin(unsafe { Box::from_raw(ptr.as_ptr()) }))
    }
}

// endregion:Behavior

// region:when

/// Creates a `Behavior` and schedules it. Used by "When" block.
pub fn run_when<C, F>(cowns: C, f: F)
where
    C: CownPtrs + Send + 'static,
    F: for<'l> Fn(C::CownRefs<'l>) + Send + 'static,
{
    PinnedBehavior::new(cowns, f).schedule();
}

/// from <https://docs.rs/tuple_list/latest/tuple_list/>
#[cfg_attr(feature = "ref", macro_export)]
macro_rules! tuple_list {
        () => ( () );

        // handling simple identifiers, for limited types and patterns support
        ($i:ident)  => ( ($i, ()) );
        ($i:ident,) => ( ($i, ()) );
        ($i:ident, $($e:ident),*)  => ( ($i, $crate::tuple_list!($($e),*)) );
        ($i:ident, $($e:ident),*,) => ( ($i, $crate::tuple_list!($($e),*)) );

        // handling complex expressions
        ($i:expr_2021)  => ( ($i, ()) );
        ($i:expr_2021,) => ( ($i, ()) );
        ($i:expr_2021, $($e:expr_2021),*)  => ( ($i, $crate::tuple_list!($($e),*)) );
        ($i:expr_2021, $($e:expr_2021),*,) => ( ($i, $crate::tuple_list!($($e),*)) );
    }

/// "When" block.
#[cfg_attr(feature = "ref", macro_export)]
macro_rules! when {
        ( $( $cs:ident ),* ; | $( $gs:ident ),* | $thunk:expr_2021 ) => {{
            run_when($crate::tuple_list!($($cs.clone()),*), move |$crate::tuple_list!($($gs),*)| $thunk);
        }};
    }

// endregion:when
