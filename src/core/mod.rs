use std::{
    cell::RefCell,
    collections::BTreeMap,
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

use event::{user_data, EventQueue};
use failure::Fail;
use libredox::flag;
use log::{debug, error, info};
use orbclient::{Color, Event};
use syscall::{
    data::Packet,
    error::EINVAL,
    flag::{O_CLOEXEC, O_CREAT, O_NONBLOCK, O_RDWR},
    flag::EventFlags,
    SchemeMut, PAGE_SIZE, KSMSG_MMAP_PREP, KSMSG_MMAP, KSMSG_MSYNC, KSMSG_MUNMAP, MapFlags, ESKMSG, SKMSG_PROVIDE_MMAP,
};

use display::Display;
use image::ImageRef;
use rect::Rect;

pub(crate) mod display;
pub(crate) mod image;
pub(crate) mod rect;

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
    #[fail(display = "system error: {}", _0)]
    LibredoxError(libredox::error::Error),
}
impl From<io::Error> for Error {
    fn from(err: io::Error) -> Self { Error::IoError(err) }
}
impl From<syscall::Error> for Error {
    fn from(err: syscall::Error) -> Self { Error::SyscallError(err) }
}
impl From<libredox::error::Error> for Error {
    fn from(err: libredox::error::Error) -> Self { Error::LibredoxError(err) }
}

/// Convenience function for setting DISPLAY and PATH environment variables
pub fn fix_env(display_path: &str) -> io::Result<()> {
    env::set_var("DISPLAY", display_path);
    Ok(())
}

fn read_to_slice<R: Read, T: Copy>(mut r: R, buf: &mut [T]) -> io::Result<usize> {
    unsafe {
        r.read(slice::from_raw_parts_mut(buf.as_mut_ptr() as *mut u8, buf.len() * mem::size_of::<T>()))
            .map(|count| count / mem::size_of::<T>())
    }
}

pub struct Properties<'a> {
    //TODO: avoid allocation
    pub flags: String,
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
    #[allow(clippy::too_many_arguments)]
    fn handle_window_new(&mut self, orb: &mut Orbital,
                         x: i32, y: i32, width: i32, height: i32,
                         flags: &str, title: String) -> syscall::Result<usize>;
    /// Called when the scheme is read for events
    fn handle_window_read(&mut self, orb: &mut Orbital, id: usize, buf: &mut [Event]) -> syscall::Result<usize>;
    /// Called when the window asks to set async
    fn handle_window_async(&mut self, orb: &mut Orbital, id: usize, is_async: bool) -> syscall::Result<()>;
    /// Called when the window asks to be dragged
    fn handle_window_drag(&mut self, _orb: &mut Orbital, id: usize /*TODO: resize sides */) -> syscall::Result<()>;
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
    /// Called when the window wants to set a flag
    fn handle_window_set_flag(&mut self, orb: &mut Orbital, id: usize, flag: char, value: bool) -> syscall::Result<()>;
    /// Called when the window asks to change title
    fn handle_window_title(&mut self, orb: &mut Orbital, id: usize, title: String) -> syscall::Result<()>;
    /// Called by fevent to clear notified status, assuming you're sending edge-triggered notifications
    /// TODO: Abstract event system away completely.
    fn handle_window_clear_notified(&mut self, orb: &mut Orbital, id: usize) -> syscall::Result<()>;
    /// Return a reference the window's image that will be mapped in the scheme's fmap function
    fn handle_window_map(&mut self, orb: &mut Orbital, id: usize, create_new: bool) -> syscall::Result<&mut [Color]>;
    /// Free a reference to the window's image, for use by funmap
    fn handle_window_unmap(&mut self, orb: &mut Orbital, id: usize) -> syscall::Result<()>;
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
    pub maps: BTreeMap<usize, (usize, usize)>,

    /// Handle to "input:consumer" to recieve input events.
    pub input: File,
}

impl Orbital {
    fn url_parts(url: &str) -> io::Result<(&str, &str)> {
        let mut url_parts = url.split(':');
        let scheme_name = url_parts.next()
            .ok_or(io::Error::new(ErrorKind::Other,
                                  "Could not get scheme name from url"))?;
        let path = url_parts.next()
            .ok_or(io::Error::new(ErrorKind::Other,
                                  "Could not get path from url"))?;
        Ok((scheme_name, path))
    }

    fn parse_display_path(path: &str) -> (&str, i32, i32) {
        let mut path_parts = path.split('/');
        let vt_screen = path_parts.next().unwrap_or("");
        let width = path_parts.next().unwrap_or("").parse::<i32>().unwrap_or(0);
        let height = path_parts.next().unwrap_or("").parse::<i32>().unwrap_or(0);

        (vt_screen, width, height)
    }

    /// Open an orbital display and connect to the scheme
    pub fn open_display(vt: &str) -> io::Result<Self> {
        let mut buffer = [0; 1024];

        let input_handle = File::open(format!("input:consumer/{vt}"))?;
        let fd = input_handle.as_raw_fd();

        let written = libredox::call::fpath(fd as usize, &mut buffer)
            .expect("init: failed to get the path to the display device");

        assert!(written <= buffer.len());

        let display_path = std::str::from_utf8(&buffer[..written])
            .expect("init: display path UTF-8 check failed");

        fix_env(&display_path)?;

        let display = libredox::call::open(display_path, flag::O_CLOEXEC | flag::O_NONBLOCK | flag::O_RDWR, 0)
            .map(|socket| {
                unsafe { File::from_raw_fd(socket as RawFd) }
            })
            .map_err(|err| {
                error!("failed to open display {}: {}", display_path, err);
                io::Error::from_raw_os_error(err.errno())
            })?;

        let scheme = libredox::call::open(":orbital", flag::O_CREAT | flag::O_CLOEXEC | flag::O_NONBLOCK | flag::O_RDWR, 0)
            .map(|socket| {
                unsafe { File::from_raw_fd(socket as RawFd) }
            })
            .map_err(|err| {
                error!("failed to open ':orbital': {}", err);
                io::Error::from_raw_os_error(err.errno())
            })?;

        let mut buf: [u8; 4096] = [0; 4096];
        let count = libredox::call::fpath(display.as_raw_fd() as usize, &mut buf)
            .map_err(|e| io::Error::new(ErrorKind::Other,
                                        format!("Could not read display path with fpath(): {e}")))?;

        let url = String::from_utf8(Vec::from(&buf[..count]))
            .map_err(|_| io::Error::new(ErrorKind::Other,
                                        "Could not create Utf8 Url String"))?;
        let (scheme_name, path) = Self::url_parts(&url)?;
        let (vt_screen, width, height) = Self::parse_display_path(path);
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
                let extra_file = match libredox::call::open(&extra_path, flag::O_CLOEXEC | flag::O_NONBLOCK | flag::O_RDWR, 0) {
                    Ok(socket) => unsafe { File::from_raw_fd(socket as RawFd) },
                    Err(_err) => break,
                };

                let mut buf: [u8; 4096] = [0; 4096];
                let count = libredox::call::fpath(extra_file.as_raw_fd() as usize, &mut buf)
                    .map_err(|_| io::Error::new(ErrorKind::Other,
                                                "Could not open extra_file as_raw_fd()"))?;

                let url = String::from_utf8(Vec::from(&buf[..count]))
                    .map_err(|_| io::Error::new(ErrorKind::Other,
                                                "Could not create Utf8 Url String"))?;

                let (_scheme_name, path) = Self::url_parts(&url)?;
                let (_vt_screen, width, height) = Self::parse_display_path(path);

                let x = if let Some(last) = displays.last() {
                    last.x + last.image.width()
                } else {
                    0
                };
                let y = 0;

                debug!("Extra display {} at {}, {}, {}, {}", screen_i, x, y, width, height);

                displays.push(Display::new(x, y, width, height, extra_file)?);
            }
        }

        Ok(Orbital {
            scheme,
            todo: Vec::new(),
            displays,
            maps: BTreeMap::new(),
            input: input_handle,
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

    /// Write a Packet to scheme I/O
    pub fn scheme_write(&mut self, packet: &Packet) -> io::Result<()> {
        self.scheme.write(packet).map(|_| ())
    }
    /// Resize the inner image buffer. You're responsible for redrawing.
    pub fn resize(&mut self, width: i32, height: i32) {
        //TODO: should other screens be moved after a resize?
        //TODO: support resizing other screens?
        self.displays[0].resize(width, height);
    }
    /// Start the main loop
    pub fn run<H>(mut self, mut handler: H) -> Result<(), Error>
        where H: Handler + 'static
    {
        user_data! {
            enum Source {
                Scheme,
                Input,
            }
        }

        let mut event_queue = EventQueue::<Source>::new()?;

        //TODO: Figure out why rand: gets opened after this: libredox::call::setrens(0, 0)?;

        let scheme_fd = self.scheme.as_raw_fd();
        let input_fd = self.input.as_raw_fd();

        handler.handle_startup(&mut self)?;

        let mut me = OrbitalHandler {
            orb: self,
            handler,
        };
        event_queue.subscribe(scheme_fd as usize, Source::Scheme, event::EventFlags::READ)?;
        event_queue.subscribe(input_fd as usize, Source::Input, event::EventFlags::READ)?;

        'events: for event_res in event_queue.map(|e| e.map(|e| e.user_data)) {
            match event_res? {
                Source::Scheme => {
                    let mut packets = [Packet::default(); 16];
                    loop {
                        match read_to_slice(&mut me.orb.scheme, &mut packets) {
                            Ok(0) => break 'events,
                            Ok(count) => {
                                let packets = &mut packets[..count];
                                for packet in packets.iter_mut() {
                                    let delay = me.handler.should_delay(packet);

                                    me.handle(packet);

                                    if delay && packet.a == 0 {
                                        me.orb.todo.push(*packet);
                                    } else {
                                        me.orb.scheme_write(packet)?;
                                    }
                                }
                                me.handler.handle_scheme(&mut me.orb, packets)?;

                                me.handler.handle_scheme_after(&mut me.orb)?;
                                me.handler.handle_after(&mut me.orb)?;
                            },
                            Err(err) => if err.kind() == ErrorKind::WouldBlock {
                                continue 'events;
                            } else {
                                return Err(err.into());
                            }
                        }
                    }
                }
                Source::Input => {
                    let mut events = [Event::new(); 16];
                    loop {
                        match read_to_slice(&mut me.orb.input, &mut events)? {
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
                                    } else {
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
                }
            }
        }

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
        let mut parts = path.split('/');

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
                "D" => match data {
                    "" => {
                        self.handler.handle_window_drag(&mut self.orb, id)?;
                        Ok(buf.len())
                    }
                    //TODO: resize by dragging edge
                    // Comma separated
                    // B is bottom
                    // L is left
                    // R is right
                    // T is top
                    _ => Err(syscall::Error::new(EINVAL)),
                }
                "F" => {
                    let mut parts = data.split(',');
                    let flags = parts.next().unwrap_or("");
                    let value = match parts.next().unwrap_or("") {
                        "0" => false,
                        "1" => true,
                        _ => return Err(syscall::Error::new(EINVAL)),
                    };
                    for flag in flags.chars() {
                        self.handler.handle_window_set_flag(&mut self.orb, id, flag, value)?;
                    }
                    Ok(buf.len())
                }
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
    fn fevent(&mut self, id: usize, _flags: EventFlags) -> syscall::Result<EventFlags> {
        self.handler
            .handle_window_clear_notified(&mut self.orb, id)
            .and(Ok(EventFlags::empty()))
    }
    /*
    fn fmap(&mut self, id: usize, map: &syscall::Map) -> syscall::Result<usize> {
        let page_size = 4096;
        let map_pages = (map.offset + map.size + page_size - 1)/page_size;
        let data = self.handler.handle_window_map(&mut self.orb, id)?;
        let data_addr = data.as_mut_ptr() as usize;
        let data_size = data.len() * mem::size_of::<Color>();
        // Do not allow leaking data before or after window to the user
        if data_addr & (page_size - 1) == 0 && map_pages * page_size <= data_size {
            let address = data_addr + map.offset;
            self.orb.maps.insert(address, (id, map.size));
            Ok(address)
        } else {
            self.handler.handle_window_unmap(&mut self.orb, id)?;
            Err(syscall::Error::new(EINVAL))
        }
    }
    fn funmap(&mut self, address: usize, size: usize) -> syscall::Result<usize> {
        match self.orb.maps.remove(&address) {
            Some((id, map_size)) => {
                if size != map_size {
                    log::warn!("orbital: mapping 0x{:x} has size {} instead of {}", address, map_size, size);
                }
                self.handler.handle_window_unmap(&mut self.orb, id)?;
            },
            None => {
                error!("failed to found mapping 0x{:x}", address);
            }
        }
        Ok(0)
    }
    */
    fn fpath(&mut self, id: usize, mut buf: &mut [u8]) -> syscall::Result<usize> {
        let props = self.handler.handle_window_properties(&mut self.orb, id)?;
        let original_len = buf.len();
        #[allow(clippy::write_literal)] // TODO: Z order
        let _ = write!(buf,
            "orbital:{}/{}/{}/{}/{}/{}",
            props.flags, props.x, props.y, props.width, props.height, props.title
        );
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
    fn mmap_prep(&mut self, id: usize, offset: u64, size: usize, flags: syscall::MapFlags) -> syscall::Result<usize> {
        //TODO: handle offset, size, flags?
        let data = self.handler.handle_window_map(&mut self.orb, id, true)?;

        if size > data.len() * core::mem::size_of::<Color>() {
            return Err(syscall::Error::new(EINVAL));
        }

        Ok(data.as_mut_ptr() as usize)
    }
    fn munmap(&mut self, id: usize, offset: u64, size: usize, flags: syscall::MunmapFlags) -> syscall::Result<usize> {
        //TODO: handle offset, size, flags?
        self.handler.handle_window_unmap(&mut self.orb, id)?;

        Ok(0)
    }
}

#[cfg(test)]
mod test {
    use crate::core::Orbital;

    #[test]
    fn invalid_url_no_colon() {
        assert!(Orbital::url_parts("foo-no-colon").is_err());
    }

    #[test]
    fn valid_url_empty_scheme() {
        // until we throw an error for an empty scheme_name...
        match Orbital::url_parts(":path") {
            Ok((scheme_name, path)) => {
                assert!(scheme_name.is_empty());
                assert_eq!(path, "path");
            },
            _ => panic!("Could not parse url")
        }
    }

    #[test]
    fn valid_url_empty_path() {
        // until we throw an error for an empty scheme_name...
        match Orbital::url_parts("scheme:") {
            Ok((scheme_name, path)) => {
                assert_eq!(scheme_name, "scheme");
                assert!(path.is_empty());
            },
            _ => panic!("Could not parse url")
        }
    }

    #[test]
    fn valid_url() {
        match Orbital::url_parts("scheme:path") {
            Ok((scheme_name, path)) => {
                assert_eq!(scheme_name, "scheme");
                assert_eq!(path, "path");
            },
            _ => panic!("Could not parse url")
        }
    }
}
