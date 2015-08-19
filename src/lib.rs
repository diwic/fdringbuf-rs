#![cfg_attr(test, feature(test))]

extern crate libc;

#[macro_use]
extern crate log;

pub mod ringbuf;

pub mod fdbuf;

/// Use this utility function to figure out how big u8 buffer you need to allocate for a ringbuf or fdbuf.
pub fn channel_bufsize<T>(capacity: usize) -> usize { ringbuf::channel_bufsize::<T>(capacity) }
