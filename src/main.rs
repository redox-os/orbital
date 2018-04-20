//#![deny(warnings)]
#![feature(const_fn)]

extern crate orbital_core;

extern crate orbclient;
extern crate orbfont;
#[macro_use]
extern crate serde_derive;
extern crate syscall;
extern crate toml;

use orbital_core::Orbital;
use std::env;

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

        orbital_core::fix_env(&display_path).unwrap();

        let display = Orbital::open_display(&display_path);

        match display {
            Ok(display) => {
                println!("orbital: found display {}x{}", display.width, display.height);
                let config = Config::from_path("/ui/orbital.toml");
                let scheme = OrbitalScheme::new(
                    display.width,
                    display.height,
                    &config
                );

                display.run(&login_cmd, args, scheme).expect("orbital: failed to launch");
            },
            Err(err) => println!("orbital: could not register orbital: {}", err)
        }
    }
}
