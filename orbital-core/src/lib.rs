#[macro_use] extern crate failure;
extern crate event;
extern crate libc;
extern crate orbclient;
extern crate orbimage;
extern crate syscall;

pub mod display;
pub mod image;
pub mod rect;

use display::Display;
use event::EventQueue;
use image::{ImageRef};
use orbclient::{Color, Event};
use rect::Rect;
use std::{
    cell::RefCell,
    env,
    fs::File,
    io::{self, ErrorKind, Read, Write},
    iter,
    mem,
    os::unix::io::{AsRawFd, FromRawFd, RawFd},
    path::PathBuf,
    rc::Rc,
    slice,
    str,
};
use syscall::{
    data::Packet,
    error::EINVAL,
    flag::{O_CLOEXEC, O_CREAT, O_NONBLOCK, O_RDWR},
    EventFlags,
    SchemeMut,
};

#[cfg(target_pointer_width = "32")]
const CLIPBOARD_FLAG: usize = 1 << 31;

#[cfg(target_pointer_width = "64")]
const CLIPBOARD_FLAG: usize = 1 << 63;

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

pub const PROPERTY_ASYNC:      u8 = 1 << 0;
pub const PROPERTY_BORDERLESS: u8 = 1 << 1;
pub const PROPERTY_RESIZABLE:  u8 = 1 << 2;
pub const PROPERTY_TRANSPARENT: u8 = 1 << 3;
pub const PROPERTY_UNCLOSABLE: u8 = 1 << 4;

pub struct Properties<'a> {
    pub properties: u8,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub title: &'a str
}

pub trait Handler {
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

    /// Called when a new window is requested by the scheme.
    /// Return a window ID that will be used to identify it later.
    fn handle_window_new(&mut self, orb: &mut Orbital,
                         x: i32, y: i32, width: i32, height: i32,
                         flags: &str, title: String) -> syscall::Result<usize>;
    /// Called when the scheme is read for events
    fn handle_window_read(&mut self, orb: &mut Orbital, id: usize, buf: &mut [Event]) -> syscall::Result<usize>;
    /// Called when the window asks to set async
    fn handle_window_async(&mut self, orb: &mut Orbital, id: usize, is_async: bool) -> syscall::Result<()>;
    /// Called when the window asks to set mouse cursor visibility
    fn handle_window_mouse_cursor(&mut self, orb: &mut Orbital, id: usize, visible: bool) -> syscall::Result<()>;
    /// Called when the window asks to set mouse grabbing
    fn handle_window_mouse_grab(&mut self, orb: &mut Orbital, id: usize, grab: bool) -> syscall::Result<()>;
    /// Called when the window asks to set mouse relative mode
    fn handle_window_mouse_relative(&mut self, orb: &mut Orbital, id: usize, relative: bool) -> syscall::Result<()>;
    /// Called when the window asks to be repositioned
    fn handle_window_position(&mut self, orb: &mut Orbital, id: usize, x: Option<i32>, y: Option<i32>) -> syscall::Result<()>;
    /// Called when the window asks to be resized
    fn handle_window_resize(&mut self, orb: &mut Orbital, id: usize, w: Option<i32>, h: Option<i32>) -> syscall::Result<()>;
    /// Called when the window asks to change title
    fn handle_window_title(&mut self, orb: &mut Orbital, id: usize, title: String) -> syscall::Result<()>;
    /// Called by fevent to clear notified status, assuming you're sending edge-triggered notifications
    /// TODO: Abstract event system away completely.
    fn handle_window_clear_notified(&mut self, orb: &mut Orbital, id: usize) -> syscall::Result<()>;
    /// Return a reference the window's image that will be mapped in the scheme's fmap function
    fn handle_window_map(&mut self, orb: &mut Orbital, id: usize) -> syscall::Result<&mut [Color]>;
    /// Called to get window properties
    fn handle_window_properties(&mut self, orb: &mut Orbital, id: usize) -> syscall::Result<Properties>;
    /// Called to flush a window. It's usually a good idea to redraw here.
    fn handle_window_sync(&mut self, orb: &mut Orbital, id: usize) -> syscall::Result<usize>;
    /// Called when a window should be closed
    fn handle_window_close(&mut self, orb: &mut Orbital, id: usize) -> syscall::Result<usize>;

    // Create a clipboard from a window
    fn handle_clipboard_new(&mut self, orb: &mut Orbital, id: usize) -> syscall::Result<usize>;
    // Read window clipboard
    fn handle_clipboard_read(&mut self, orb: &mut Orbital, id: usize, buf: &mut [u8]) -> syscall::Result<usize>;
    // Write window clipboard
    fn handle_clipboard_write(&mut self, orb: &mut Orbital, id: usize, buf: &[u8]) -> syscall::Result<usize>;
    // Close the window's clipboard access
    fn handle_clipboard_close(&mut self, orb: &mut Orbital, id: usize) -> syscall::Result<usize>;
}

pub struct Orbital {
    pub scheme: File,
    pub todo: Vec<Packet>,
    pub displays: Vec<Display>,
}
impl Orbital {
    /// Open an orbital display and connect to the scheme
    pub fn open_display(display_path: &str) -> io::Result<Self> {
        let display = syscall::open(&display_path, O_CLOEXEC | O_NONBLOCK | O_RDWR)
            .map(|socket| {
                unsafe { File::from_raw_fd(socket as RawFd) }
            })
            .map_err(|err| {
                eprintln!("orbital: failed to open display {}: {}", display_path, err);
                io::Error::from_raw_os_error(err.errno)
            })?;

        let scheme = syscall::open(":orbital", O_CREAT | O_CLOEXEC | O_NONBLOCK | O_RDWR)
            .map(|socket| {
                unsafe { File::from_raw_fd(socket as RawFd) }
            })
            .map_err(|err| {
                eprintln!("orbital: failed to create :orbital: {}", err);
                io::Error::from_raw_os_error(err.errno)
            })?;

        let mut buf: [u8; 4096] = [0; 4096];
        let count = syscall::fpath(display.as_raw_fd() as usize, &mut buf).unwrap();

        let url = unsafe { String::from_utf8_unchecked(Vec::from(&buf[..count])) };

        let mut url_parts = url.split(':');
        let scheme_name = url_parts.next().unwrap();
        let path = url_parts.next().unwrap();

        let mut path_parts = path.split('/');
        let vt_screen = path_parts.next().unwrap_or("");
        let width = path_parts.next().unwrap_or("").parse::<i32>().unwrap_or(0);
        let height = path_parts.next().unwrap_or("").parse::<i32>().unwrap_or(0);

        let mut displays = vec![Display::new(0, 0, width, height, display)?];

        // If display server supports multiple displays in a VT
        if vt_screen.contains('.') {
            // Look for other screens in the same VT
            let mut parts = vt_screen.split('.');
            let vt_i = parts.next().unwrap_or("").parse::<usize>().unwrap_or(0);
            let start_screen_i = parts.next().unwrap_or("").parse::<usize>().unwrap_or(0);
            //TODO: determine maximum number of screens
            for screen_i in start_screen_i + 1..1024 {
                let extra_path = format!("{}:{}.{}", scheme_name, vt_i, screen_i);
                let extra_file = match syscall::open(&extra_path, O_CLOEXEC | O_NONBLOCK | O_RDWR) {
                    Ok(socket) => unsafe { File::from_raw_fd(socket as RawFd) },
                    Err(_err) => break,

                };

                let mut buf: [u8; 4096] = [0; 4096];
                let count = syscall::fpath(extra_file.as_raw_fd() as usize, &mut buf).unwrap();

                let url = unsafe { String::from_utf8_unchecked(Vec::from(&buf[..count])) };

                let mut url_parts = url.split(':');
                let _scheme_name = url_parts.next().unwrap();
                let path = url_parts.next().unwrap();

                let mut path_parts = path.split('/');
                let _vt_screen = path_parts.next().unwrap_or("");
                let width = path_parts.next().unwrap_or("").parse::<i32>().unwrap_or(0);
                let height = path_parts.next().unwrap_or("").parse::<i32>().unwrap_or(0);

                let x = if let Some(last) = displays.last() {
                    last.x + last.image.width()
                } else {
                    0
                };
                let y = 0;

                println!(
                    "orbital: Extra display {} at {}, {}, {}, {}",
                    screen_i,
                    x,
                    y,
                    width,
                    height
                );

                displays.push(Display::new(x, y, width, height, extra_file)?);
            }
        }

        Ok(Orbital {
            scheme,
            todo: Vec::new(),
            displays,
        })
    }

    //TODO: replace these adapter functions
    pub fn image(&self) -> &ImageRef<'static> {
        &self.displays[0].image
    }
    pub fn image_mut(&mut self) -> &mut ImageRef<'static> {
        &mut self.displays[0].image
    }
    /// Return the screen rectangle
    pub fn screen_rect(&self) -> Rect {
        self.displays[0].screen_rect()
    }

    /// Write an Event to display I/O
    pub fn display_write(&mut self, event: &Event) -> io::Result<()> {
        self.displays[0].file.write(event).map(|_| ())
    }
    /// Synchronize display I/O
    pub fn display_sync(&mut self) -> io::Result<()> {
        for display in self.displays.iter_mut() {
            display.file.sync_all()?;
        }
        Ok(())
    }
    /// Write a Packet to scheme I/O
    pub fn scheme_write(&mut self, packet: &Packet) -> io::Result<()> {
        self.scheme.write(packet).map(|_| ())
    }
    /// Synchronize the scheme I/O
    pub fn scheme_sync(&mut self) -> io::Result<()> {
        self.scheme.sync_all()
    }
    /// Resize the inner image buffer. You're responsible for redrawing.
    pub fn resize(&mut self, width: i32, height: i32) {
        //TODO: should other screens be moved after a resize?
        //TODO: support resizing other screens?
        unsafe { self.displays[0].resize(width, height); }
    }
    /// Start the main loop
    pub fn run<H>(mut self, mut handler: H) -> Result<(), Error>
        where H: Handler + 'static
    {
        let mut event_queue = EventQueue::<()>::new()?;

        //TODO: Figure out why rand: gets opened after this: syscall::setrens(0, 0)?;

        let scheme_fd = self.scheme.as_raw_fd();
        let display_fd = self.displays[0].file.as_raw_fd();

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
            let result = loop {
                match unsafe { read_to_slice(&mut me.orb.scheme, &mut packets) } {
                    Ok(0) => break Some(()),
                    Ok(count) => {
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
                    },
                    Err(err) => if err.kind() == ErrorKind::WouldBlock {
                        break None;
                    } else {
                        return Err(err);
                    }
                }
            };
            me.handler.handle_scheme_after(&mut me.orb)?;
            me.handler.handle_after(&mut me.orb)?;
            Ok(result)
        })?;

        event_queue.add(display_fd, move |_| -> io::Result<Option<()>> {
            let mut me = me2.borrow_mut();
            let me = &mut *me;
            let mut events = [Event::new(); 16];
            loop {
                match unsafe { read_to_slice(&mut me.orb.displays[0].file, &mut events) }? {
                    0 => break,
                    count => {
                        let events = &mut events[..count];

                        let mut i = 0;
                        while i < me.orb.todo.len() {
                            let mut packet = me.orb.todo[i];

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
            flags: EventFlags::empty(),
        })?;
        event_queue.run()?;
        //TODO: Cleanup and handle TODO
        Ok(())
    }
}
pub struct OrbitalHandler<H: Handler> {
    orb: Orbital,
    handler: H
}
impl<H: Handler> SchemeMut for OrbitalHandler<H> {
    fn open(&mut self, path: &str, _: usize, _: u32, _: u32) -> syscall::Result<usize> {
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
    fn dup(&mut self, id: usize, buf: &[u8]) -> syscall::Result<usize> {
        if buf == b"clipboard" {
            //TODO: implement better clipboard mechanism
            self.handler.handle_clipboard_new(&mut self.orb, id).map(|id| id | CLIPBOARD_FLAG)
        } else {
            Err(syscall::Error::new(EINVAL))
        }
    }
    fn read(&mut self, id: usize, buf: &mut [u8]) -> syscall::Result<usize> {
        //TODO: implement better clipboard mechanism
        if id & CLIPBOARD_FLAG == CLIPBOARD_FLAG {
            return self.handler.handle_clipboard_read(&mut self.orb, id & !CLIPBOARD_FLAG, buf);
        }

        let slice: &mut [Event] = unsafe {
            slice::from_raw_parts_mut(
                buf.as_mut_ptr() as *mut Event,
                buf.len() / mem::size_of::<Event>()
            )
        };
        let n = self.handler.handle_window_read(&mut self.orb, id, slice)?;
        Ok(n * mem::size_of::<Event>())
    }
    fn write(&mut self, id: usize, buf: &[u8]) -> syscall::Result<usize> {
        //TODO: implement better clipboard mechanism
        if id & CLIPBOARD_FLAG == CLIPBOARD_FLAG {
            return self.handler.handle_clipboard_write(&mut self.orb, id & !CLIPBOARD_FLAG, buf);
        }

        if let Ok(msg) = str::from_utf8(buf) {
            let (kind, data) = {
                let mut parts = msg.splitn(2, ',');
                let kind = parts.next().unwrap_or("");
                let data = parts.next().unwrap_or("");
                (kind, data)
            };
            match kind {
                "A" => match data {
                    "0" => {
                        self.handler.handle_window_async(&mut self.orb, id, false)?;
                        Ok(buf.len())
                    },
                    "1" => {
                        self.handler.handle_window_async(&mut self.orb, id, true)?;
                        Ok(buf.len())
                    },
                    _ => Err(syscall::Error::new(EINVAL)),
                },
                "M" => match data {
                    "C,0" => {
                        self.handler.handle_window_mouse_cursor(&mut self.orb, id, false)?;
                        Ok(buf.len())
                    },
                    "C,1" => {
                        self.handler.handle_window_mouse_cursor(&mut self.orb, id, true)?;
                        Ok(buf.len())
                    },
                    "G,0" => {
                        self.handler.handle_window_mouse_grab(&mut self.orb, id, false)?;
                        Ok(buf.len())
                    }
                    "G,1" => {
                        self.handler.handle_window_mouse_grab(&mut self.orb, id, true)?;
                        Ok(buf.len())
                    },
                    "R,0" => {
                        self.handler.handle_window_mouse_relative(&mut self.orb, id, false)?;
                        Ok(buf.len())
                    },
                    "R,1" => {
                        self.handler.handle_window_mouse_relative(&mut self.orb, id, true)?;
                        Ok(buf.len())
                    },
                    _ => Err(syscall::Error::new(EINVAL)),
                },
                "P" => {
                    let mut parts = data.split(',');
                    let x = parts.next().unwrap_or("").parse::<i32>().ok();
                    let y = parts.next().unwrap_or("").parse::<i32>().ok();

                    self.handler.handle_window_position(&mut self.orb, id, x, y)?;

                    Ok(buf.len())
                },
                "S" => {
                    let mut parts = data.split(',');
                    let w = parts.next().unwrap_or("").parse::<i32>().ok();
                    let h = parts.next().unwrap_or("").parse::<i32>().ok();

                    self.handler.handle_window_resize(&mut self.orb, id, w, h)?;

                    Ok(buf.len())
                },
                "T" => {
                    self.handler.handle_window_title(&mut self.orb, id, data.to_string())?;

                    Ok(buf.len())
                },
                _ => Err(syscall::Error::new(EINVAL))
            }
        } else {
            Err(syscall::Error::new(EINVAL))
        }
    }
    fn fevent(&mut self, id: usize, _flags: syscall::EventFlags) -> syscall::Result<syscall::EventFlags> {
        self.handler
            .handle_window_clear_notified(&mut self.orb, id)
            .and(Ok(syscall::EventFlags::empty()))
    }
    fn fmap(&mut self, id: usize, map: &syscall::Map) -> syscall::Result<usize> {
        let page_size = 4096;
        let map_pages = (map.offset + map.size + page_size - 1)/page_size;
        let data = self.handler.handle_window_map(&mut self.orb, id)?;
        let data_addr = data.as_mut_ptr() as usize;
        let data_size = data.len() * mem::size_of::<Color>();
        // Do not allow leaking data before or after window to the user
        if data_addr & (page_size - 1) == 0 && map_pages * page_size <= data_size {
            Ok(data_addr + map.offset)
        } else {
            Err(syscall::Error::new(EINVAL))
        }
    }
    fn fmap_old(&mut self, id: usize, map: &syscall::OldMap) -> syscall::Result<usize> {
        self.fmap(id, &syscall::Map {
            offset: map.offset,
            size: map.size,
            flags: map.flags,
            address: 0,
        })
    }
    fn funmap(&mut self, _address: usize, _size: usize) -> syscall::Result<usize> {
        // TODO
        Ok(0)
    }
    fn funmap_old(&mut self, _address: usize) -> syscall::Result<usize> {
        // TODO
        Ok(0)
    }
    fn fpath(&mut self, id: usize, mut buf: &mut [u8]) -> syscall::Result<usize> {
        let props = self.handler.handle_window_properties(&mut self.orb, id)?;
        let original_len = buf.len();
        write!(buf,
            "orbital:{}{}{}{}{}{}/{}/{}/{}/{}/{}",
            if props.properties & PROPERTY_ASYNC == PROPERTY_ASYNC { "a" } else { "" },
            "", // TODO: Z order
            if props.properties & PROPERTY_BORDERLESS == PROPERTY_BORDERLESS { "l" } else { "" },
            if props.properties & PROPERTY_RESIZABLE == PROPERTY_RESIZABLE { "r" } else { "" },
            if props.properties & PROPERTY_TRANSPARENT == PROPERTY_TRANSPARENT { "t" } else { "" },
            if props.properties & PROPERTY_UNCLOSABLE == PROPERTY_UNCLOSABLE { "u" } else { "" },
            props.x, props.y, props.width, props.height, props.title
        ).unwrap();
        Ok(original_len - buf.len())
    }
    fn fsync(&mut self, id: usize) -> syscall::Result<usize> {
        self.handler.handle_window_sync(&mut self.orb, id)
    }
    fn close(&mut self, id: usize) -> syscall::Result<usize> {
        //TODO: implement better clipboard mechanism
        if id & CLIPBOARD_FLAG == CLIPBOARD_FLAG {
            return self.handler.handle_clipboard_close(&mut self.orb, id & !CLIPBOARD_FLAG);
        }

        self.handler.handle_window_close(&mut self.orb, id)
    }
}
