//! Ringbuffer with signalling via fd:s.
//! You can use it with std::os::Pipe, but do try nix-rust's eventfds
//! for slightly better performance!
//! You will typically integrate with mio so you can wait for many fds at once,
//! hence there are no functions that actually wait, just functions that give out
//! the Fd to wait for.

use std::os::unix::prelude::Fd;
use std::io;
use std::os::Pipe;

use ringbuf::Sender as RSender;
use ringbuf::Receiver as RReceiver;

pub struct Sender<'a, T: 'a + Copy> {
    inner: RSender<'a, T>,
    signal_fd: Fd,
    wait_fd: Fd,
}

pub struct Receiver<'a, T: 'a + Copy> {
    inner: RReceiver<'a, T>,
    signal_fd: Fd,
    wait_fd: Fd,
}

unsafe impl<'a, T: Copy> Send for Sender<'a, T> {}
unsafe impl<'a, T: Copy> Send for Receiver<'a, T> {}

fn write_fd(fd: Fd) -> io::Result<()> {
    let e = unsafe { ::libc::write(fd, &1u64 as *const _ as *const ::libc::c_void, ::std::mem::size_of::<u64>() as u64) };
    //println!("write {} to fd {}", e, fd);
    if e == -1 { return Err(io::Error::last_os_error()) }
    assert!(e > 0);
    Ok(())
}

fn flush_fd(fd: Fd) -> io::Result<()> {
    type Arr = [u64; 32];
    let b: Arr = unsafe { ::std::mem::uninitialized() };
    let e = unsafe { ::libc::read(fd, b.as_ptr() as *mut ::libc::c_void, ::std::mem::size_of::<Arr>() as u64) };
    //println!("read {} from fd {}", e, fd);
    if e == -1 { return Err(io::Error::last_os_error()) }
    assert!(e > 0);
    Ok(())
}


impl<'a, T: Copy> Sender<'a, T> {

    /// Returns number of items that can be written to the buffer (until it's full).
    /// f: This closure returns a tuple of (items written, please call me again).
    /// The array sent to the closure is an "out" parameter and contains
    /// garbage data on entering the closure.
    pub fn send<F: FnMut(&mut [T]) -> (usize, bool)>(&mut self, mut f: F) -> io::Result<usize> {
        let mut r = 0;
        let mut last;
        let mut was_empty = false;
        loop {
            let mut repeat = false;
            let (ll, wempty) = self.inner.send(|buf| {
                let (rr, rep) = f(buf);
                repeat = rep;
                r += rr;
                rr
            });
            last = ll;
            was_empty |= wempty;
            if !repeat { break; }
        }
        if r > 0 && was_empty { try!(write_fd(self.signal_fd)) };
        Ok(last)
    }

    /// Returns fd to wait for, and number of items that can be written
    /// You should only wait for this fd if the number is zero.
    /// The Fd will not change during the lifetime of the sender.
    pub fn wait_status(&self) -> (Fd, usize) {
        (self.wait_fd, self.inner.write_count())
    }

    /// Call this after woken up by the waitfd, or you'll just wake up again.
    /// Note: Might deadlock if you call it when not woken up by the waitfd.
    pub fn wait_clear(&mut self) -> io::Result<()> {
        flush_fd(self.wait_fd)
    }

}

impl<'a, T: Copy> Receiver<'a, T> {

    /// Returns remaining items that can be read.
    /// The second item is true if the buffer was full but read from
    /// (this can be used to signal remote side that more data can be written).
    /// f: This closure returns a tuple of (items written, please call me again).
    pub fn recv<F: FnMut(&[T]) -> (usize, bool)>(&mut self, mut f: F) -> io::Result<usize> {
        let mut r = 0;
        let mut last;
        let mut was_full = false;
        loop {
            let mut repeat = false;
            let (ll, wfull) = self.inner.recv(|buf| {
                let (rr, rep) = f(buf);
                repeat = rep;
                r += rr;
                rr
            });
            last = ll;
            was_full |= wfull;
            if !repeat { break; }
        }

        if r > 0 && was_full { try!(write_fd(self.signal_fd)) };
        Ok(last)
    }

    /// Returns fd to wait for, and number of items that can be read
    /// You should only wait for this fd if the number is zero.
    /// The Fd will not change during the lifetime of the sender.
    pub fn wait_status(&self) -> (Fd, usize) {
        (self.wait_fd, self.inner.read_count())
    }

    /// Call this after woken up by the waitfd, or you'll just wake up again.
    /// Note: Might deadlock if you call it when not woken up by the waitfd.
    pub fn wait_clear(&mut self) -> io::Result<()> {
        flush_fd(self.wait_fd)
    }

}

/*
#[unsafe_destructor]
impl<'a, T: Copy> Drop for Sender<'a, T> {
    fn drop(&mut self) {
        unsafe {
            ::libc::close(self.wait_fd);
            ::libc::close(self.signal_fd);
        }
    }
}

#[unsafe_destructor]
impl<'a, T: Copy> Drop for Receiver<'a, T> {
    fn drop(&mut self) {
        unsafe {
            ::libc::close(self.wait_fd);
            ::libc::close(self.signal_fd);
        }
    }
}
*/

/// Creates a channel with fd signalling.
/// Does not take ownership of the fds - they will not be closed
/// when Sender and Receiver goes out of scope.
pub fn channel<'a, T: Copy>(slice: &'a mut[u8], empty: Pipe, full: Pipe) ->
        (Sender<'a, T>, Receiver<'a, T>) {
    let (s, r) = ::ringbuf::channel(slice);
    (Sender { inner: s, signal_fd: empty.writer, wait_fd: full.reader },
     Receiver { inner: r, signal_fd: full.writer, wait_fd: empty.reader })
}

#[cfg(test)]
mod tests {
    extern crate test;
    extern crate nix;
    use self::nix::sys::epoll::*;
    use ::std::os::unix::prelude::Fd;
    use ::std::os::{self, pipe, Pipe};

    fn make_epoll(fd: Fd) -> Fd {
        let sleep = epoll_create().unwrap();
        let event = EpollEvent { data: 0, events: EPOLLIN };
        epoll_ctl(sleep, EpollOp::EpollCtlAdd, fd, &event).unwrap();
        sleep
    }

    fn wait_epoll(fd: Fd) {
        let mut events = [EpollEvent { data: 0, events: EPOLLIN }];
        assert_eq!(1, epoll_wait(fd, &mut events, 5000).unwrap());
    }

    #[bench]
    fn pipe_send400_recv300_bufsize1024_u32(b: &mut test::Bencher) {
        let (pipe1, pipe2) = unsafe { (pipe().unwrap(), pipe().unwrap()) };
        run400_300_1024_bench(b, pipe1, pipe2);
        unsafe {
            ::libc::close(pipe1.reader);
            ::libc::close(pipe1.writer);
            ::libc::close(pipe2.reader);
            ::libc::close(pipe2.writer);
        }
    }

    #[bench]
    fn eventfd_send400_recv300_bufsize1024_u32(b: &mut test::Bencher) {
        use self::nix::sys::eventfd::*;
        let (efd1, efd2) = (eventfd(0, EFD_CLOEXEC).unwrap(), eventfd(0, EFD_CLOEXEC).unwrap());
        let pipe1 = Pipe { reader: efd1, writer: efd1 };
        let pipe2 = Pipe { reader: efd2, writer: efd2 };
        run400_300_1024_bench(b, pipe1, pipe2);
        unsafe {
            ::libc::close(efd1);
            ::libc::close(efd2);
        }
    }

    fn run400_300_1024_bench(b: &mut test::Bencher, pipe1: os::Pipe, pipe2: os::Pipe) {
        let mut q: Vec<u8> = vec![0; ::ringbuf::channel_bufsize::<u32>(1024)];
        let (mut s, mut r) = super::channel::<u32>(&mut q, pipe1, pipe2);

        let guard = ::std::thread::scoped(move || {
            let mut sum = 0;
            let mut quit = false;
            let waitfd = make_epoll(r.wait_status().0);
            loop {
                let can_recv = r.recv(|d| {
                    let mut cc = 0;
                    for z in d.iter().take(300) { cc += 1; if *z == -1 { quit = true; return (cc, false); } sum += *z as u64 };
                    (cc, true)
                }).unwrap();
                if quit { break; }
                if can_recv == 0 {
                    //println!("Recv wait");
                    wait_epoll(waitfd);
                    r.wait_clear().unwrap();
                };
            };
            unsafe { ::libc::close(waitfd) };
            sum
        });

        let mut total1 = 0;
        let waitfd = make_epoll(s.wait_status().0);
        b.iter(|| {
            let can_send = s.send(|d| {
                let mut c = 0;
                for z in d.iter_mut().take(400) { *z = c; total1 += c as u64; c += 1; };
                (c as usize, false)
            }).unwrap();
            if can_send == 0 {
                 //println!("Send wait");
                 wait_epoll(waitfd);
                 s.wait_clear().unwrap();
            };
        });
        s.send(|d| { d[0] = -1; (1, false) }).unwrap();
        unsafe { ::libc::close(waitfd) };
        let total2 = guard.join();
        assert_eq!(total1, total2);
    }
    
}
