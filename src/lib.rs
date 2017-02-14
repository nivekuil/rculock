//! A lock that allows for an unlimited number of concurrent readers, which are never blocked.
//! Only one writer can access the resource at a time.
//! # Examples
//! ```
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
extern crate crossbeam;
use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;
use std::ops::{Deref, DerefMut};
use crossbeam::mem::epoch::{self, Atomic, Owned};
use parking_lot::{Mutex, MutexGuard};

unsafe impl<T: Clone> Sync for RcuLock<T> {}
unsafe impl<T: Clone> Send for RcuLock<T> {}
#[derive(Debug)]
pub struct RcuLock<T: Clone> {
    /// The resource protected by the lock, behind an `Atomic` for atomic stores,
    /// and an Arc to hand the resource out to readers without fear of memory leaks.
    inner: Atomic<Arc<T>>,
    /// Mutex to ensure at most one writer to prevent a data race, which will occur
    /// when multiple writers each acquire a copy of the resource protected by the
    /// `RcuLock`, write to it, and then store their individual changes to the master `RcuLock`.
    /// Acquired on `write()` and released when `RcuGuard` is dropped.
    write_lock: Mutex<()>,
}

impl<T: Clone> RcuLock<T> {
    /// Create a new RcuLock.
    pub fn new(target: T) -> RcuLock<T> {
        let inner = Atomic::null();
        let arc = Owned::new(Arc::new(target));
        inner.store(Some(arc), Relaxed);
        RcuLock {
            inner: inner,
            write_lock: Mutex::new(()),
        }
    }

    /// Acquire a read handle to the `RcuLock`.  This operation never blocks.
    pub fn read(&self) -> Arc<T> {
        let epoch = epoch::pin();
        let inner = self.inner.load(Relaxed, &epoch);
        inner.unwrap().deref().deref().clone()
    }

    /// Acquire an exclusive write handle to the `RcuLock`, protected by an `RcuGuard`.
    /// This operation blocks if another `RcuGuard` is currently alive, i.e.
    /// the `RcuLock` has already handed one out to another writer.
    ///
    /// Clones the data protected by the `RcuLock`, which can be expensive.
    pub fn write(&self) -> RcuGuard<T> {
        let guard = self.write_lock.lock();
        let epoch = epoch::pin();
        let data: T = self.inner.load(Relaxed, &epoch).unwrap().deref().deref().deref().clone();
        RcuGuard {
            lock: self,
            data: data,
            _guard: guard,
        }
    }
}

pub struct RcuGuard<'a, T: 'a + Clone> {
    lock: &'a RcuLock<T>,
    data: T,
    _guard: MutexGuard<'a, ()>,
}

impl<'a, T: Clone> DerefMut for RcuGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.data
    }
}

impl<'a, T: Clone> Deref for RcuGuard<'a, T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.data
    }
}

/// On drop, atomically store the data back into the owning `RcuLock`.
impl<'a, T: Clone> Drop for RcuGuard<'a, T> {
    fn drop(&mut self) {
        let data = Owned::new(Arc::new(self.data.clone()));
        self.lock.inner.store(Some(data), Relaxed)
    }
}

#[test]
fn test() {
    // Create a new RcuLock protecting a piece of data, in this case a number (u32).
    let data: RcuLock<u32> = RcuLock::new(5);
    assert_eq!(5, *data.read());
    {
        // The data is cloned and handed to the writer
        let mut guard: RcuGuard<u32> = data.write();
        // RcuGuard implements `Deref` and `DerefMut` for easy access to the data.
        *guard = 4;
        // The writer has changed its copy of the data, but the changes
        // have not yet made it back to the master `RcuLock`.
        assert_eq!(5, *data.read());
    }
    // After the write guard is dropped, the state of the resource
    // as the writer sees it is atomically stored back into the master RcuLock.
    assert_eq!(4, *data.read());
}

#[test]
fn threaded() {
    use std::thread;
    let data = RcuLock::new(5);
    let read = data.read().clone();
    let thread1 = {
        let data = data.read().clone();
        thread::spawn(move || for _ in 0..1000 {
            assert_eq!(5, *data);
            thread::sleep_ms(1);
        })
    };

    let thread2 = thread::spawn(move || {

        for i in 1..1001 {
            let mut guard = data.write();
            *guard -= 1;
            drop(guard);
            assert_eq!(5 - i, *data.read());
            thread::sleep_ms(1);
        }

        assert_eq!(-995, *data.read());
    });

    thread1.join().unwrap();
    thread2.join().unwrap();

    assert_eq!(5, *read);
}

#[test]
fn hashmap() {
    use std::thread;
    use std::collections::HashMap;
    let map = Arc::new(RcuLock::new(HashMap::<i32, i32>::new()));
    map.write().insert(1, 1);
    if let Some(x) = map.read().get(&1) {
        assert_eq!(*x, 1);
    }
    let t = {
        let map = map.clone();
        thread::spawn(move || {
            let mut map = map.write();
            for i in 0..1000 {
                map.insert(i % 10, i);
                thread::sleep_ms(1);
            }
        })
    };

    t.join().unwrap();
    let res = map.read();
    let res = res.get(&9);
    assert_eq!(Some(&999), res);
}

#[test]
fn hashmap_race_condition() {
    use std::thread;
    use std::collections::HashMap;
    let map = Arc::new(RcuLock::new(HashMap::<i32, i32>::new()));
    map.write().insert(1, 1);
    if let Some(x) = map.read().get(&1) {
        assert_eq!(*x, 1);
    }
    // t takes longer than t2 to finish but acquires the write lock first
    let t = {
        let map = map.clone();
        thread::spawn(move || {
            let mut map = map.write();
            for i in 0..1000 {
                map.insert(i % 10, i);
                thread::sleep_ms(2);
            }
        })
    };
    thread::sleep_ms(100);
    let t2 = {
        let map = map.clone();
        thread::spawn(move || {
            let mut map = map.write();
            for i in 0..1000 {
                map.insert(i % 10, i);
                thread::sleep_ms(1);
            }
        })
    };

    t.join().unwrap();
    t2.join().unwrap();
    let res = map.read();
    let res = res.get(&9);
    assert_eq!(Some(&999), res);
}
