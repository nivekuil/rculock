extern crate rculock;
use std::sync::Arc;
use rculock::{RcuLock, RcuGuard};

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
            for i in 0..1200 {
                map.insert(i % 10, i);
                thread::sleep_ms(1);
            }
        })
    };

    t.join().unwrap();
    t2.join().unwrap();
    let res = map.read();
    let res = res.get(&9);
    assert_eq!(Some(&1199), res);
}
