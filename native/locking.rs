//! Lock acquisition that survives a poisoned mutex.
//!
//! A panic while some other operation held a registry or budget lock used to
//! make every later lock panic too, turning one failed call into a dead
//! runtime. The state behind these locks is a registry of live objects or a
//! counter, so continuing with it is both possible and preferable to taking the
//! process down; a caller that needs to know a panic happened has the journal.

use std::sync::{Mutex, MutexGuard};

pub fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}
