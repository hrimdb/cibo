extern crate crossbeam_epoch as epoch;
extern crate crossbeam_utils as utils;

use std::fmt;
use std::marker::PhantomData;
use std::mem;
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{self, AtomicIsize};
use std::sync::atomic::Ordering::{Acquire, Relaxed, Release, SeqCst};

use epoch::{Atomic, Owned};
use utils::cache_padded::CachePadded;
/// Minimum buffer capacity for a deque.
const DEFAULT_MIN_CAP: usize = 16;

/// If a buffer of at least this size is retired, thread-local garbage is flushed so that it gets
/// deallocated as soon as possible.
const FLUSH_THRESHOLD_BYTES: usize = 1 << 10;

/// Possible outcomes of a steal operation.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Copy, Clone)]
pub enum Steal<T> {
    /// The deque was empty at the time of stealing.
    Empty,

    /// Some data has been successfully stolen.
    Data(T),

    /// Lost the race for stealing data to another concurrent operation. Try again.
    Retry,
}

/// A buffer that holds elements in a deque.
struct Buffer<T> {
    /// Pointer to the allocated memory.
    ptr: *mut T,

    /// Capacity of the buffer. Always a power of two.
    cap: usize,
}

unsafe impl<T> Send for Buffer<T> {}

impl<T> Buffer<T> {
    /// Returns a new buffer with the specified capacity.
    fn new(cap: usize) -> Self {
        debug_assert_eq!(cap, cap.next_power_of_two());

        let mut v = Vec::with_capacity(cap);
        let ptr = v.as_mut_ptr();
        mem::forget(v);

        Buffer { ptr, cap }
    }

    /// Returns a pointer to the element at the specified `index`.
    unsafe fn at(&self, index: isize) -> *mut T {
        // `self.cap` is always a power of two.
        self.ptr.offset(index & (self.cap - 1) as isize)
    }

    /// Writes `value` into the specified `index`.
    unsafe fn write(&self, index: isize, value: T) {
        ptr::write(self.at(index), value)
    }

    /// Reads a value from the specified `index`.
    unsafe fn read(&self, index: isize) -> T {
        ptr::read(self.at(index))
    }
}

impl<T> Drop for Buffer<T> {
    fn drop(&mut self) {
        unsafe {
            drop(Vec::from_raw_parts(self.ptr, 0, self.cap));
        }
    }
}

/// Internal data that is shared between the deque and its stealers.
struct Inner<T> {
    /// The bottom index.
    bottom: AtomicIsize,

    /// The top index.
    top: AtomicIsize,

    /// The underlying buffer.
    buffer: Atomic<Buffer<T>>,

    /// Minimum capacity of the buffer. Always a power of two.
    min_cap: usize,
}

impl<T> Inner<T> {
    /// Returns a new `Inner` with default minimum capacity.
    fn new() -> Self {
        Self::with_min_capacity(DEFAULT_MIN_CAP)
    }

    /// Returns a new `Inner` with minimum capacity of `min_cap` rounded to the next power of two.
    fn with_min_capacity(min_cap: usize) -> Self {
        let power = min_cap.next_power_of_two();
        assert!(power >= min_cap, "capacity too large: {}", min_cap);
        Inner {
            bottom: AtomicIsize::new(0),
            top: AtomicIsize::new(0),
            buffer: Atomic::new(Buffer::new(power)),
            min_cap: power,
        }
    }

    /// Resizes the internal buffer to the new capacity of `new_cap`.
    #[cold]
    unsafe fn resize(&self, new_cap: usize) {
        // Load the bottom, top, and buffer.
        let b = self.bottom.load(Relaxed);
        let t = self.top.load(Relaxed);

        let buffer = self.buffer.load(Relaxed, epoch::unprotected());

        // Allocate a new buffer.
        let new = Buffer::new(new_cap);

        // Copy data from the old buffer to the new one.
        let mut i = t;
        while i != b {
            ptr::copy_nonoverlapping(buffer.deref().at(i), new.at(i), 1);
            i = i.wrapping_add(1);
        }

        let guard = &epoch::pin();

        // Replace the old buffer with the new one.
        let old = self.buffer
            .swap(Owned::new(new).into_shared(guard), Release, guard);

        // Destroy the old buffer later.
        guard.defer(move || old.into_owned());

        // If the buffer is very large, then flush the thread-local garbage in order to
        // deallocate it as soon as possible.
        if mem::size_of::<T>() * new_cap >= FLUSH_THRESHOLD_BYTES {
            guard.flush();
        }
    }
}

impl<T> Drop for Inner<T> {
    fn drop(&mut self) {
        // Load the bottom, top, and buffer.
        let b = self.bottom.load(Relaxed);
        let t = self.top.load(Relaxed);

        unsafe {
            let buffer = self.buffer.load(Relaxed, epoch::unprotected());

            // Go through the buffer from top to bottom and drop all elements in the deque.
            let mut i = t;
            while i != b {
                ptr::drop_in_place(buffer.deref().at(i));
                i = i.wrapping_add(1);
            }

            // Free the memory allocated by the buffer.
            drop(buffer.into_owned());
        }
    }
}

pub struct Deque<T> {
    inner: Arc<CachePadded<Inner<T>>>,
    _marker: PhantomData<*mut ()>, // !Send + !Sync
}

unsafe impl<T: Send> Send for Deque<T> {}

impl<T: Default> Deque<T> {
    pub fn new() -> Deque<T> {
        Deque {
            inner: Arc::new(CachePadded::new(Inner::new())),
            _marker: PhantomData,
        }
    }

    pub fn with_min_capacity(min_cap: usize) -> Deque<T> {
        Deque {
            inner: Arc::new(CachePadded::new(Inner::with_min_capacity(min_cap))),
            _marker: PhantomData,
        }
    }

    pub fn len(&self) -> usize {
        let b = self.inner.bottom.load(Relaxed);
        let t = self.inner.top.load(Relaxed);
        b.wrapping_sub(t) as usize
    }

    pub fn set(&self, key: isize, value: T) {
        // Load the bottom, top, and buffer. The buffer doesn't have to be epoch-protected
        // because the current thread (the worker) is the only one that grows and shrinks it.
        let b = self.inner.bottom.load(Relaxed);
        let t = self.inner.top.load(Acquire);
        unsafe {
            let mut buffer = self.inner.buffer.load(Relaxed, epoch::unprotected());

            // Calculate the length of the deque.
            let len = b.wrapping_sub(t);

            // Is the deque full?
            let cap = buffer.deref().cap;
            if len >= cap as isize {
                // Yes. Grow the underlying buffer.
                self.inner.resize(2 * cap);
                buffer = self.inner.buffer.load(Relaxed, epoch::unprotected());
            }
            // Write `value` into the right slot and increment `b`.
            buffer.deref().write(key, value);
            atomic::fence(Release);
            self.inner.bottom.store(b.wrapping_add(1), Relaxed);
        }
    }

    pub fn get(&self, key: isize) -> Option<T> {
        // Load the bottom.
        let b = self.inner.bottom.load(Relaxed);

        // If the deque is empty, return early without incurring the cost of a SeqCst fence.
        let t = self.inner.top.load(Relaxed);
        if b.wrapping_sub(t) <= 0 {
            return None;
        }

        // Load the buffer. The buffer doesn't have to be epoch-protected because the current
        // thread (the worker) is the only one that grows and shrinks it.
        let buf = unsafe { self.inner.buffer.load(Relaxed, epoch::unprotected()) };

        atomic::fence(SeqCst);
        // Load the top.
        let t = self.inner.top.load(Relaxed);

        // Compute the length after the bottom was decremented.
        let len = b.wrapping_sub(t);

        if len <= 0 {
            None
        } else {
            let value = unsafe { Some(buf.deref().read(key)) };
            value
        }
    }

    pub fn remove(&self, key: isize) -> bool {
        let b = self.inner.bottom.load(Relaxed);

        let t = self.inner.top.load(Relaxed);
        if b.wrapping_sub(t) <= 0 {
            return false;
        }

        let buf = unsafe { self.inner.buffer.load(Relaxed, epoch::unprotected()) };

        atomic::fence(SeqCst);
        // Load the top.
        let t = self.inner.top.load(Relaxed);

        // Compute the length after the bottom was decremented.
        let len = b.wrapping_sub(t);

        if len <= 0 {
            return false;
        } else {
            unsafe {
                buf.deref().write(key, Default::default());
            }
            true
        }
    }
}
