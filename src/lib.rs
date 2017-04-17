//! A lock that allows for an unlimited number of concurrent readers, which are never blocked.
//! Only one writer can access the resource at a time.
//! # Examples
//! ```
//! use rculock::{RcuLock, RcuGuard};
//!
//! // Create a new RcuLock protecting a piece of data, in this case a number (u32).
//! let data: RcuLock<u32> = RcuLock::new(5);
//! assert_eq!(5, *data.read());
//! {
//!     // The data is cloned and handed to the writer
//!     let mut guard: RcuGuard<u32> = data.write();
//!     // RcuGuard implements `Deref` and `DerefMut` for easy access to the data.
//!     *guard = 4;
//!     // The writer has changed its copy of the data, but the changes
//!     // have not yet made it back to the master `RcuLock`.
//!     assert_eq!(5, *data.read());
//! }
//! // After the write guard is dropped, the state of the resource
//! // as the writer sees it is atomically stored back into the master RcuLock.
//! assert_eq!(4, *data.read());
//! ```

extern crate parking_lot;
extern crate coco;
use std::mem::drop;
use std::sync::Arc;
use std::ops::{Deref, DerefMut};
use coco::epoch::{self, Atomic, Garbage};
use parking_lot::{Mutex, MutexGuard};

#[derive(Debug)]
pub struct RcuLock<T> {
    /// The resource protected by the lock, behind an `Atomic` for atomic stores,
    /// and an Arc to hand the resource out to readers without fear of memory leaks.
    inner: Atomic<Arc<T>>,
    /// Mutex to ensure at most one writer to prevent a data race, which will occur
    /// when multiple writers each acquire a copy of the resource protected by the
    /// `RcuLock`, write to it, and then store their individual changes to the master `RcuLock`.
    /// Acquired on `write()` and released when `RcuGuard` is dropped.
    write_lock: Mutex<()>,
    /// Epoch-based garbage collector to free our Arc<T> once there's no more
    /// references to it.
    garbage: Garbage,
}

impl<T: Clone + Send + 'static> RcuLock<T> {
    /// Create a new RcuLock.
    pub fn new(target: T) -> RcuLock<T> {
        let inner = Atomic::from_box(Box::new(Arc::new(target)), 0);
        RcuLock {
            inner: inner,
            write_lock: Mutex::new(()),
            garbage: Garbage::new(),
        }
    }

    /// Acquire a read handle to the `RcuLock`.  This operation never blocks.
    pub fn read(&self) -> Arc<T> {
        epoch::pin(|pin| {
            self.inner.load(pin).unwrap().clone()
        })
    }

    /// Acquire an exclusive write handle to the `RcuLock`, protected by an `RcuGuard`.
    /// This operation blocks if another `RcuGuard` is currently alive, i.e.
    /// the `RcuLock` has already handed one out to another writer.
    ///
    /// Clones the data protected by the `RcuLock`, which can be expensive.
    pub fn write(&self) -> RcuGuard<T> {
        let guard = self.write_lock.lock();
        let data = epoch::pin(|pin| {
            T::clone(self.inner.load(pin).unwrap())
        });
        RcuGuard {
            lock: self,
            data: data,
            _guard: guard,
        }
    }
}

impl<T> Drop for RcuLock<T> {
    fn drop(&mut self) {
        epoch::pin(|pin| {
            let inner = self.inner.load(pin);
            drop(unsafe { Box::from_raw(inner.as_raw()) });
        })
    }
}

pub struct RcuGuard<'a, T: Clone + Send + 'static> {
    lock: &'a RcuLock<T>,
    data: T,
    _guard: MutexGuard<'a, ()>,
}

impl<'a, T: Clone + Send + 'static> DerefMut for RcuGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.data
    }
}

impl<'a, T: Clone + Send + 'static> Deref for RcuGuard<'a, T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.data
    }
}

/// On drop, atomically store the data back into the owning `RcuLock`.
impl<'a, T: Clone + Send + 'static> Drop for RcuGuard<'a, T> {
    fn drop(&mut self) {
        let data = Box::new(Arc::new(self.data.clone()));
        epoch::pin(|pin| {
            let old_data = self.lock.inner.swap_box(data, 0, pin);
            unsafe {
                self.lock.garbage.defer_drop(old_data.as_raw(), 1, pin);
            }
        });
    }
}
