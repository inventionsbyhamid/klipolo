//! A process-wide list of callbacks for an OS signal (the network path, a wake). The backend calls them outside any
//! lock, so a listener may take its time or register another without holding up anyone.

use std::sync::{Arc, Mutex, PoisonError};

pub(crate) struct Listeners<F: ?Sized>(Mutex<Vec<Arc<F>>>);

impl<F: ?Sized> Listeners<F> {
    pub(crate) const fn new() -> Self {
        Self(Mutex::new(Vec::new()))
    }

    pub(crate) fn add(&self, listener: Arc<F>) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(listener);
    }

    /// The listeners now, copied out: the caller calls them after the lock is released.
    pub(crate) fn all(&self) -> Vec<Arc<F>> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}
