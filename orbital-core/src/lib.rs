#![feature(asm)]

#[macro_use] extern crate failure;
extern crate event;
extern crate orbclient;
extern crate orbimage;
extern crate syscall;

pub mod rect;
pub mod image;

use event::EventQueue;
use image::{ImageRef};
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
    rc::Rc,
    slice,
    str,
};
use syscall::{
    SchemeMut,
    data::Packet,
    error::{EINVAL},
    flag::{O_CLOEXEC, O_CREAT, O_NONBLOCK, O_RDWR}
};

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
pub fn fix_env(display_path: &str) -> io::Result<()> {
    env::set_current_dir("file:")?;

    env::set_var("DISPLAY", &display_path);

    let path = env::var("PATH").unwrap_or(String::new());
    let new_path = env::join_paths(
        env::split_paths(&path)
            .chain(iter::once(PathBuf::from("/ui/bin")))
    ).unwrap();
    env::set_var("PATH", new_path);
    Ok(())
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

pub trait Handler {
    type Drain: IntoIterator<Item = Event>;

    /// Called when the event loop is first ran
    fn handle_startup(&mut self, _orb: &mut Orbital) -> io::Result<()> { Ok(()) }

    /// Return true if a packet should be delayed until a display event
    fn should_delay(&mut self, packet: &Packet) -> bool;

    /// Callback to handle events over the scheme
    fn handle_scheme(&mut self, orb: &mut Orbital, packets: &mut [Packet]) -> io::Result<()>;
    /// Callback to handle events over the display socket
    fn handle_display(&mut self, orb: &mut Orbital, events: &mut [Event]) -> io::Result<()>;

    /// Called after a batch of scheme events have been handled
    fn handle_scheme_after(&mut self, _orb: &mut Orbital) -> io::Result<()> { Ok(()) }
    /// Called after a batch of display events have been handled
    fn handle_display_after(&mut self, _orb: &mut Orbital) -> io::Result<()> { Ok(()) }
    /// Called after a batch of any events have been handled
    fn handle_after(&mut self, _orb: &mut Orbital) -> io::Result<()> { Ok(()) }

    /// Called when a new window is requested by the scheme
    fn handle_window_new(&mut self, orb: &mut Orbital,
                         x: i32, y: i32, width: i32, height: i32,
                         flags: &str, title: String) -> syscall::Result<usize>;
    /// Called when the scheme is read
    fn handle_window_read(&mut self, orb: &mut Orbital, id: usize, buf: &mut [u8]) -> syscall::Result<usize>;
    fn handle_window_position(&mut self, orb: &mut Orbital, id: usize, x: Option<i32>, y: Option<i32>) -> syscall::Result<()>;
    fn handle_window_resize(&mut self, orb: &mut Orbital, id: usize, w: Option<i32>, h: Option<i32>) -> syscall::Result<()>;
    fn handle_window_title(&mut self, orb: &mut Orbital, id: usize, title: String) -> syscall::Result<()>;
    fn handle_window_lookup(&mut self, orb: &mut Orbital, id: usize) -> syscall::Result<usize>;
    fn handle_window_map(&mut self, orb: &mut Orbital, id: usize, offset: usize, size: usize) -> syscall::Result<usize>;
    fn handle_window_path(&mut self, orb: &mut Orbital, id: usize, buf: &mut [u8]) -> syscall::Result<usize>;
    fn handle_window_sync(&mut self, orb: &mut Orbital, id: usize) -> syscall::Result<usize>;
    fn handle_window_close(&mut self, orb: &mut Orbital, id: usize) -> syscall::Result<usize>;
}

pub struct Orbital {
    pub scheme: File,
    pub display: File,
    pub image: ImageRef<'static>,
    pub todo: Vec<Packet>,

    pub width: i32,
    pub height: i32
}
impl Orbital {
    /// Open an orbital display and connect to the scheme
    pub fn open_display(display_path: &str) -> io::Result<Self> {
        let scheme = syscall::open(":orbital", O_CREAT | O_CLOEXEC | O_NONBLOCK | O_RDWR)
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
            scheme: scheme,
            display: display,
            image: image,
            todo: Vec::new(),

            width: width,
            height: height
        })
    }
    /// Write an Event to display I/O
    pub fn display_write(&mut self, event: &Event) -> io::Result<()> {
        self.display.write(event).map(|_| ())
    }
    /// Synchronize display I/O
    pub fn display_sync(&mut self) -> io::Result<()> {
        self.display.sync_all()
    }
    /// Write a Packet to scheme I/O
    pub fn scheme_write(&mut self, packet: &Packet) -> io::Result<()> {
        self.scheme.write(packet).map(|_| ())
    }
    /// Synchronize the scheme I/O
    pub fn scheme_sync(&mut self) -> io::Result<()> {
        self.scheme.sync_all()
    }
    /// Return the screen rectangle
    pub fn screen_rect(&self) -> Rect {
        Rect::new(0, 0, self.image.width(), self.image.height())
    }
    /// Start the main loop
    pub fn run<H>(mut self, mut handler: H) -> Result<(), Error>
        where H: Handler + 'static
    {
        let mut event_queue = EventQueue::<()>::new()?;

        syscall::setrens(0, 0)?;

        let scheme_fd = self.scheme.as_raw_fd();
        let display_fd = self.display.as_raw_fd();

        handler.handle_startup(&mut self)?;

        let me = Rc::new(RefCell::new(OrbitalHandler {
            orb: self,
            handler: handler,
        }));
        let me2 = Rc::clone(&me);

        event_queue.add(scheme_fd, move |_| -> io::Result<Option<()>> {
            let mut me = me.borrow_mut();
            let me = &mut *me;
            let mut packets = [Packet::default(); 16];
            loop {
                match unsafe { read_to_slice(&mut me.orb.scheme, &mut packets) }? {
                    0 => break,
                    count => {
                        let packets = &mut packets[..count];
                        for packet in packets.iter_mut() {
                            let delay = me.handler.should_delay(packet);

                            me.handle(packet);

                            if delay && packet.a == 0 {
                                me.orb.todo.push(*packet);
                            } else {
                                me.orb.scheme_write(&packet)?;
                            }
                        }
                        me.handler.handle_scheme(&mut me.orb, packets)?;
                    }
                }
            }
            me.handler.handle_scheme_after(&mut me.orb)?;
            me.handler.handle_after(&mut me.orb)?;
            Ok(None)
        })?;

        event_queue.add(display_fd, move |_| -> io::Result<Option<()>> {
            let mut me = me2.borrow_mut();
            let me = &mut *me;
            let mut events = [Event::new(); 16];
            loop {
                match unsafe { read_to_slice(&mut me.orb.display, &mut events) }? {
                    0 => break,
                    count => {
                        let events = &mut events[..count];
                        for event in events.iter() {
                            if let EventOption::Resize(event) = event.to_option() {
                                unsafe {
                                    display_fd_unmap(&mut me.orb.image);
                                    me.orb.image = display_fd_map(
                                        event.width as i32,
                                        event.height as i32,
                                        display_fd as usize
                                    );
                                }
                            }
                        }

                        let mut i = 0;
                        while i < me.orb.todo.len() {
                            let mut packet = me.orb.todo[i].clone();

                            let delay = me.handler.should_delay(&packet);

                            me.handle(&mut packet);

                            if delay && packet.a == 0 {
                                i += 1;
                            }else{
                                me.orb.todo.remove(i);
                                me.orb.scheme_write(&packet)?;
                            }
                        }

                        me.handler.handle_display(&mut me.orb, events)?;
                    }
                }
            }
            me.handler.handle_display_after(&mut me.orb)?;
            me.handler.handle_after(&mut me.orb)?;
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
pub struct OrbitalHandler<H: Handler> {
    orb: Orbital,
    handler: H
}
impl<H: Handler> SchemeMut for OrbitalHandler<H> {
    fn open(&mut self, path: &[u8], _: usize, _: u32, _: u32) -> syscall::Result<usize> {
        let path = try!(str::from_utf8(path).or(Err(syscall::Error::new(EINVAL))));
        let mut parts = path.split("/");

        let flags = parts.next().unwrap_or("");

        let x = parts.next().unwrap_or("").parse::<i32>().unwrap_or(0);
        let y = parts.next().unwrap_or("").parse::<i32>().unwrap_or(0);
        let width = parts.next().unwrap_or("").parse::<i32>().unwrap_or(0);
        let height = parts.next().unwrap_or("").parse::<i32>().unwrap_or(0);

        let mut title = parts.next().unwrap_or("").to_string();
        for part in parts {
            title.push('/');
            title.push_str(part);
        }

        self.handler.handle_window_new(&mut self.orb, x, y, width, height, flags, title)
    }
    fn read(&mut self, id: usize, buf: &mut [u8]) -> syscall::Result<usize> {
        self.handler.handle_window_read(&mut self.orb, id, buf)
    }
    fn write(&mut self, id: usize, buf: &[u8]) -> syscall::Result<usize> {
        if let Ok(msg) = str::from_utf8(buf) {
            let mut parts = msg.split(',');
            match parts.next() {
                Some("P") => {
                    let x = parts.next().unwrap_or("").parse::<i32>().ok();
                    let y = parts.next().unwrap_or("").parse::<i32>().ok();

                    self.handler.handle_window_position(&mut self.orb, id, x, y)?;

                    Ok(buf.len())
                },
                Some("S") => {
                    let w = parts.next().unwrap_or("").parse::<i32>().ok();
                    let h = parts.next().unwrap_or("").parse::<i32>().ok();

                    self.handler.handle_window_resize(&mut self.orb, id, w, h)?;

                    Ok(buf.len())
                },
                Some("T") => {
                    let title = parts.next().unwrap_or("").to_string();

                    self.handler.handle_window_title(&mut self.orb, id, title)?;

                    Ok(buf.len())
                },
                _ => Err(syscall::Error::new(EINVAL))
            }
        } else {
            Err(syscall::Error::new(EINVAL))
        }
    }
    fn fevent(&mut self, id: usize, _flags: usize) -> syscall::Result<usize> {
        self.handler.handle_window_lookup(&mut self.orb, id)
    }
    fn fmap(&mut self, id: usize, offset: usize, size: usize) -> syscall::Result<usize> {
        self.handler.handle_window_map(&mut self.orb, id, offset, size)
    }
    fn fpath(&mut self, id: usize, buf: &mut [u8]) -> syscall::Result<usize> {
        self.handler.handle_window_path(&mut self.orb, id, buf)
    }
    fn fsync(&mut self, id: usize) -> syscall::Result<usize> {
        self.handler.handle_window_sync(&mut self.orb, id)
    }
    fn close(&mut self, id: usize) -> syscall::Result<usize> {
        self.handler.handle_window_close(&mut self.orb, id)
    }
}
