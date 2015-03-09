### Ringbuffer with fd signalling - fast IPC without memory copies!

This is an attempt to make the most efficient, fast, and flexible
[IPC](http://en.wikipedia.org/wiki/Inter-process_communication) on the planet!
One use case could be streaming audio/video between applications, where
you need high performance and low latency.

It's efficient:
 * The code gives you direct access to the ringbuffer memory (no memory copies).
 * No busy waiting (well, unless you want to).

It's fast:
 * No syscalls unless necessary - as long as the buffer is not completely
full or completely empty, there's no need to sleep or signal a wakeup.
 * Just two atomic operations per read and per write.

It's flexible:
 * By using file descriptors for signalling, you can wait on several fds at the same time.
I e, you can have the same thread waiting for more than one buffer, if you wish.
 * You can decide what type of fd you want (eventfd, pipes, or something else).
(If you don't know where to start, use eventfd as that would give best performance.)

It's usable:
 * No allocations - suitable for real-time usage.
 * While primarily designed for Linux, there's no mandatory dependency that
makes it Linux only (except for some benchmarks that only run under Linux).

Limitations:
 * The ringbuffer capacity cannot be changed after creation, and it works only on `Copy` types.
 * The ringbuffer is single producer and single consumer, but the producer and
the consumer can be different threads, and even different processes (if the
ringbuffer points to shared memory).


Other options
-------------

Just sending a few bytes in every message, so the number of memory copies don't matter much?
Then a `std::io::pipe` might be better.

Want something more flexible, with a message bus daemon, signals, RPC, etc, and can live with
some extra overhead? Try the [D-Bus](https://github.com/diwic/dbus-rs).

Usage
-----

First, you need to allocate memory for your buffer in the way you prefer.
Use `ringbuf::channel_size` to figure out how much memory the buffer needs.

Second, decide if you want a `ringbuf::channel` or a `fdbuf::channel` - you probably
want the `fdbuf`, but in case you want to implement the signalling yourself (or just
waste power busy waiting), you can use the `ringbuf` instead.

The sender side can call the `send` method which takes a closure as argument. You will get
a mutable slice to fill with your data. Note that since this is a ringbuffer that avoids
memory copies, the closure might need to be called twice to fill it completely.
The same applies to the receiver side, which calls the `recv` method. Your closure needs to
return how many items the closure has read (for `recv`) or written (for `send`).

If the buffer is empty (and only then), the receiver side will be woken up when data can be read from the
buffer. Similar, if the buffer is full (and only then), the sender side will be woken up when more data
can be written. When woken up (and only then), call the `wait_clear` method to reset the wakeup.
See the fdbuf benchmark for an example of how to receive, send and wait accordingly.

Finally, a hint: [mio](https://github.com/carllerche/mio)'s event loop might be useful here, since it is
based on fd waiting, too.

License
-------

Apache-2.0/MIT

