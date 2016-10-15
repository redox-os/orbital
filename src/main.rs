#![feature(asm)]
#![feature(const_fn)]

extern crate core;
extern crate event;
extern crate orbclient;
extern crate orbimage;
extern crate syscall;

use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::{env, io, mem, str, thread};
use std::os::unix::io::AsRawFd;
use std::process::Command;
use std::sync::{Arc, Mutex};

use event::EventQueue;
use orbclient::event::{EVENT_KEY, EVENT_MOUSE, FocusEvent, QuitEvent};
use syscall::data::Packet;
use syscall::error::{Error, Result, EBADF, EINVAL};
use syscall::number::SYS_READ;
use syscall::scheme::SchemeMut;

use self::config::Config;

pub use orbclient::event::{Event, EventOption};
pub use self::color::Color;
pub use self::font::Font;
pub use self::image::{Image, ImageRoi};
pub use self::rect::Rect;
pub use self::socket::Socket;
pub use self::window::Window;

pub mod color;
pub mod config;
pub mod font;
pub mod image;
pub mod rect;
pub mod socket;
pub mod window;

fn schedule(redraws: &mut Vec<Rect>, request: Rect) {
    let mut push = true;
    for mut rect in redraws.iter_mut() {
        //If contained, ignore new redraw request
        let container = rect.container(&request);
        if container.area() <= rect.area() + request.area() {
            *rect = container;
            push = false;
            break;
        }
    }

    if push {
        redraws.push(request);
    }
}

struct OrbitalScheme {
    image: Image,
    background: Image,
    cursor: Image,
    cursor_x: i32,
    cursor_y: i32,
    dragging: bool,
    drag_x: i32,
    drag_y: i32,
    next_id: isize,
    next_x: i32,
    next_y: i32,
    order: VecDeque<usize>,
    windows: BTreeMap<usize, Window>,
    redraws: Vec<Rect>,
    todo: Vec<Packet>
}

impl OrbitalScheme {
    fn new(width: i32, height: i32, config: &Config) -> OrbitalScheme {
        OrbitalScheme {
            image: Image::new(width, height),
            background: Image::from_path(&config.background),
            cursor: Image::from_path(&config.cursor),
            cursor_x: 0,
            cursor_y: 0,
            dragging: false,
            drag_x: 0,
            drag_y: 0,
            next_id: 1,
            next_x: 20,
            next_y: 20,
            order: VecDeque::new(),
            windows: BTreeMap::new(),
            redraws: vec![Rect::new(0, 0, width, height)],
            todo: Vec::new()
        }
    }

    fn background_rect(&self) -> Rect {
        let w = self.background.width();
        let h = self.background.height();
        let x = self.image.width()/2 - w/2;
        let y = self.image.height()/2 - h/2;
        Rect::new(x, y, w, h)
    }

    fn cursor_rect(&self) -> Rect {
        Rect::new(self.cursor_x, self.cursor_y, self.cursor.width(), self.cursor.height())
    }

    fn screen_rect(&self) -> Rect {
        Rect::new(0, 0, self.image.width(), self.image.height())
    }

    fn redraw(&mut self, display: &Socket){
        let mut redraws = Vec::new();
        mem::swap(&mut self.redraws, &mut redraws);

        let screen_rect = self.screen_rect();

        for mut rect in redraws.iter_mut() {
            *rect = rect.intersection(&screen_rect);

            if ! rect.is_empty() {
                //TODO: only clear area not covered by background
                self.image.roi(&rect).set(Color::rgb(75, 163, 253));

                let background_rect = self.background_rect();
                let background_intersect = rect.intersection(&background_rect);
                if ! background_intersect.is_empty(){
                    self.image.roi(&background_intersect).blit(&self.background.roi(&background_intersect.offset(-background_rect.left(), -background_rect.top())));
                }

                let mut i = self.order.len();
                for id in self.order.iter().rev() {
                    i -= 1;
                    if let Some(mut window) = self.windows.get_mut(&id) {
                        window.draw_title(&mut self.image, &rect, i == 0);
                        window.draw(&mut self.image, &rect);
                    }
                }

                let cursor_rect = self.cursor_rect();
                let cursor_intersect = rect.intersection(&cursor_rect);
                if ! cursor_intersect.is_empty() {
                    self.image.roi(&cursor_intersect).blend(&self.cursor.roi(&cursor_intersect.offset(-cursor_rect.left(), -cursor_rect.top())));
                }
            }
        }

        /*
        for rect in redraws.iter_mut() {
            if ! rect.is_empty() {
                let data = self.image.data();
                for row in rect.top()..rect.bottom() {
                    let off1 = row * self.image.width() + rect.left();
                    let off2 = row * self.image.width() + rect.right();

                    unsafe { display.seek(SeekFrom::Start(off1 as u64 * 4)).unwrap(); }
                    display.send_type(&data[off1 as usize .. off2 as usize]).unwrap();
                }
            }
        }
        */
        display.send_type(self.image.data()).unwrap();
    }

    fn event(&mut self, event: Event){
        if event.code == EVENT_KEY {
            if let Some(id) = self.order.front() {
                if let Some(mut window) = self.windows.get_mut(&id) {
                    window.event(event);
                }
            }
        } else if event.code == EVENT_MOUSE {
            if event.a as i32 != self.cursor_x || event.b as i32 != self.cursor_y {
                let cursor_rect = self.cursor_rect();
                schedule(&mut self.redraws, cursor_rect);

                self.cursor_x = event.a as i32;
                self.cursor_y = event.b as i32;

                let cursor_rect = self.cursor_rect();
                schedule(&mut self.redraws, cursor_rect);
            }

            if self.dragging {
                if event.c > 0 {
                    if let Some(id) = self.order.front() {
                        if let Some(mut window) = self.windows.get_mut(&id) {
                            if self.drag_x != self.cursor_x || self.drag_y != self.cursor_y {
                                schedule(&mut self.redraws, window.title_rect());
                                schedule(&mut self.redraws, window.rect());
                                window.x += self.cursor_x - self.drag_x;
                                window.y += self.cursor_y - self.drag_y;
                                self.drag_x = self.cursor_x;
                                self.drag_y = self.cursor_y;
                                schedule(&mut self.redraws, window.title_rect());
                                schedule(&mut self.redraws, window.rect());
                            }
                        } else {
                            self.dragging = false;
                        }
                    } else {
                        self.dragging = false;
                    }
                } else {
                    self.dragging = false;
                }
            } else {
                let mut focus = 0;
                let mut i = 0;
                for id in self.order.iter() {
                    if let Some(mut window) = self.windows.get_mut(&id) {
                        if window.rect().contains(event.a as i32, event.b as i32) {
                            let mut window_event = event;
                            window_event.a -= window.x as i64;
                            window_event.b -= window.y as i64;
                            window.event(window_event);
                            if event.c > 0 {
                                focus = i;
                            }
                            break;
                        } else if window.title_rect().contains(event.a as i32, event.b as i32) {
                            if event.c > 0 {
                                focus = i;
                                if window.exit_contains(event.a as i32, event.b as i32) {
                                    window.event(QuitEvent.to_event());
                                } else {
                                    self.dragging = true;
                                    self.drag_x = self.cursor_x;
                                    self.drag_y = self.cursor_y;
                                }
                            }
                            break;
                        }
                    }
                    i += 1;
                }
                if focus > 0 {
                    //Redraw old focused window
                    if let Some(id) = self.order.front() {
                        if let Some(mut window) = self.windows.get_mut(&id){
                            schedule(&mut self.redraws, window.title_rect());
                            schedule(&mut self.redraws, window.rect());
                            window.event(FocusEvent {
                                focused: false
                            }.to_event());
                        }
                    }
                    //Redraw new focused window
                    if let Some(id) = self.order.remove(focus) {
                        if let Some(mut window) = self.windows.get_mut(&id){
                            schedule(&mut self.redraws, window.title_rect());
                            schedule(&mut self.redraws, window.rect());
                            window.event(FocusEvent {
                                focused: true
                            }.to_event());
                        }
                        self.order.push_front(id);
                    }
                }
            }
        }
    }
}

impl SchemeMut for OrbitalScheme {
    fn open(&mut self, url: &[u8], _flags: usize, _uid: u32, _gid: u32) -> Result<usize> {
        let path = try!(str::from_utf8(url).or(Err(Error::new(EINVAL))));
        let mut parts = path.split("/");

        let flags = parts.next().unwrap_or("");

        let mut async = false;
        for flag in flags.chars() {
            if flag == 'a' {
                async = true;
            }
        }

        let mut x = parts.next().unwrap_or("").parse::<i32>().unwrap_or(0);
        let mut y = parts.next().unwrap_or("").parse::<i32>().unwrap_or(0);
        let width = parts.next().unwrap_or("").parse::<i32>().unwrap_or(0);
        let height = parts.next().unwrap_or("").parse::<i32>().unwrap_or(0);

        let mut title = parts.next().unwrap_or("").to_string();
        for part in parts {
            title.push('/');
            title.push_str(part);
        }

        let id = self.next_id as usize;
        self.next_id += 1;
        if self.next_id < 0 {
            self.next_id = 1;
        }

        if x < 0 && y < 0 {
            x = self.next_x;
            y = self.next_y;

            self.next_x += 20;
            if self.next_x + 20 >= self.image.width() {
                self.next_x = 20;
            }
            self.next_y += 20;
            if self.next_y + 20 >= self.image.height() {
                self.next_y = 20;
            }
        }

        if let Some(id) = self.order.front() {
            if let Some(window) = self.windows.get(&id){
                schedule(&mut self.redraws, window.title_rect());
                schedule(&mut self.redraws, window.rect());
            }
        }

        let window = Window::new(x, y, width, height, title, async);
        schedule(&mut self.redraws, window.title_rect());
        schedule(&mut self.redraws, window.rect());
        self.order.push_front(id);
        self.windows.insert(id, window);

        Ok(id)
    }

    fn read(&mut self, id: usize, buf: &mut [u8]) -> Result<usize> {
        if let Some(mut window) = self.windows.get_mut(&id) {
            window.read(buf)
        } else {
            Err(Error::new(EBADF))
        }
    }

    fn write(&mut self, id: usize, buf: &[u8]) -> Result<usize> {
        if let Some(mut window) = self.windows.get_mut(&id) {
            schedule(&mut self.redraws, window.rect());
            window.write(buf)
        } else {
            Err(Error::new(EBADF))
        }
    }

    fn fpath(&mut self, id: usize, buf: &mut [u8]) -> Result<usize> {
        if let Some(window) = self.windows.get(&id) {
            window.path(buf)
        } else {
            Err(Error::new(EBADF))
        }
    }

    fn close(&mut self, id: usize) -> Result<usize> {
        self.order.retain(|&e| e != id);

        if let Some(id) = self.order.front() {
            if let Some(window) = self.windows.get(&id){
                schedule(&mut self.redraws, window.title_rect());
                schedule(&mut self.redraws, window.rect());
            }
        }

        if let Some(window) = self.windows.remove(&id) {
            schedule(&mut self.redraws, window.title_rect());
            schedule(&mut self.redraws, window.rect());
            Ok(0)
        } else {
            Err(Error::new(EBADF))
        }
    }
}

fn run(scheme_cell: Arc<RefCell<OrbitalScheme>>, display: Arc<Socket>, socket: Arc<Socket>) {
    {
        let mut scheme = scheme_cell.borrow_mut();
        scheme.redraw(&display);
    }

    let mut event_queue = EventQueue::<()>::new().unwrap();

    let scheme_event = scheme_cell.clone();
    let display_event = display.clone();
    let socket_event = socket.clone();
    let display_fd = display.as_raw_fd();
    event_queue.add(display_fd, move |_count: usize| -> io::Result<Option<()>> {
        let mut event = Event::new();
        if display_event.receive(&mut event)? == mem::size_of::<Event>() {
            let mut scheme = scheme_event.borrow_mut();

            scheme.event(event);

            let mut i = 0;
            while i < scheme.todo.len() {
                let mut packet = scheme.todo[i];

                let delay = if packet.a == SYS_READ {
                    if let Some(window) = scheme.windows.get(&packet.b) {
                        window.async == false
                    } else {
                        true
                    }
                } else {
                    false
                };

                scheme.handle(&mut packet);

                if delay && packet.a == 0 {
                    i += 1;
                }else{
                    socket_event.send(&packet)?;
                    scheme.todo.remove(i);
                }
            }

            scheme.redraw(&display_event);
        }

        Ok(None)
    }).unwrap();

    let socket_fd = socket.as_raw_fd();
    event_queue.add(socket_fd, move |_count: usize| -> io::Result<Option<()>> {
        let mut packet = Packet::default();
        if socket.receive(&mut packet)? == mem::size_of::<Packet>() {
            let mut scheme = scheme_cell.borrow_mut();

            let delay = if packet.a == SYS_READ {
                if let Some(window) = scheme.windows.get(&packet.b) {
                    window.async == false
                } else {
                    true
                }
            } else {
                false
            };

            scheme.handle(&mut packet);

            if delay && packet.a == 0 {
                packet.a = SYS_READ;
                scheme.todo.push(packet);
            } else {
                socket.send(&packet)?;
            }

            scheme.redraw(&display);
        }

        Ok(None)
    }).unwrap();

    event_queue.run().unwrap();
}

enum Status {
    Starting,
    Running,
    Stopping
}

fn main() {
    let display_path = env::args().nth(1).expect("orbital: no display argument");

    env::set_var("DISPLAY", &display_path);

    let status_mutex = Arc::new(Mutex::new(Status::Starting));

    let status_daemon = status_mutex.clone();
    thread::spawn(move || {
        match Socket::create(":orbital").map(|socket| Arc::new(socket)) {
            Ok(socket) => match Socket::open(&display_path).map(|display| Arc::new(display)) {
                Ok(display) => {
                    let path = display.path().map(|path| path.into_os_string().into_string().unwrap_or(String::new())).unwrap_or(String::new());
                    let res = path.split(":").nth(1).unwrap_or("");
                    let width = res.split("/").nth(1).unwrap_or("").parse::<i32>().unwrap_or(0);
                    let height = res.split("/").nth(2).unwrap_or("").parse::<i32>().unwrap_or(0);

                    println!("orbital: found display {}x{}", width, height);

                    let config = Config::from_path("/etc/orbital.conf");

                    let scheme = Arc::new(RefCell::new(OrbitalScheme::new(width, height, &config)));

                    *status_daemon.lock().unwrap() = Status::Running;

                    run(scheme, display, socket);
                },
                Err(err) => println!("orbital: no display found: {}", err)
            },
            Err(err) => println!("orbital: could not register orbital: {}", err)
        }

        *status_daemon.lock().unwrap() = Status::Stopping;
    });

    'waiting: loop {
        match *status_mutex.lock().unwrap() {
            Status::Starting => (),
            Status::Running => {
                Command::new("orblogin").spawn().expect("orbital: failed to spawn launcher");
                break 'waiting;
            },
            Status::Stopping => break 'waiting,
        }

        thread::sleep_ms(30);
    }
}
