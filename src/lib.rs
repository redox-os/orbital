#![feature(asm)]

#[macro_use] extern crate failure;
extern crate event;
extern crate orbclient;
extern crate orbfont;
extern crate orbimage;
extern crate syscall;

pub mod rect;
pub mod image;

use event::EventQueue;
use image::{Image, ImageRef};
use orbclient::{Color, Event, EventOption, Renderer};
use rect::Rect;
use std::{
    cell::RefCell,
    env,
    fs::File,
    io::{self, Read, Write},
    iter,
    mem,
    os::unix::io::{AsRawFd, FromRawFd},
    path::PathBuf,
    process::Command,
    rc::Rc,
    slice,
};
use syscall::data::Packet;
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

unsafe fn read_to_slice<R: Read, T: Copy>(mut r: R, buf: &mut [T]) -> io::Result<usize> {
    r.read(slice::from_raw_parts_mut(
        buf.as_mut_ptr() as *mut u8,
        buf.len() * mem::size_of::<T>())
    ).map(|count| count/mem::size_of::<T>())
}
unsafe fn display_fd_map(width: i32, height: i32, display_fd: usize) -> ImageRef<'static> {
    let display_ptr = syscall::fmap(display_fd, 0, (width * height * 4) as usize).unwrap();
    let display_slice = slice::from_raw_parts_mut(display_ptr as *mut Color, (width * height) as usize);
    ImageRef::from_data(width, height, display_slice)
}

unsafe fn display_fd_unmap(image: &mut ImageRef) {
    let _ = syscall::funmap(image.data().as_ptr() as usize);
}

pub struct Orbital {
    pub socket: File,
    pub display: File,
    pub image: ImageRef<'static>,

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

        let image = unsafe { display_fd_map(width, height, display_fd) };

        Ok(Orbital {
            socket: socket,
            display: display,
            image: image,

            width: width,
            height: height
        })
    }
    pub fn display_send(&mut self, event: &Event) -> io::Result<()> {
        self.display.write_all(event)
    }
    pub fn display_sync(&mut self) -> io::Result<()> {
        self.display.sync_all()
    }
    pub fn socket_send(&mut self, packet: &Packet) -> io::Result<()> {
        self.socket.write_all(packet)
    }
    pub fn screen_rect(&self) -> Rect {
        Rect::new(0, 0, self.image.width(), self.image.height())
    }
    /// Start the main loop
    pub fn run<I, HD, HS>(self, login_cmd: &str, args: I, mut handle_display: HD, mut handle_socket: HS) -> Result<(), Error>
        where I: IntoIterator<Item = String>,
              HD: FnMut(&mut Orbital, &mut [Event]) -> io::Result<()> + 'static,
              HS: FnMut(&mut Orbital, &mut [Packet]) -> io::Result<()> + 'static
    {
        Command::new(&login_cmd)
            .args(args)
            .spawn()?;

        syscall::setrens(0, 0)?;

        let mut event_queue = EventQueue::<()>::new()?;

        let socket_fd = self.socket.as_raw_fd();
        let display_fd = self.display.as_raw_fd();

        let me = Rc::new(RefCell::new(self));
        let me2 = Rc::clone(&me);

        event_queue.add(socket_fd, move |_| -> io::Result<Option<()>> {
            let mut me = me2.borrow_mut();
            let mut packets = [Packet::default(); 16];
            let mut all: Vec<Packet> = Vec::new();
            loop {
                match unsafe { read_to_slice(&mut me.socket, &mut packets) }? {
                    0 => break,
                    count => all.extend(&packets[..count])
                }
            }
            handle_socket(&mut me, &mut packets)?;
            Ok(None)
        })?;

        event_queue.add(display_fd, move |_| -> io::Result<Option<()>> {
            let mut me = me.borrow_mut();
            let mut events = [Event::new(); 16];
            let mut all: Vec<Event> = Vec::new();
            loop {
                match unsafe { read_to_slice(&mut me.display, &mut events) }? {
                    0 => break,
                    count => all.extend(&events[..count])
                }
            }
            for event in &all {
                if let EventOption::Resize(event) = event.to_option() {
                    unsafe {
                        display_fd_unmap(&mut me.image);
                        me.image = display_fd_map(event.width as i32, event.height as i32, display_fd as usize);
                    }
                }
            }
            handle_display(&mut me, &mut events)?;
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
impl Drop for Orbital {
    fn drop(&mut self) {
        unsafe {
            display_fd_unmap(&mut self.image);
        }
    }
}
