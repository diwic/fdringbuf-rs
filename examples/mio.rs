extern crate nix;
extern crate mio;
extern crate fdringbuf;
use fdringbuf::fdbuf;

#[derive(Copy, Clone, Debug)]
enum Protocol {
    Error,
    Hello(i32),
    Data(u8),
    Goodbye,
}


fn send_data<U>(i: i32, mut s: fdbuf::Sender<Protocol, U>) {
    use std::thread;

    s.send_foreach(1, || Protocol::Hello(i)).unwrap();
    thread::sleep_ms(100);
    let mut dummy = 0;
    s.send_foreach(6, || { dummy += 1; Protocol::Data(dummy) }).unwrap();

    thread::sleep_ms(100);
    s.send_foreach(1, || Protocol::Goodbye).unwrap();
}

struct MyReceiver(Vec<fdbuf::Receiver<Protocol, Vec<u8>>>, Vec<mio::Io>, i32);

impl mio::Handler for MyReceiver {
    type Timeout = ();
    type Message = ();

    fn ready(&mut self, eloop: &mut mio::EventLoop<MyReceiver>, token: mio::Token, _: mio::EventSet) {
        let mut goodbye = false;
        {
            let r = &mut self.0[token.0];
            r.recv(|d| {
                for dd in d {
                    println!("Receiving {:?} from thread {}", dd, token.0);
                    if let &Protocol::Goodbye = dd { goodbye = true; }
                }
                (d.len(), false)
            }).unwrap();
        }
        if goodbye {
            self.2 += 1;
            if self.2 == self.0.len() as i32 { eloop.shutdown() }
        }
    }    
}

fn main() {
    let mut eloop = mio::EventLoop::new().unwrap();

    let mut rvec = MyReceiver(vec![], vec![], 0);
    for i in 0..8 {
        use self::nix::sys::eventfd::*;
        let (efd1, efd2) = (eventfd(0, EFD_CLOEXEC).unwrap(), eventfd(0, EFD_CLOEXEC).unwrap());
        let pipe1 = fdbuf::Pipe { reader: efd1, writer: efd1 };
        let pipe2 = fdbuf::Pipe { reader: efd2, writer: efd2 };
        let buf = vec![0u8; fdringbuf::channel_bufsize::<Protocol>(64)];

        let (s, r) = fdbuf::channel(buf, pipe1, pipe2);
        std::thread::spawn(move || { send_data(i, s) });
        rvec.0.push(r);
        for fd in [efd1, efd2].into_iter() {
            let mioio = mio::Io::from_raw_fd(*fd);
            eloop.register(&mioio, mio::Token(i as usize)).unwrap();
            rvec.1.push(mioio);
        }
    }

    eloop.run(&mut rvec).unwrap();
}
