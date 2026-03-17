use std::{
    collections::{HashMap, VecDeque},
    io::{self, Write},
    mem,
    os::unix::io::AsRawFd,
    slice, str,
};

use event::{EventQueue, user_data};
use graphics_ipc::v2::V2GraphicsHandle;
use inputd::{ConsumerHandle, ConsumerHandleEvent};
use log::error;
use orbclient::{Color, Event};
use redox_scheme::{
    CallerCtx, OpenResult, RequestKind, Response, SignalBehavior, Socket,
    scheme::{IntoTag, Op, OpRead, SchemeState, SchemeSync, register_scheme_inner},
};
use syscall::{
    EACCES, EAGAIN, EBADF, ECANCELED, EINVAL, EOPNOTSUPP, EWOULDBLOCK, flag::EventFlags,
    schemev2::NewFdFlags,
};

use crate::core::display::Displays;
use crate::scheme::OrbitalScheme;

pub(crate) mod display;
pub(crate) mod image;
pub(crate) mod rect;

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

    /// Handle to "/scheme/input/consumer" to receive input events.
    pub input: ConsumerHandle,
}

impl Orbital {
    /// Open an orbital display and connect to the scheme
    pub fn open_display() -> io::Result<(Self, Displays)> {
        let input_handle = ConsumerHandle::new_vt()?;

        let display = input_handle.open_display_v2().map_err(|err| {
            error!("failed to open display: {}", err);
            err
        })?;

        let scheme = Socket::nonblock().map_err(|err| {
            error!("failed to create scheme: {}", err);
            err
        })?;

        let displays = Displays::new(V2GraphicsHandle::from_file(display)?)?;

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
    pub fn scheme_write(&self, response: Response) -> io::Result<()> {
        self.scheme
            .write_response(response, SignalBehavior::Restart)?;
        Ok(())
    }

    /// Start the main loop
    pub fn run(
        self,
        handler: OrbitalScheme,
        login_cmd: &mut std::process::Command,
    ) -> Result<(), Error> {
        user_data! {
            enum Source {
                Scheme,
                Input,
            }
        }

        let event_queue = EventQueue::<Source>::new()?;

        //TODO: Figure out why rand: gets opened after this: libredox::call::setrens(0, 0)?;

        let scheme_fd = self.scheme.inner().raw();
        let input_fd = self.input.event_handle().as_raw_fd();

        let mut state = SchemeState::new();
        let mut me = OrbitalHandler {
            orb: self,
            handler,
            handles: HashMap::new(),
            next_id: 0,
        };
        let cap_id = me.scheme_root()?;
        register_scheme_inner(&mut me.orb.scheme, "orbital", cap_id)?;

        unsafe {
            // FIXME remove DISPLAY env var once orbclient no longer depends on it
            std::env::set_var("DISPLAY", "orbital:99.0");

            std::env::set_var("ORBITAL_DISPLAY", "/scheme/orbital")
        };

        event_queue.subscribe(scheme_fd, Source::Scheme, event::EventFlags::READ)?;
        event_queue.subscribe(input_fd as usize, Source::Input, event::EventFlags::READ)?;

        login_cmd.spawn()?;

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
                                let should_delay = me.should_delay(read_op.fd);
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
                                let resp = op.handle_sync(caller_ctx, &mut me, &mut state);
                                me.orb.scheme_write(resp)?;
                            }
                        }
                        me.handler.handle_after(&mut me.orb)?;
                    }
                }
                Source::Input => {
                    let mut events = [Event::new(); 16];
                    loop {
                        match me.orb.input.read_events(&mut events)? {
                            ConsumerHandleEvent::Events(&[]) => break,
                            ConsumerHandleEvent::Events(events) => {
                                let mut delayed_left = me.orb.delayed.len();

                                while delayed_left > 0
                                    && let Some((ctx, mut read_op)) = me.orb.delayed.pop_front()
                                {
                                    delayed_left -= 1;

                                    let should_delay = me.should_delay(read_op.fd);

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

                                me.handler.handle_input(events);
                            }
                            ConsumerHandleEvent::Handoff => {}
                        }
                    }
                    me.handler.handle_after(&mut me.orb)?;
                }
            }
        }

        //TODO: Cleanup and handle TODO
        Ok(())
    }
}
enum Handle {
    SchemeRoot,
    DisplaySize(usize),
    Window(usize),
    Clipboard(usize),
}
pub struct OrbitalHandler {
    orb: Orbital,
    handler: OrbitalScheme,
    handles: HashMap<usize, Handle>,
    next_id: usize,
}
impl SchemeSync for OrbitalHandler {
    fn scheme_root(&mut self) -> syscall::Result<usize> {
        let new_id = self.next_id;
        self.handles.insert(new_id, Handle::SchemeRoot);
        self.next_id += 1;
        Ok(new_id)
    }
    fn openat(
        &mut self,
        dirfd: usize,
        path: &str,
        _flags: usize,
        _fcntl_flags: u32,
        _ctx: &CallerCtx,
    ) -> syscall::Result<OpenResult> {
        {
            let Some(handle) = self.handles.get(&dirfd) else {
                return Err(syscall::Error::new(EBADF));
            };
            if !matches!(handle, Handle::SchemeRoot) {
                return Err(syscall::Error::new(EACCES));
            }
        }

        // FIXME remove once orbclient no longer depends on the DISPLAY env var
        if let Some(display) = path.strip_prefix("99.") {
            let display = display.parse().map_err(|_| syscall::Error::new(EINVAL))?;
            if display >= self.handler.display_count() {
                return Err(syscall::Error::new(EINVAL));
            }

            let new_id = self.next_id;
            self.handles.insert(new_id, Handle::DisplaySize(display));
            self.next_id += 1;
            return Ok(OpenResult::ThisScheme {
                number: new_id,
                flags: NewFdFlags::empty(),
            });
        }

        let mut parts = path.split('/');

        let path_first_char = path.chars().nth(0).unwrap_or('\0');
        let flags = if path_first_char.is_ascii_digit() || path_first_char == '-' {
            // to handle case like `/scheme/orbital//` being assumed as one slash
            ""
        } else {
            parts.next().unwrap_or("")
        };

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
        let new_id = self.next_id;
        self.handles.insert(new_id, Handle::Window(id));
        self.next_id += 1;
        Ok(OpenResult::ThisScheme {
            number: new_id,
            flags: NewFdFlags::empty(),
        })
    }
    fn dup(&mut self, id: usize, buf: &[u8], _ctx: &CallerCtx) -> syscall::Result<OpenResult> {
        let Some(&Handle::Window(id) | &Handle::Clipboard(id)) = self.handles.get(&id) else {
            return Err(syscall::Error::new(EBADF));
        };
        if buf == b"clipboard" {
            //TODO: implement better clipboard mechanism
            let id = self.handler.handle_clipboard_new(id)?;
            let new_id = self.next_id;
            self.handles.insert(new_id, Handle::Clipboard(id));
            self.next_id += 1;
            Ok(OpenResult::ThisScheme {
                number: new_id,
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
        let Some(handle) = self.handles.get(&id) else {
            return Err(syscall::Error::new(EBADF));
        };
        //TODO: implement better clipboard mechanism
        let id = match *handle {
            Handle::Clipboard(id) => return self.handler.handle_clipboard_read(id, buf),
            Handle::Window(id) => id,
            Handle::SchemeRoot | Handle::DisplaySize(_) => return Err(syscall::Error::new(EBADF)),
        };

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
        let Some(handle) = self.handles.get(&id) else {
            return Err(syscall::Error::new(EBADF));
        };
        //TODO: implement better clipboard mechanism
        let id = match *handle {
            Handle::Clipboard(id) => return self.handler.handle_clipboard_write(id, buf),
            Handle::Window(id) => id,
            Handle::SchemeRoot | Handle::DisplaySize(_) => return Err(syscall::Error::new(EBADF)),
        };

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
        let Some(&Handle::Window(id) | &Handle::Clipboard(id)) = self.handles.get(&id) else {
            return Err(syscall::Error::new(EBADF));
        };
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
        match self.handles.get(&id) {
            Some(&Handle::DisplaySize(display)) => {
                let (width, height) = self.handler.display_size(display);
                let original_len = buf.len();
                let _ = write!(buf, "orbital:99.{display}/{}/{}", width, height);
                Ok(original_len - buf.len())
            }
            Some(&Handle::Window(id) | &Handle::Clipboard(id)) => {
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
            _ => Err(syscall::Error::new(EBADF)),
        }
    }
    fn fsync(&mut self, id: usize, _ctx: &CallerCtx) -> syscall::Result<()> {
        let Some(&Handle::Window(id) | &Handle::Clipboard(id)) = self.handles.get(&id) else {
            return Err(syscall::Error::new(EBADF));
        };
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
        let Some(&Handle::Window(id) | &Handle::Clipboard(id)) = self.handles.get(&id) else {
            return Err(syscall::Error::new(EBADF));
        };
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
        let Some(&Handle::Window(id) | &Handle::Clipboard(id)) = self.handles.get(&id) else {
            return Err(syscall::Error::new(EBADF));
        };
        //TODO: handle offset, size, flags?
        self.handler.handle_window_unmap(id)?;

        Ok(())
    }
}
impl OrbitalHandler {
    fn should_delay(&self, id: usize) -> bool {
        if let Some(handle) = self.handles.get(&id) {
            match *handle {
                Handle::Clipboard(id) | Handle::Window(id) => self.handler.should_delay(id),
                Handle::SchemeRoot | Handle::DisplaySize(_) => false,
            }
        } else {
            false
        }
    }

    fn on_close(&mut self, id: usize) {
        let Some(handle) = self.handles.get(&id) else {
            return;
        };
        //TODO: implement better clipboard mechanism
        let id = match *handle {
            Handle::Clipboard(id) => return self.handler.handle_clipboard_close(id),
            Handle::Window(id) => id,
            Handle::SchemeRoot | Handle::DisplaySize(_) => return,
        };

        self.handler.handle_window_close(id)
    }
}
