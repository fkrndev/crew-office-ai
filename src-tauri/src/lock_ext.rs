//! Poison-tolerant locking.
//!
//! The release profile unwinds on panic (no `panic = "abort"`), so a panic in
//! one thread no longer kills the process — but a panic while a std `Mutex` /
//! `RwLock` guard is held POISONS the lock, and a bare `.lock().unwrap()` would
//! then panic on every later use, turning one stray panic into a cascade that
//! takes down every thread touching that state. These extension traits keep the
//! hive alive instead (the upstream Electron app behaves the same way via
//! `process.on('uncaughtException')`): a poisoned lock is recovered with
//! `into_inner()`, the poison flag is cleared, and the recovery is logged once
//! per poisoning event. The protected data may be mid-update — exactly what the
//! panicked writer left behind — which for this app's plain-data state is
//! preferable to a permanently wedged process.
//!
//! std-only: no `parking_lot` (whose locks never poison) dependency.

use std::sync::{Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};

#[cold]
fn note_recovered(kind: &str, ty: &str) {
    crate::panic_guard::diag(
        log::Level::Warn,
        &format!("recovered a poisoned {kind} ({ty}): a thread panicked while holding it; continuing with its inner data"),
    );
}

/// `lock_safe()` for `std::sync::Mutex`.
pub trait LockExt<T: ?Sized> {
    /// Like `lock().unwrap()` but never panics on poison: the guard is
    /// recovered, the poison flag cleared, and a warning logged.
    fn lock_safe(&self) -> MutexGuard<'_, T>;
}

impl<T: ?Sized> LockExt<T> for Mutex<T> {
    fn lock_safe(&self) -> MutexGuard<'_, T> {
        self.lock().unwrap_or_else(|e| {
            let g = e.into_inner();
            // Later users get a healthy lock (and no repeated log line).
            self.clear_poison();
            note_recovered("Mutex", std::any::type_name::<T>());
            g
        })
    }
}

/// `read_safe()` / `write_safe()` for `std::sync::RwLock`.
pub trait RwLockExt<T: ?Sized> {
    fn read_safe(&self) -> RwLockReadGuard<'_, T>;
    fn write_safe(&self) -> RwLockWriteGuard<'_, T>;
}

impl<T: ?Sized> RwLockExt<T> for RwLock<T> {
    fn read_safe(&self) -> RwLockReadGuard<'_, T> {
        self.read().unwrap_or_else(|e| {
            let g = e.into_inner();
            self.clear_poison();
            note_recovered("RwLock (read)", std::any::type_name::<T>());
            g
        })
    }

    fn write_safe(&self) -> RwLockWriteGuard<'_, T> {
        self.write().unwrap_or_else(|e| {
            let g = e.into_inner();
            self.clear_poison();
            note_recovered("RwLock (write)", std::any::type_name::<T>());
            g
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn poisoned_mutex() -> Arc<Mutex<Vec<i32>>> {
        let m = Arc::new(Mutex::new(vec![1, 2]));
        let m2 = m.clone();
        let _ = std::thread::spawn(move || {
            let mut g = m2.lock().unwrap();
            g.push(3);
            panic!("poison the mutex on purpose");
        })
        .join();
        assert!(m.is_poisoned(), "precondition: the mutex is poisoned");
        m
    }

    #[test]
    fn lock_safe_recovers_a_poisoned_mutex_and_keeps_the_data() {
        let m = poisoned_mutex();
        assert!(m.lock().is_err());
        {
            let mut g = m.lock_safe();
            assert_eq!(*g, vec![1, 2, 3], "the panicked writer's partial update is kept");
            g.push(4);
        }
        // Poison cleared: plain lock() works again and sees the new data.
        assert!(!m.is_poisoned());
        assert_eq!(*m.lock().unwrap(), vec![1, 2, 3, 4]);
    }

    #[test]
    fn lock_safe_on_a_healthy_mutex_is_a_plain_lock() {
        let m = Mutex::new(5u32);
        *m.lock_safe() += 1;
        assert_eq!(*m.lock_safe(), 6);
    }

    #[test]
    fn rwlock_read_and_write_safe_recover_from_poison() {
        let l = Arc::new(RwLock::new(String::from("a")));
        let l2 = l.clone();
        let _ = std::thread::spawn(move || {
            let mut g = l2.write().unwrap();
            g.push('b');
            panic!("poison the rwlock on purpose");
        })
        .join();
        assert!(l.is_poisoned());
        assert_eq!(&*l.read_safe(), "ab");
        assert!(!l.is_poisoned());

        // Poison again, this time recover through write_safe.
        let l4 = l.clone();
        let _ = std::thread::spawn(move || {
            let _g = l4.write().unwrap();
            panic!("again");
        })
        .join();
        assert!(l.is_poisoned());
        l.write_safe().push('c');
        assert_eq!(&*l.read_safe(), "abc");
    }

    #[test]
    fn a_poisoned_lock_does_not_cascade_across_threads() {
        let m = poisoned_mutex();
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let m = m.clone();
                std::thread::spawn(move || {
                    m.lock_safe().push(9);
                })
            })
            .collect();
        for h in handles {
            h.join().expect("no thread may panic on the recovered lock");
        }
        assert_eq!(m.lock_safe().len(), 3 + 4);
    }
}
