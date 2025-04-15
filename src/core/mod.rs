use std::{
    collections::VecDeque,
    env,
    fs::File,
    io::{self, ErrorKind, Read, Write},
    mem,
    os::unix::io::{AsRawFd, FromRawFd, RawFd},
    slice, str,
};

use event::{user_data, EventQueue};
use libredox::flag;
use log::{debug, error};
use orbclient::{Color, Event};
use redox_scheme::{
    scheme::{IntoTag, Op, OpRead, SchemeSync},
    CallerCtx, OpenResult, RequestKind, Response, SignalBehavior, Socket,
};
use syscall::{
    error::EINVAL, flag::EventFlags, schemev2::NewFdFlags, EAGAIN, ECANCELED, EOPNOTSUPP,
    EWOULDBLOCK,
};

use crate::scheme::OrbitalScheme;
use display::Display;

pub(crate) mod display;
pub(crate) mod image;
pub(crate) mod rect;

#[cfg(target_pointer_width = "32")]
const CLIPBOARD_FLAG: usize = 1 << 31;

#[cfg(target_pointer_width = "64")]
const CLIPBOARD_FLAG: usize = 1 << 63;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io error")]
    IoError(#[from] io::Error),
    #[error("syscall error: {0}")]
    SyscallError(syscall::Error),
    #[error("system error")]
    LibredoxError(#[from] libredox::error::Error),
}
impl From<syscall::Error> for Error {
    fn from(err: syscall::Error) -> Self {
        Error::SyscallError(err)
    }
}

/// Convenience function for setting DISPLAY environment variable
pub fn fix_env(display_path: &str) -> io::Result<()> {
    env::set_var("DISPLAY", display_path);
    Ok(())
}

fn read_to_slice<R: Read, T: Copy>(mut r: R, buf: &mut [T]) -> io::Result<usize> {
    unsafe {
        r.read(slice::from_raw_parts_mut(
            buf.as_mut_ptr() as *mut u8,
            buf.len() * mem::size_of::<T>(),
        ))
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
    pub title: &'a str,
}

pub struct Orbital {
    pub scheme: Socket,
    pub delayed: VecDeque<(CallerCtx, OpRead)>,

    /// Handle to "/scheme/input/consumer" to recieve input events.
    pub input: File,
}

impl Orbital {
    fn url_parts(url: &str) -> io::Result<(&str, &str)> {
        let mut url_parts = url.split(':');
        let scheme_name = url_parts.next().ok_or(io::Error::new(
            ErrorKind::Other,
            "Could not get scheme name from url",
        ))?;
        let path = url_parts.next().ok_or(io::Error::new(
            ErrorKind::Other,
            "Could not get path from url",
        ))?;
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
    pub fn open_display(vt: &str) -> io::Result<(Self, Vec<Display>)> {
        let mut buffer = [0; 1024];

        let input_handle = File::open(format!("/scheme/input/consumer/{vt}"))?;
        let fd = input_handle.as_raw_fd();

        let written = libredox::call::fpath(fd as usize, &mut buffer)
            .expect("init: failed to get the path to the display device");

        assert!(written <= buffer.len());

        let display_path =
            std::str::from_utf8(&buffer[..written]).expect("init: display path UTF-8 check failed");

        fix_env(&display_path)?;

        let display = libredox::call::open(
            display_path,
            flag::O_CLOEXEC | flag::O_NONBLOCK | flag::O_RDWR,
            0,
        )
        .map(|socket| unsafe { File::from_raw_fd(socket as RawFd) })
        .map_err(|err| {
            error!("failed to open display {}: {}", display_path, err);
            io::Error::from_raw_os_error(err.errno())
        })?;

        let scheme = Socket::nonblock("orbital").map_err(|err| {
            error!("failed to open '/scheme/orbital': {}", err);
            err
        })?;

        let mut buf: [u8; 4096] = [0; 4096];
        let count = libredox::call::fpath(display.as_raw_fd() as usize, &mut buf).map_err(|e| {
            io::Error::new(
                ErrorKind::Other,
                format!("Could not read display path with fpath(): {e}"),
            )
        })?;

        let url = String::from_utf8(Vec::from(&buf[..count]))
            .map_err(|_| io::Error::new(ErrorKind::Other, "Could not create Utf8 Url String"))?;
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
                let extra_path = format!("/scheme/{}/{}.{}", scheme_name, vt_i, screen_i);
                let extra_file = match libredox::call::open(
                    &extra_path,
                    flag::O_CLOEXEC | flag::O_NONBLOCK | flag::O_RDWR,
                    0,
                ) {
                    Ok(socket) => unsafe { File::from_raw_fd(socket as RawFd) },
                    Err(_err) => break,
                };

                let mut buf: [u8; 4096] = [0; 4096];
                let count = libredox::call::fpath(extra_file.as_raw_fd() as usize, &mut buf)
                    .map_err(|_| {
                        io::Error::new(ErrorKind::Other, "Could not open extra_file as_raw_fd()")
                    })?;

                let url = String::from_utf8(Vec::from(&buf[..count])).map_err(|_| {
                    io::Error::new(ErrorKind::Other, "Could not create Utf8 Url String")
                })?;

                let (_scheme_name, path) = Self::url_parts(&url)?;
                let (_vt_screen, width, height) = Self::parse_display_path(path);

                let x = if let Some(last) = displays.last() {
                    last.x + last.image.width()
                } else {
                    0
                };
                let y = 0;

                debug!(
                    "Extra display {} at {}, {}, {}, {}",
                    screen_i, x, y, width, height
                );

                displays.push(Display::new(x, y, width, height, extra_file)?);
            }
        }

        Ok((
            Orbital {
                scheme,
                delayed: VecDeque::new(),
                input: input_handle,
            },
            displays,
        ))
    }

    /// Write a Packet to scheme I/O
    pub fn scheme_write(&mut self, response: Response) -> io::Result<()> {
        self.scheme
            .write_response(response, SignalBehavior::Restart)?;
        Ok(())
    }

    /// Start the main loop
    pub fn run(self, handler: OrbitalScheme) -> Result<(), Error> {
        user_data! {
            enum Source {
                Scheme,
                Input,
            }
        }

        let event_queue = EventQueue::<Source>::new()?;

        //TODO: Figure out why rand: gets opened after this: libredox::call::setrens(0, 0)?;

        let scheme_fd = self.scheme.inner().raw();
        let input_fd = self.input.as_raw_fd();

        let mut me = OrbitalHandler { orb: self, handler };
        event_queue.subscribe(scheme_fd, Source::Scheme, event::EventFlags::READ)?;
        event_queue.subscribe(input_fd as usize, Source::Input, event::EventFlags::READ)?;

        let mut event_iter = event_queue.map(|e| e.map(|e| e.user_data));
        let mut fake_input_event = None; // TODO: a hack
        let mut request_buf = Vec::with_capacity(16);

        'events: while let Some(event_res) = fake_input_event.take().or_else(|| event_iter.next()) {
            match event_res? {
                Source::Scheme => {
                    loop {
                        match me
                            .orb
                            .scheme
                            .read_requests(&mut request_buf, SignalBehavior::Restart)
                        {
                            Ok(()) => (),
                            Err(err) => {
                                if err.errno == EWOULDBLOCK || err.errno == EAGAIN {
                                    continue 'events;
                                } else {
                                    return Err(err.into());
                                }
                            }
                        }
                        if request_buf.is_empty() {
                            break 'events;
                        }
                        for request in request_buf.drain(..) {
                            let req = match request.kind() {
                                RequestKind::Call(req) => req,
                                RequestKind::OnClose { id } => {
                                    me.on_close(id);
                                    continue;
                                }
                                // TODO: faster than search?
                                RequestKind::Cancellation(req) => {
                                    if let Some(idx) = me
                                        .orb
                                        .delayed
                                        .iter()
                                        .position(|(_, op)| op.req_id() == req.id)
                                    {
                                        let (_, op) = me
                                            .orb
                                            .delayed
                                            .remove(idx)
                                            .expect("already found at index");
                                        me.orb.scheme_write(Response::err(ECANCELED, op))?;
                                    }
                                    fake_input_event = Some(Ok(Source::Input));
                                    continue;
                                }
                                _ => continue, // TODO?
                            };
                            let caller_ctx = req.caller();
                            let op = match req.op() {
                                Ok(op) => op,
                                Err(req) => {
                                    me.orb.scheme_write(Response::err(EOPNOTSUPP, req))?;
                                    continue;
                                }
                            };
                            if let Op::Read(mut read_op) = op {
                                let should_delay = me.handler.should_delay(read_op.fd);
                                let res = me.read(
                                    read_op.fd,
                                    read_op.buf(),
                                    // dont-care
                                    0,
                                    // dont-care
                                    0,
                                    &caller_ctx,
                                );
                                if should_delay && res == Ok(0) {
                                    me.orb.delayed.push_back((caller_ctx, read_op));
                                } else {
                                    me.orb.scheme_write(Response::new(res, read_op))?;
                                }
                            } else {
                                let resp = op.handle_sync(caller_ctx, &mut me);
                                me.orb.scheme_write(resp)?;
                            }
                        }
                        me.handler.handle_scheme_after(&mut me.orb)?;
                        me.handler.handle_after()?;
                    }
                }
                Source::Input => {
                    let mut events = [Event::new(); 16];
                    loop {
                        match read_to_slice(&mut me.orb.input, &mut events)? {
                            0 => break,
                            count => {
                                let events = &mut events[..count];

                                let mut delayed_left = me.orb.delayed.len();

                                while delayed_left > 0
                                    && let Some((ctx, mut read_op)) = me.orb.delayed.pop_front()
                                {
                                    delayed_left -= 1;

                                    let should_delay = me.handler.should_delay(read_op.fd);

                                    // TODO: deduplicate with the same code above
                                    let res = me.read(
                                        read_op.fd,
                                        read_op.buf(),
                                        // dont-care
                                        0,
                                        // dont-care
                                        0,
                                        &ctx,
                                    );
                                    if should_delay && res == Ok(0) {
                                        me.orb.delayed.push_back((ctx, read_op));
                                    } else {
                                        me.orb.scheme_write(Response::new(res, read_op))?;
                                    }
                                }

                                me.handler.handle_input(&mut me.orb, events)?;
                            }
                        }
                    }
                    me.handler.handle_scheme_after(&mut me.orb)?;
                    me.handler.handle_after()?;
                }
            }
        }

        //TODO: Cleanup and handle TODO
        Ok(())
    }
}
pub struct OrbitalHandler {
    orb: Orbital,
    handler: OrbitalScheme,
}
impl SchemeSync for OrbitalHandler {
    fn open(&mut self, path: &str, _flags: usize, _ctx: &CallerCtx) -> syscall::Result<OpenResult> {
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

        let id = self
            .handler
            .handle_window_new(x, y, width, height, flags, title)?;
        Ok(OpenResult::ThisScheme {
            number: id,
            flags: NewFdFlags::empty(),
        })
    }
    fn dup(&mut self, id: usize, buf: &[u8], _ctx: &CallerCtx) -> syscall::Result<OpenResult> {
        if buf == b"clipboard" {
            //TODO: implement better clipboard mechanism
            let id = self
                .handler
                .handle_clipboard_new(id)
                .map(|id| id | CLIPBOARD_FLAG)?;
            Ok(OpenResult::ThisScheme {
                number: id,
                flags: NewFdFlags::empty(),
            })
        } else {
            Err(syscall::Error::new(EINVAL))
        }
    }
    fn read(
        &mut self,
        id: usize,
        buf: &mut [u8],
        _offset: u64,
        _flags: u32,
        _ctx: &CallerCtx,
    ) -> syscall::Result<usize> {
        //TODO: implement better clipboard mechanism
        if id & CLIPBOARD_FLAG == CLIPBOARD_FLAG {
            return self
                .handler
                .handle_clipboard_read(id & !CLIPBOARD_FLAG, buf);
        }

        let slice: &mut [Event] = unsafe {
            slice::from_raw_parts_mut(
                buf.as_mut_ptr() as *mut Event,
                buf.len() / mem::size_of::<Event>(),
            )
        };
        let n = self.handler.handle_window_read(id, slice)?;
        Ok(n * mem::size_of::<Event>())
    }
    fn write(
        &mut self,
        id: usize,
        buf: &[u8],
        _offset: u64,
        _flags: u32,
        _ctx: &CallerCtx,
    ) -> syscall::Result<usize> {
        //TODO: implement better clipboard mechanism
        if id & CLIPBOARD_FLAG == CLIPBOARD_FLAG {
            return self
                .handler
                .handle_clipboard_write(id & !CLIPBOARD_FLAG, buf);
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
                        self.handler.handle_window_async(id, false)?;
                        Ok(buf.len())
                    }
                    "1" => {
                        self.handler.handle_window_async(id, true)?;
                        Ok(buf.len())
                    }
                    _ => Err(syscall::Error::new(EINVAL)),
                },
                "D" => match data {
                    "" => {
                        self.handler.handle_window_drag(id)?;
                        Ok(buf.len())
                    }
                    //TODO: resize by dragging edge
                    // Comma separated
                    // B is bottom
                    // L is left
                    // R is right
                    // T is top
                    _ => Err(syscall::Error::new(EINVAL)),
                },
                "F" => {
                    let mut parts = data.split(',');
                    let flags = parts.next().unwrap_or("");
                    let value = match parts.next().unwrap_or("") {
                        "0" => false,
                        "1" => true,
                        _ => return Err(syscall::Error::new(EINVAL)),
                    };
                    for flag in flags.chars() {
                        self.handler.handle_window_set_flag(id, flag, value)?;
                    }
                    Ok(buf.len())
                }
                "M" => match data {
                    "C,0" => {
                        self.handler.handle_window_mouse_cursor(id, false)?;
                        Ok(buf.len())
                    }
                    "C,1" => {
                        self.handler.handle_window_mouse_cursor(id, true)?;
                        Ok(buf.len())
                    }
                    "G,0" => {
                        self.handler.handle_window_mouse_grab(id, false)?;
                        Ok(buf.len())
                    }
                    "G,1" => {
                        self.handler.handle_window_mouse_grab(id, true)?;
                        Ok(buf.len())
                    }
                    "R,0" => {
                        self.handler.handle_window_mouse_relative(id, false)?;
                        Ok(buf.len())
                    }
                    "R,1" => {
                        self.handler.handle_window_mouse_relative(id, true)?;
                        Ok(buf.len())
                    }
                    _ => Err(syscall::Error::new(EINVAL)),
                },
                "P" => {
                    let mut parts = data.split(',');
                    let x = parts.next().unwrap_or("").parse::<i32>().ok();
                    let y = parts.next().unwrap_or("").parse::<i32>().ok();

                    self.handler.handle_window_position(id, x, y)?;

                    Ok(buf.len())
                }
                "S" => {
                    let mut parts = data.split(',');
                    let w = parts.next().unwrap_or("").parse::<i32>().ok();
                    let h = parts.next().unwrap_or("").parse::<i32>().ok();

                    self.handler.handle_window_resize(id, w, h)?;

                    Ok(buf.len())
                }
                "T" => {
                    self.handler.handle_window_title(id, data.to_string())?;

                    Ok(buf.len())
                }
                _ => Err(syscall::Error::new(EINVAL)),
            }
        } else {
            Err(syscall::Error::new(EINVAL))
        }
    }
    fn fevent(
        &mut self,
        id: usize,
        _flags: EventFlags,
        _ctx: &CallerCtx,
    ) -> syscall::Result<EventFlags> {
        self.handler
            .handle_window_clear_notified(id)
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
    fn fpath(&mut self, id: usize, mut buf: &mut [u8], _ctx: &CallerCtx) -> syscall::Result<usize> {
        let props = self.handler.handle_window_properties(id)?;
        let original_len = buf.len();
        #[allow(clippy::write_literal)] // TODO: Z order
        let _ = write!(
            buf,
            "{}/{}/{}/{}/{}/{}",
            props.flags, props.x, props.y, props.width, props.height, props.title
        );
        Ok(original_len - buf.len())
    }
    fn fsync(&mut self, id: usize, _ctx: &CallerCtx) -> syscall::Result<()> {
        self.handler.handle_window_sync(id)
    }
    fn mmap_prep(
        &mut self,
        id: usize,
        _offset: u64,
        size: usize,
        _flags: syscall::MapFlags,
        _ctx: &CallerCtx,
    ) -> syscall::Result<usize> {
        //TODO: handle offset, flags?
        let data = self.handler.handle_window_map(id, true)?;

        if size > data.len() * core::mem::size_of::<Color>() {
            return Err(syscall::Error::new(EINVAL));
        }

        Ok(data.as_mut_ptr() as usize)
    }
    fn munmap(
        &mut self,
        id: usize,
        _offset: u64,
        _size: usize,
        _flags: syscall::MunmapFlags,
        _ctx: &CallerCtx,
    ) -> syscall::Result<()> {
        //TODO: handle offset, size, flags?
        self.handler.handle_window_unmap(id)?;

        Ok(())
    }
}
impl OrbitalHandler {
    fn on_close(&mut self, id: usize) {
        //TODO: implement better clipboard mechanism
        if id & CLIPBOARD_FLAG == CLIPBOARD_FLAG {
            return self.handler.handle_clipboard_close(id & !CLIPBOARD_FLAG);
        }

        self.handler.handle_window_close(id)
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
            }
            _ => panic!("Could not parse url"),
        }
    }

    #[test]
    fn valid_url_empty_path() {
        // until we throw an error for an empty scheme_name...
        match Orbital::url_parts("scheme:") {
            Ok((scheme_name, path)) => {
                assert_eq!(scheme_name, "scheme");
                assert!(path.is_empty());
            }
            _ => panic!("Could not parse url"),
        }
    }

    #[test]
    fn valid_url() {
        match Orbital::url_parts("scheme:path") {
            Ok((scheme_name, path)) => {
                assert_eq!(scheme_name, "scheme");
                assert_eq!(path, "path");
            }
            _ => panic!("Could not parse url"),
        }
    }
}
