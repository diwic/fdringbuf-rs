//! This is a fast ringbuffer that tries to avoid memory copies as much as possible.
//! There can be one producer and one consumer, but they can be in different threads
//! i e, they are Send but not Clone.

use std::cmp::min;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::mem::size_of;

pub struct Sender<'a, T: 'a + Copy> {
    index: usize,
    count: &'a AtomicUsize,
    data: &'a mut [T],
}

pub struct Receiver<'a, T: 'a + Copy> {
    index: usize,
    count: &'a AtomicUsize,
    data: &'a mut [T],
}

unsafe impl<'a, T: Copy> Send for Sender<'a, T> {}
unsafe impl<'a, T: Copy> Send for Receiver<'a, T> {}

impl<'a, T: Copy> Sender<'a, T> {

    /// Returns (free items, was empty)
    /// The first item is number of items that can be written to the buffer (until it's full).
    /// The second item is true if the buffer was empty but was written to
    /// (this can be used to signal remote side that more data can be read).
    /// f: This closure returns number of items written to the buffer.
    /// The array sent to the closure is an "out" parameter and contains
    /// garbage data on entering the closure.
    pub fn send<F: FnMut(&mut [T]) -> usize>(&mut self, mut f: F) -> (usize, bool) {
        let l = self.data.len();
        let cb = self.count.load(Ordering::SeqCst);
        let end = self.index + min(l - self.index, l - cb);
        let slice = &mut self.data[self.index .. end];

        let n = if slice.len() == 0 { 0 } else { f(slice) };

        assert!(n <= slice.len());
        let c = self.count.fetch_add(n, Ordering::SeqCst);
        self.index = (self.index + n) % l;
        //println!("Send: cb = {}, c = {}, l = {}, n = {}", cb, c, l, n);
        (l - c - n, c == 0 && n > 0)
    }

    /// Returns number of items that can be written
    pub fn write_count(&self) -> usize { self.data.len() - self.count.load(Ordering::Relaxed) }
}

impl<'a, T: Copy> Receiver<'a, T> {

    /// Returns (remaining items, was full) 
    /// The second item is true if the buffer was full but was read from
    /// (this can be used to signal remote side that more data can be written).
    /// f: This closure returns number of items that can be dropped from buffer.
    pub fn recv<F: FnMut(&[T]) -> usize>(&mut self, mut f: F) -> (usize, bool) {
        let l = self.data.len();
        let cb = self.count.load(Ordering::SeqCst);
        let slice = &self.data[self.index .. min(self.index + cb, l)];

        let n = if slice.len() == 0 { 0 } else { f(slice) };
        assert!(n <= slice.len());
        let c = self.count.fetch_sub(n, Ordering::SeqCst);
        self.index = (self.index + n) % l;
        //println!("Recv: cb = {}, c = {}, l = {}, n = {}", cb, c, l, n);
        return (c - n, c >= l && n > 0)
    }

    /// Returns number of items that can be read
    pub fn read_count(&self) -> usize { self.count.load(Ordering::Relaxed) }
}

/// Create a channel (without signaling)
/// Non-allocating - expects a pre-allocated buffer
pub fn channel<'a, T: Copy>(slice: &'a mut[u8]) -> (Sender<'a, T>, Receiver<'a, T>) {
    use std::mem::transmute_copy;
    assert!(slice.len() >= size_of::<AtomicUsize>() + size_of::<T>(), "Buffer too small");
    unsafe {
        let count: &'a AtomicUsize = &*(slice.as_ptr() as *mut AtomicUsize);
        count.store(0, Ordering::Relaxed);

        let buf = &mut slice[size_of::<AtomicUsize>() ..];
        let data: &'a mut [T] = ::std::slice::from_raw_parts_mut(
            buf.as_mut_ptr() as *mut _,
            buf.len() / size_of::<T>());
        let data2: &'a mut [T] = transmute_copy(&data);
        (Sender { index: 0, count: count, data: data },
         Receiver { index: 0, count: count, data: data2 })
    }
}

/// Use this utility function to figure out how big buffer you need to allocate.
pub fn channel_bufsize<T: Copy>(capacity: usize) -> usize { capacity * size_of::<T>() + size_of::<AtomicUsize>() }

#[cfg(test)]
mod tests {
    extern crate test;

    #[test]
    fn simple_test() {
        let mut q: Vec<u8> = vec![10; 20];
        let (mut s, mut r) = super::channel::<u16>(&mut q);
        // is it empty?
        r.recv(|_| panic!());
        s.send(|d| { d[0] = 5; 1 });
        r.recv(|d| { assert_eq!(d.len(), 1);
            assert_eq!(d[0], 5); 0 });
        r.recv(|d| { assert_eq!(d.len(), 1);
            assert_eq!(d[0], 5); 1 });
        r.recv(|_| panic!());
    }

    #[test]
    fn full_buf_test() {
        let mut q: Vec<u8> = vec![66; super::channel_bufsize::<u16>(3)];
        let (mut s, mut r) = super::channel::<u16>(&mut q);
        s.send(|d| { assert_eq!(d.len(), 3);
           d[0] = 5; d[1] = 8; d[2] = 9; 2 });
        s.send(|d| { assert_eq!(d.len(), 1);
           d[0] = 10; 1 });
        s.send(|_| panic!());
        r.recv(|d| { assert_eq!(d.len(), 3); 0 });
        s.send(|_| panic!());
        r.recv(|d| { assert_eq!(d.len(), 3);
            assert_eq!(d[0], 5);
            assert_eq!(d[1], 8);
            assert_eq!(d[2], 10); 1 });
        s.send(|d| { assert_eq!(d.len(), 1); d[0] = 1; 1 });
        s.send(|_| panic!());
        r.recv(|d| { assert_eq!(d.len(), 2);
            assert_eq!(d[0], 8);
            assert_eq!(d[1], 10); 2 });
        r.recv(|d| { assert_eq!(d.len(), 1);
            assert_eq!(d[0], 1); 1
        });
    }


    #[bench]
    fn buf_send400_recv300_bufsize1024_u32(b: &mut test::Bencher) {
        let mut q: Vec<u8> = vec![0; super::channel_bufsize::<u32>(1024)];
        let (mut s, mut r) = super::channel::<u32>(&mut q);
        let (mut total1, mut total2) = (0u64, 0u64);
        b.iter(|| {
            s.send(|d| {
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
}
