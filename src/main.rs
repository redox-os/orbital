#![deny(warnings)]
#![feature(asm)]
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
use orbital::Orbital;
use std::cell::RefCell;
use std::env;
use std::fs::File;
use std::io::{Error, Result};
use std::os::unix::io::{asrawfd, fromrawfd};
use std::path::PathBuf;
use std::rc::Rc;
use syscall::flag::{O_CLOEXEC, O_CREAT, O_NONBLOCK, O_RDWR};

use config::Config;
use scheme::OrbitalScheme;

mod config;
mod image;
mod rect;
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
                    display.socket,
                    display.display,
                    &config
                )));

                display.run(&login_cmd, args).expect("orbital: failed to launch");
            },
            Err(err) => println!("orbital: could not register orbital: {}", err)
        }
    }
}
