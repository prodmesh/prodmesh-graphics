//! A single-producer, single-consumer ring of interleaved samples.
//!
//! # Why this is hand-written and lock-free
//!
//! The consumer is a host audio callback — CoreAudio's IOProc, WASAPI's render
//! thread — running at real-time priority with a hard deadline. **It may not
//! allocate and it may not wait.** A `Mutex` here is not a performance
//! question: if the render thread is preempted while holding it, the callback
//! blocks, misses its deadline, and the plant hears a click. That is priority
//! inversion, and it is the classic way audio code goes wrong.
//!
//! So: a fixed allocation made once at open, two atomics, and no branch that
//! can block. The producer only advances `write`; the consumer only advances
//! `read`. Neither ever moves the other's cursor, which is what makes the pair
//! safe without a lock.

use std::sync::atomic::{AtomicUsize, Ordering};

pub struct Ring {
    /// Fixed at open. `UnsafeCell` because the two ends touch disjoint regions
    /// — the cursors are what prove that, not the borrow checker.
    slots: Box<[std::cell::UnsafeCell<i32>]>,
    mask: usize,
    write: AtomicUsize,
    read: AtomicUsize,
}

// The cursors make the disjointness true: a producer writes only where the
// consumer is not, and vice versa.
unsafe impl Sync for Ring {}
unsafe impl Send for Ring {}

impl Ring {
    /// `capacity` is rounded up to a power of two, so the wrap is a mask rather
    /// than a modulo — the callback does this per sample.
    pub fn new(capacity: usize) -> Self {
        let cap = capacity.next_power_of_two().max(2);
        let mut slots = Vec::with_capacity(cap);
        slots.resize_with(cap, || std::cell::UnsafeCell::new(0));
        Self {
            slots: slots.into_boxed_slice(),
            mask: cap - 1,
            write: AtomicUsize::new(0),
            read: AtomicUsize::new(0),
        }
    }

    pub fn capacity(&self) -> usize {
        self.slots.len()
    }

    /// Samples available to the consumer.
    pub fn len(&self) -> usize {
        self.write.load(Ordering::Acquire).wrapping_sub(self.read.load(Ordering::Acquire))
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Push what fits, and report what did NOT.
    ///
    /// A full ring means the device is consuming slower than the video clock is
    /// producing — real, and something `AudioSync` corrects. Dropping the tail
    /// is the honest failure: blocking here would stall the render thread, and
    /// a late video frame is worse than a short audio buffer.
    pub fn push(&self, samples: &[i32]) -> usize {
        let w = self.write.load(Ordering::Relaxed);
        let free = self.capacity() - w.wrapping_sub(self.read.load(Ordering::Acquire));
        let n = samples.len().min(free);
        for (i, s) in samples[..n].iter().enumerate() {
            // SAFETY: within `free` slots the consumer has already passed.
            unsafe { *self.slots[(w.wrapping_add(i)) & self.mask].get() = *s }
        }
        self.write.store(w.wrapping_add(n), Ordering::Release);
        samples.len() - n
    }

    /// Fill `out` from the ring, and report how many samples were MISSING.
    ///
    /// A short read is an underrun. The remainder is left as the caller
    /// prepared it — silence — because a callback that returns early leaves the
    /// driver playing whatever was in the buffer last, which is worse than a
    /// gap: it is a loop of the previous milliseconds.
    pub fn pop_into(&self, out: &mut [i32]) -> usize {
        let r = self.read.load(Ordering::Relaxed);
        let avail = self.write.load(Ordering::Acquire).wrapping_sub(r);
        let n = out.len().min(avail);
        for (i, slot) in out[..n].iter_mut().enumerate() {
            // SAFETY: within `avail` slots the producer has already released.
            *slot = unsafe { *self.slots[(r.wrapping_add(i)) & self.mask].get() };
        }
        self.read.store(r.wrapping_add(n), Ordering::Release);
        out.len() - n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_round_trips_in_order() {
        let r = Ring::new(8);
        assert_eq!(r.push(&[1, 2, 3]), 0);
        let mut out = [0i32; 3];
        assert_eq!(r.pop_into(&mut out), 0);
        assert_eq!(out, [1, 2, 3]);
        assert!(r.is_empty());
    }

    #[test]
    fn a_full_ring_drops_the_tail_rather_than_blocking() {
        let r = Ring::new(4);
        // A stalled device must not stall the render thread: a late video frame
        // is worse than a short audio buffer.
        assert_eq!(r.push(&[1, 2, 3, 4, 5, 6]), 2);
        assert_eq!(r.len(), 4);
    }

    #[test]
    fn a_short_read_reports_the_gap_and_leaves_the_rest_untouched() {
        let r = Ring::new(8);
        r.push(&[7, 7]);
        let mut out = [-1i32; 5];
        // The callback pre-fills silence; what it does NOT get back stays
        // silent rather than replaying the last milliseconds.
        assert_eq!(r.pop_into(&mut out), 3);
        assert_eq!(out, [7, 7, -1, -1, -1]);
    }

    #[test]
    fn it_wraps_without_losing_order() {
        let r = Ring::new(4);
        let mut out = [0i32; 3];
        for round in 0..100 {
            let base = round * 3;
            r.push(&[base, base + 1, base + 2]);
            r.pop_into(&mut out);
            assert_eq!(out, [base, base + 1, base + 2], "round {round}");
        }
    }

    #[test]
    fn the_two_ends_can_run_at_once() {
        use std::sync::Arc;
        let r = Arc::new(Ring::new(1 << 12));
        let producer = {
            let r = Arc::clone(&r);
            std::thread::spawn(move || {
                let mut sent = 0i32;
                while sent < 100_000 {
                    let batch: Vec<i32> = (sent..sent + 64).collect();
                    let dropped = r.push(&batch);
                    sent += 64 - dropped as i32;
                    if dropped > 0 {
                        std::thread::yield_now();
                    }
                }
            })
        };
        let mut expect = 0i32;
        let mut buf = [0i32; 64];
        while expect < 100_000 {
            let missing = r.pop_into(&mut buf);
            for s in &buf[..64 - missing] {
                assert_eq!(*s, expect, "samples arrived out of order");
                expect += 1;
            }
        }
        producer.join().unwrap();
    }
}
