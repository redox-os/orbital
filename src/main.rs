//#![deny(warnings)]

extern crate orbital_core;

extern crate env_logger;
extern crate log;
extern crate orbclient;
extern crate orbfont;
#[macro_use]
extern crate serde_derive;
extern crate syscall;
extern crate toml;

use orbital_core::Orbital;
use std::{
    env,
    process::Command
};

use config::Config;
use scheme::OrbitalScheme;

mod config;
mod scheme;
mod theme;
mod window;

fn main() {
    // Daemonize
    if unsafe { syscall::clone(syscall::CloneFlags::empty()).unwrap() } == 0 {
        env_logger::builder()
            .filter_level(log::LevelFilter::Debug)
            .parse_default_env()
            .init();

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

                Command::new(&login_cmd)
                    .args(args)
                    .spawn()
                    .expect("orbital: failed to launch login cmd");

                display.run(scheme).expect("orbital: failed to run main loop");
            },
            Err(err) => println!("orbital: could not register orbital: {}", err)
        }
    }
}
