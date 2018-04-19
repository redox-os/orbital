//#![deny(warnings)]
#![feature(const_fn)]

extern crate orbital;

extern crate event;
extern crate orbclient;
extern crate orbimage;
extern crate orbfont;
#[macro_use]
extern crate serde_derive;
extern crate syscall;
extern crate toml;

use event::EventQueue;
use orbclient::Event;
use orbital::Orbital;
use std::cell::RefCell;
use std::env;
use std::fs::File;
use std::io::{Error, Result};
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::path::PathBuf;
use std::rc::Rc;
use syscall::flag::{O_CLOEXEC, O_CREAT, O_NONBLOCK, O_RDWR};
use syscall::data::Packet;

use config::Config;
use scheme::OrbitalScheme;

mod config;
mod scheme;
mod theme;
mod window;

fn main() {
    // Daemonize
    if unsafe { syscall::clone(0).unwrap() } == 0 {
        let mut args = env::args().skip(1);

        let display_path = args.next().expect("orbital: no display argument");
        let login_cmd = args.next().expect("orbital: no login manager argument");

        env::set_current_dir("file:").unwrap();

        orbital::fix_env(&display_path);

        let display = Orbital::open_display(&display_path);

        match display {
            Ok(display) => {
                println!("orbital: found display {}x{}", display.width, display.height);
                let config = Config::from_path("/ui/orbital.toml");
                let scheme = Rc::new(RefCell::new(OrbitalScheme::new(
                    display.width,
                    display.height,
                    &config
                )));
                let scheme2 = Rc::clone(&scheme);

                let handle_display = move |orb: &mut Orbital, events: &mut [Event]| {
                    scheme.borrow_mut().with_orbital(orb).display_event(events)
                };
                let handle_socket  = move |orb: &mut Orbital, packets: &mut [Packet]| {
                    scheme2.borrow_mut().with_orbital(orb).scheme_event(packets)
                };

                display.run(&login_cmd, args, handle_display, handle_socket)
                    .expect("orbital: failed to launch");
            },
            Err(err) => println!("orbital: could not register orbital: {}", err)
        }
    }
}
