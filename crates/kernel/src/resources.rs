use crate::Fallible;
use crate::failures::KernelFailure;
use std::any::{Any, TypeId, type_name};
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::Arc;

/// Host resources sealed into the kernel at finish, independently of module registration.
#[derive(Default)]
pub struct Resources(HashMap<TypeId, Arc<dyn Any + Send + Sync>>);

impl Resources {
    /// An empty resource set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Install a host resource by its concrete type.
    ///
    /// # Errors
    /// `kernel.resource_taken` when the host installed the same type twice.
    pub fn with<T: Send + Sync + 'static>(mut self, value: Arc<T>) -> Fallible<Self> {
        match self.0.entry(TypeId::of::<T>()) {
            Entry::Vacant(entry) => {
                entry.insert(value);
                Ok(self)
            }
            Entry::Occupied(_) => Err(Box::new(KernelFailure::ResourceTaken {
                resource: type_name::<T>(),
            })),
        }
    }

    pub(crate) fn get<T: Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        Arc::clone(self.0.get(&TypeId::of::<T>())?)
            .downcast::<T>()
            .ok()
    }
}
