#[macro_use] extern crate failure;
extern crate event;
extern crate orbclient;
extern crate orbfont;
extern crate orbimage;
extern crate syscall;

use event::EventQueue;
use std::{
    env,
    fs::File,
    io,
    iter,
    os::unix::io::{AsRawFd, FromRawFd},
    path::PathBuf,
    process::Command,
};
use syscall::flag::{O_CLOEXEC, O_CREAT, O_NONBLOCK, O_RDWR};

#[derive(Debug, Fail)]
pub enum Error {
    #[fail(display = "io error: {}", _0)]
    IoError(io::Error),
    #[fail(display = "syscall error: {}", _0)]
    SyscallError(syscall::Error),
}
impl From<io::Error> for Error {
    fn from(err: io::Error) -> Self { Error::IoError(err) }
}
impl From<syscall::Error> for Error {
    fn from(err: syscall::Error) -> Self { Error::SyscallError(err) }
}

/// Convenience function for setting DISPLAY and PATH environment variables
pub fn fix_env(display_path: &str) {
    env::set_var("DISPLAY", &display_path);

    let path = env::var("PATH").unwrap_or(String::new());
    let new_path = env::join_paths(
        env::split_paths(&path)
            .chain(iter::once(PathBuf::from("/ui/bin")))
    ).unwrap();
    env::set_var("PATH", new_path);
}

pub struct Orbital {
    pub socket: File,
    pub display: File,

    pub width: i32,
    pub height: i32
}
impl Orbital {
    /// Open an orbital display and connect to the socket
    pub fn open_display(display_path: &str) -> io::Result<Self> {
        let socket = syscall::open(":orbital", O_CREAT | O_CLOEXEC | O_NONBLOCK | O_RDWR)
                        .map(|socket| {
                            // Not that you can actually use this on targets other than redox...
                            // But it's still nice if it would compile.
                            #[cfg(not(target_os = "redox"))]
                            let socket = socket as i32;

                            unsafe { File::from_raw_fd(socket) }
                        })
                        .map_err(|err| io::Error::from_raw_os_error(err.errno))?;

        let display = syscall::open(&display_path, O_CLOEXEC | O_NONBLOCK | O_RDWR)
                        .map(|socket| {
                            // Not that you can actually use this on targets other than redox...
                            // But it's still nice if it would compile.
                            #[cfg(not(target_os = "redox"))]
                            let socket = socket as i32;

                            unsafe { File::from_raw_fd(socket) }
                        })
                        .map_err(|err| io::Error::from_raw_os_error(err.errno))?;

        let display_fd = display.as_raw_fd();

        #[cfg(not(target_os = "redox"))]
        let display_fd = display_fd as usize;

        let mut buf: [u8; 4096] = [0; 4096];
        let count = syscall::fpath(display_fd, &mut buf).unwrap();
        let path = unsafe { String::from_utf8_unchecked(Vec::from(&buf[..count])) };
        let res = path.split(":").nth(1).unwrap_or("");
        let width = res.split("/").nth(1).unwrap_or("").parse::<i32>().unwrap_or(0);
        let height = res.split("/").nth(2).unwrap_or("").parse::<i32>().unwrap_or(0);

        Ok(Orbital {
            socket: socket,
            display: display,

            width: width,
            height: height
        })
    }
    pub fn run<I>(&mut self, login_cmd: &str, args: I) -> Result<(), Error>
        where I: IntoIterator<Item = String>
    {
        Command::new(&login_cmd)
            .args(args)
            .spawn()?;

        syscall::setrens(0, 0)?;

        let mut event_queue = EventQueue::<()>::new()?;

        let socket_fd = self.socket.as_raw_fd();
        let display_fd = self.display.as_raw_fd();

        event_queue.add(display_fd, move |_| -> io::Result<Option<()>> {
            // TODO: handle display events
            Ok(None)
        })?;

        event_queue.add(socket_fd, move |_| -> io::Result<Option<()>> {
            // TODO: handle socket events
            Ok(None)
        })?;

        event_queue.trigger_all(event::Event {
            fd: 0,
            flags: 0,
        })?;
        event_queue.run()?;
        Ok(())
    }
}
