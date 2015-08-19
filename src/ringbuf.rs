//! This is a fast ringbuffer that tries to avoid memory copies as much as possible.
//! There can be one producer and one consumer, but they can be in different threads
//! i e, they are Send but not Clone.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::mem::size_of;
use std::ops::DerefMut;

#[allow(raw_pointer_derive)]
#[derive(Copy, Clone)]
struct Buf<T> {
    data: *mut T,
    count_ptr: *const AtomicUsize,
    length: usize,
}

unsafe impl<T> Send for Buf<T> {}

pub struct Sender<T, U> {
    buf: Buf<T>,
    index: usize,
    _owner: Arc<U>,
}

pub struct Receiver<T, U> {
    buf: Buf<T>,
    index: usize,
    _owner: Arc<U>,
}

/// Use this utility function to figure out how big buffer you need to allocate.
pub fn channel_bufsize<T>(capacity: usize) -> usize { capacity * size_of::<T>() + size_of::<AtomicUsize>() }


/// Create a channel (without signaling)
/// Non-allocating - expects a pre-allocated buffer
pub fn channel<T: Send + Copy, U: Send + DerefMut<Target=[u8]>>(buffer: U) -> (Sender<T, U>, Receiver<T, U>) {

    let mut mem = buffer;
    let b = {
        let slice: &mut [u8] = &mut mem;
        assert!(slice.len() >= size_of::<AtomicUsize>() + size_of::<T>(), "Buffer too small");

        Buf {
            count_ptr: slice.as_ptr() as *const AtomicUsize,
            data: unsafe { slice.as_ptr().offset(size_of::<AtomicUsize>() as isize) } as *mut T,
            length: (slice.len() - size_of::<AtomicUsize>()) / size_of::<T>(),
        }
    };
    b.count().store(0, Ordering::Relaxed);

    let o = Arc::new(mem);
    let s = Sender { buf: b, index: 0, _owner: o.clone() };
    let r = Receiver { buf: b, index: 0, _owner: o };
    (s, r)
}

impl<T> Buf<T> {
    #[inline]
    fn count(&self) -> &AtomicUsize { unsafe { &*self.count_ptr }}

    #[inline]
    fn slice(&mut self) -> &mut [T] {
        unsafe { ::std::slice::from_raw_parts_mut(self.data, self.length) }
    }
}

impl<T, U> Sender<T, U> {

    /// Lowest level "send" function
    ///
    /// Returns (free items, was empty)
    /// The first item is number of items that can be written to the buffer (until it's full).
    /// The second item is true if the buffer was empty but was written to
    /// (this can be used to signal remote side that more data can be read).
    /// f: This closure returns number of items written to the buffer.
    ///
    /// The pointer sent to the closure is an "out" parameter and contains
    /// garbage data on entering the closure. (This cannot safely be a &mut [T] because
    /// the closure might then read from uninitialized memory, even though it shouldn't)
    ///
    /// Since this is a ringbuffer, there might be more items to write even if you
    /// completely fill up during the closure.
    pub fn send<F: FnOnce(*mut T, usize) -> usize>(&mut self, f: F) -> (usize, bool) {
        use std::cmp;

        let cb = self.buf.count().load(Ordering::SeqCst);
        let l = self.buf.length;

        let n = {
             let data = self.buf.slice();
             let end = self.index + cmp::min(l - self.index, l - cb);
             let slice = &mut data[self.index .. end];

             let n = if slice.len() == 0 { 0 } else { f(slice.as_mut_ptr(), slice.len()) };

             assert!(n <= slice.len());
             n
        };

        let c = self.buf.count().fetch_add(n, Ordering::SeqCst);
        self.index = (self.index + n) % l;
        trace!("Send: cb = {}, c = {}, l = {}, n = {}", cb, c, l, n);
        (l - c - n, c == 0 && n > 0)
    }

    /// "Safe" version of send. Will call your closure up to "count" times
    /// and depend on RVO to avoid memory copies.
    ///
    /// Returns (free items, was empty) like send does
    pub fn send_foreach<F: FnMut(usize) -> T>(&mut self, count: usize, mut f: F) -> (usize, bool) {
        use std::ptr;

        let mut i = 0;
        self.send(|p, c| {
            while i < c && i < count {
                unsafe { ptr::write(p.offset(i as isize), f(i)) };
                i += 1;
            };
            i
        })
    }

    /// Returns number of items that can be written
    pub fn write_count(&self) -> usize { self.buf.length - self.buf.count().load(Ordering::Relaxed) }
}

impl<T, U> Receiver<T, U> {
    /// Returns (remaining items, was full) 
    /// The second item is true if the buffer was full but was read from
    /// (this can be used to signal remote side that more data can be written).
    /// f: This closure returns number of items that can be dropped from buffer.
    /// Since this is a ringbuffer, there might be more items to read even if you
    /// read it all during the closure.
    pub fn recv<F: FnOnce(&[T]) -> usize>(&mut self, f: F) -> (usize, bool) {
        use std::cmp;

        let cb = self.buf.count().load(Ordering::SeqCst);
        let l = self.buf.length;
        let n = {
            let data: &[T] = self.buf.slice();
            let slice = &data[self.index .. cmp::min(self.index + cb, l)];

            let n = if slice.len() == 0 { 0 } else { f(slice) };
            assert!(n <= slice.len());
            n
        };

        let c = self.buf.count().fetch_sub(n, Ordering::SeqCst);
        self.index = (self.index + n) % l;
        trace!("Recv: cb = {}, c = {}, l = {}, n = {}", cb, c, l, n);
        return (c - n, c >= l && n > 0)
    }

    /// Returns number of items that can be read
    pub fn read_count(&self) -> usize { self.buf.count().load(Ordering::Relaxed) }
}

#[cfg(test)]
mod tests {
    extern crate test;

    #[test]
    fn owner() {
        let mut v = vec![20; 30];
        let v2: &mut[u8] = &mut *v;
        let (_, _) = super::channel::<i64, _>(v2);
    }

    #[test]
    fn simple_test() {
        let (mut s, mut r) = super::channel(vec![10; 20]);
        // is it empty?
        r.recv(|_| panic!());
        s.send(|d, _| { unsafe { *d = 5u16 }; 1 });
        r.recv(|d| { assert_eq!(d.len(), 1);
            assert_eq!(d[0], 5); 0 });
        r.recv(|d| { assert_eq!(d.len(), 1);
            assert_eq!(d[0], 5); 1 });
        r.recv(|_| panic!());

        let mut i = 6;
        s.send_foreach(2, |_| { i += 1; i } );
        r.recv(|d| { assert_eq!(d.len(), 2);
            assert_eq!(d[0], 7);
            assert_eq!(d[1], 8);
            2
        });
    }

    #[test]
    fn full_buf_test() {
        let q: Vec<u8> = vec![66; super::channel_bufsize::<u16>(3)];
        let (mut s, mut r) = super::channel(q);
        s.send(|dd, l| { assert_eq!(l, 3);
            let d = unsafe { ::std::slice::from_raw_parts_mut(dd, l) };
            d[0] = 5u16; d[1] = 8; d[2] = 9;
            2
        });
        let mut called = false;
        s.send_foreach(2, |i| {
            assert_eq!(called, false);
            assert_eq!(i, 0);
            called = true;
            10
        });
        s.send(|_, _| panic!());
        r.recv(|d| { assert_eq!(d.len(), 3); 0 });
        s.send(|_, _| panic!());
        r.recv(|d| { assert_eq!(d.len(), 3);
            assert_eq!(d[0], 5);
            assert_eq!(d[1], 8);
            assert_eq!(d[2], 10); 1 });
        s.send(|d, l| { assert_eq!(l, 1); unsafe { *d = 1 }; 1 });
        s.send(|_, _| panic!());
        r.recv(|d| { assert_eq!(d.len(), 2);
            assert_eq!(d[0], 8);
            assert_eq!(d[1], 10); 2 });
        r.recv(|d| { assert_eq!(d.len(), 1);
            assert_eq!(d[0], 1); 1
        });
    }

    #[bench]
    fn buf_send400_recv300_bufsize1024_u32(b: &mut test::Bencher) {
        let q = vec![0u8; super::channel_bufsize::<u32>(1024)];
        let (mut s, mut r) = super::channel::<u32, _>(q);
        let (mut total1, mut total2) = (0u64, 0u64);
        b.iter(|| {
            s.send(|dd, l| {
                let d = unsafe { ::std::slice::from_raw_parts_mut(dd, l) };
                let mut c = 0;
                for z in d.iter_mut().take(400) { *z = c; total1 += c as u64; c += 1; };
                c as usize
            });
            r.recv(|d| {
                for z in d.iter().take(300) { total2 += *z as u64 };
                ::std::cmp::min(300, d.len())
            });
        });

        r.recv(|d| { for z in d.iter() { total2 += *z as u64 }; d.len() });
        r.recv(|d| { for z in d.iter() { total2 += *z as u64 }; d.len() });

        assert_eq!(total1, total2);
    }

    #[bench]
    fn buf_send_foreach400_recv300_bufsize1024_u32(b: &mut test::Bencher) {
        let q = vec![0u8; super::channel_bufsize::<u32>(1024)];
        let (mut s, mut r) = super::channel::<u32, _>(q);
        let (mut total1, mut total2) = (0u64, 0u64);
        b.iter(|| {
            let mut c = 0;
            s.send_foreach(400, |_| { c += 1; total1 += c as u64; c });
            r.recv(|d| {
                for z in d.iter().take(300) { total2 += *z as u64 };
                ::std::cmp::min(300, d.len())
            });
        });

        r.recv(|d| { for z in d.iter() { total2 += *z as u64 }; d.len() });
        r.recv(|d| { for z in d.iter() { total2 += *z as u64 }; d.len() });

        assert_eq!(total1, total2);
    }

}

