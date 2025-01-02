use core::fmt::{self, Debug};
use core::marker::PhantomData;
use std::sync::Arc;

#[derive(Debug)]
enum Certificate {}

#[derive(Debug)]
enum Owned {}

#[derive(Debug)]
enum Initial {}

struct Resource<S> {
    cown: Arc<dyn CownTrait>,
    _state: PhantomData<S>,
}

impl<S> fmt::Debug for Resource<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Resource")
            .field("_state", &self._state)
            .finish_non_exhaustive()
    }
}

impl Resource<Initial> {
    fn new<T: Send + 'static>(resource: CownPtr<T>) -> Self {
        Self {
            cown: Arc::new(resource),
            _state: PhantomData,
        }
    }
}

#[derive(Debug)]
enum ResourceHolder {
    Initial(Resource<Initial>),
    Certificate(Resource<Certificate>),
    Owned(Resource<Owned>),
}
impl ResourceHolder {
    fn new<T: Send + 'static>(resource: CownPtr<T>) -> Self {
        Self::Initial(Resource::new(resource))
    }
}

#[derive(Debug)]
struct Request {
    /// hold resource
    resource: ResourceHolder,
}

impl Request {
    /// Create a new `Request`
    fn new<T: Send + 'static>(resource: CownPtr<T>) -> Self {
        Self {
            resource: ResourceHolder::new(resource),
        }
    }
}

#[derive(Debug)]
struct CownPtr<T>(T);

trait CownTrait {}

impl<T> CownTrait for CownPtr<T> {}
