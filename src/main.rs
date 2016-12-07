#![deny(warnings)]
#![feature(asm)]
#![feature(const_fn)]

extern crate core;
extern crate orbclient;
extern crate orbimage;
extern crate syscall;

use orbclient::event::Event;
use std::{env, mem, slice, str, thread};
use std::os::unix::io::AsRawFd;
use std::process::Command;
use std::sync::{Arc, Mutex};
use syscall::data::Packet;
use syscall::number::SYS_READ;
use syscall::scheme::SchemeMut;

use config::Config;
use scheme::OrbitalScheme;
use socket::Socket;

mod config;
mod font;
mod image;
mod rect;
mod scheme;
mod socket;
mod window;

fn event_loop(scheme_mutex: Arc<Mutex<OrbitalScheme>>, display: Arc<Socket>, socket: Arc<Socket>){
    loop {
        {
            let mut scheme = scheme_mutex.lock().unwrap();
            scheme.redraw(&display);
        }

        let mut events = [Event::new(); 128];
        let count = display.receive_type(&mut events).unwrap();
        {
            let mut scheme = scheme_mutex.lock().unwrap();
            for &event in events[.. count].iter() {
                scheme.event(event);
            }

            let mut i = 0;
            while i < scheme.todo.len() {
                let mut packet = scheme.todo[i].clone();

                let delay = if packet.a == SYS_READ {
                    if let Some(window) = scheme.windows.get(&packet.b) {
                        window.async == false
                    } else {
                        true
                    }
                } else {
                    false
                };

                scheme.handle(&mut packet);

                if delay && packet.a == 0 {
                    i += 1;
                }else{
                    scheme.todo.remove(i);
                    socket.send(&packet).unwrap();
                }
            }

            for (id, window) in scheme.windows.iter() {
                if ! window.events.is_empty() {
                    socket.send(&Packet {
                        id: 0,
                        pid: 0,
                        uid: 0,
                        gid: 0,
                        a: syscall::number::SYS_FEVENT,
                        b: *id,
                        c: syscall::flag::EVENT_READ,
                        d: window.events.len() * mem::size_of::<Event>()
                    }).unwrap();
                }
            }
        }
    }
}

fn server_loop(scheme_mutex: Arc<Mutex<OrbitalScheme>>, display: Arc<Socket>, socket: Arc<Socket>){
    loop {
        {
            let mut scheme = scheme_mutex.lock().unwrap();
            scheme.redraw(&display);
        }

        let mut packets = [Packet::default(); 128];
        let count = socket.receive_type(&mut packets).unwrap();
        {
            let mut scheme = scheme_mutex.lock().unwrap();
            for mut packet in packets[.. count].iter_mut() {
                let delay = if packet.a == SYS_READ {
                    if let Some(window) = scheme.windows.get(&packet.b) {
                        window.async == false
                    } else {
                        true
                    }
                } else {
                    false
                };

                scheme.handle(packet);

                if delay && packet.a == 0 {
                    scheme.todo.push(*packet);
                } else {
                    socket.send(&packet).unwrap();
                }
            }

            for (id, window) in scheme.windows.iter() {
                if ! window.events.is_empty() {
                    socket.send(&Packet {
                        id: 0,
                        pid: 0,
                        uid: 0,
                        gid: 0,
                        a: syscall::number::SYS_FEVENT,
                        b: *id,
                        c: syscall::flag::EVENT_READ,
                        d: window.events.len() * mem::size_of::<Event>()
                    }).unwrap();
                }
            }
        }
    }
}

fn main() {
    // Daemonize
    if unsafe { syscall::clone(0).unwrap() } == 0 {
        let mut args = env::args().skip(1);

        let display_path = args.next().expect("orbital: no display argument");
        let login_cmd = args.next().expect("orbital: no login manager argument");

        env::set_current_dir("file:").unwrap();

        env::set_var("DISPLAY", &display_path);

        match Socket::create(":orbital").map(|socket| Arc::new(socket)) {
            Ok(socket) => match Socket::open(&display_path).map(|display| Arc::new(display)) {
                Ok(display) => {
                    let mut buf: [u8; 4096] = [0; 4096];
                    let count = syscall::fpath(display.as_raw_fd() as usize, &mut buf).unwrap();
                    let path = unsafe { String::from_utf8_unchecked(Vec::from(&buf[..count])) };
                    let res = path.split(":").nth(1).unwrap_or("");
                    let width = res.split("/").nth(1).unwrap_or("").parse::<i32>().unwrap_or(0);
                    let height = res.split("/").nth(2).unwrap_or("").parse::<i32>().unwrap_or(0);

                    println!("orbital: found display {}x{}", width, height);

                    let display_ptr = unsafe { syscall::fmap(display.as_raw_fd(), 0, (width * height * 4) as usize).unwrap() };
                    let display_slice = unsafe { slice::from_raw_parts_mut(display_ptr as *mut u32, (width * height) as usize) };
                    println!("orbital: mapped display to {:X}", display_ptr);

                    let config = Config::from_path("/etc/orbital.conf");

                    let scheme = Arc::new(Mutex::new(OrbitalScheme::new(width, height, display_slice, &config)));

                    let mut command = Command::new(&login_cmd);
                    for arg in args {
                        command.arg(&arg);
                    }
                    match command.spawn() {
                        Ok(_child) => (),
                        Err(err) => println!("orbital: failed to launch '{}': {}", login_cmd, err)
                    }

                    let scheme_event = scheme.clone();
                    let display_event = display.clone();
                    let socket_event = socket.clone();

                    let event_thread = thread::spawn(move || {
                        event_loop(scheme_event, display_event, socket_event);
                    });

                    server_loop(scheme, display, socket);

                    let _ = event_thread.join();

                    unsafe { let _ = syscall::funmap(display_ptr); }
                },
                Err(err) => println!("orbital: no display found: {}", err)
            },
            Err(err) => println!("orbital: could not register orbital: {}", err)
        }
    }
}
