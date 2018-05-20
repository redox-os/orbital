#![deny(warnings)]
#![feature(asm)]
#![feature(const_fn)]

extern crate event;
extern crate orbclient;
extern crate orbimage;
extern crate orbfont;
#[macro_use]
extern crate serde_derive;
extern crate syscall;
extern crate toml;

use event::EventQueue;
use std::env;
use std::cell::RefCell;
use std::fs::File;
use std::io::{Error, Result};
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::path::PathBuf;
use std::process::Command;
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

        env::set_var("DISPLAY", &display_path);

        {
            let path = env::var("PATH").unwrap_or(String::new());
            let mut paths = env::split_paths(&path).collect::<Vec<_>>();
            paths.push(PathBuf::from("file:/ui/bin"));
            let new_path = env::join_paths(paths).unwrap();
            env::set_var("PATH", new_path);
        }

        let socket_res = syscall::open(":orbital", O_CREAT | O_CLOEXEC | O_NONBLOCK | O_RDWR)
                                .map(|socket| unsafe { File::from_raw_fd(socket) })
                                .map_err(|err| Error::from_raw_os_error(err.errno));

        let display_res = syscall::open(&display_path, O_CLOEXEC | O_NONBLOCK | O_RDWR)
                                .map(|socket| unsafe { File::from_raw_fd(socket) })
                                .map_err(|err| Error::from_raw_os_error(err.errno));

        match socket_res {
            Ok(socket) => match display_res {
                Ok(display) => {
                    let socket_fd = socket.as_raw_fd();
                    let display_fd = display.as_raw_fd();

                    let width;
                    let height;
                    {
                        let mut buf: [u8; 4096] = [0; 4096];
                        let count = syscall::fpath(display_fd, &mut buf).unwrap();
                        let path = unsafe { String::from_utf8_unchecked(Vec::from(&buf[..count])) };
                        let res = path.split(":").nth(1).unwrap_or("");
                        width = res.split("/").nth(1).unwrap_or("").parse::<i32>().unwrap_or(0);
                        height = res.split("/").nth(2).unwrap_or("").parse::<i32>().unwrap_or(0);
                    }

                    println!("orbital: found display {}x{}", width, height);

                    let config = Config::from_path("/ui/orbital.toml");

                    let scheme = Rc::new(RefCell::new(OrbitalScheme::new(width, height, socket, display, &config)));

                    let mut event_queue = EventQueue::<()>::new().expect("orbital: failed to create event queue");

                    let mut command = Command::new(&login_cmd);
                    for arg in args {
                        command.arg(&arg);
                    }
                    match command.spawn() {
                        Ok(_child) => (),
                        Err(err) => println!("orbital: failed to launch '{}': {}", login_cmd, err)
                    }

                    syscall::setrens(0, 0).expect("orbital: failed to enter null namespace");

                    let scheme_display = scheme.clone();
                    event_queue.add(display_fd, move |_| -> Result<Option<()>> {
                        scheme_display.borrow_mut().display_event()?;
                        Ok(None)
                    }).expect("orbital: failed to poll display");

                    event_queue.add(socket_fd, move |_| -> Result<Option<()>> {
                        scheme.borrow_mut().scheme_event()?;
                        Ok(None)
                    }).expect("orbital: failed to poll scheme");

                    event_queue.trigger_all(event::Event {
                        fd: 0,
                        flags: 0,
                    }).expect("orbital: failed to trigger event queue");

                    event_queue.run().expect("orbital: failed to run event queue");
                },
                Err(err) => println!("orbital: no display found: {}", err)
            },
            Err(err) => println!("orbital: could not register orbital: {}", err)
        }
    }
}
