#![feature(libc, std_misc, unsafe_destructor)]
#![cfg_attr(test, feature(test))]

extern crate libc;

pub mod ringbuf;

pub mod fdbuf;
