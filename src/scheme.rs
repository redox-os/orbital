use orbclient::{
    self, Color, Event, EventOption, KeyEvent, MouseEvent, MouseRelativeEvent, ButtonEvent,
    ClipboardEvent, FocusEvent, QuitEvent, MoveEvent, ResizeEvent, ScreenEvent, Renderer
};
use orbfont;
use syscall;

use orbital_core::{
    Handler,
    Orbital,
    Properties,
    image::{Image},
    rect::Rect
};
use std::{
    cmp,
    collections::{
        BTreeMap,
        VecDeque
    },
    io,
    mem,
    str
};
use syscall::data::Packet;
use syscall::error::{Error, Result, EBADF};
use syscall::number::SYS_READ;

use config::Config;
use theme::{BACKGROUND_COLOR, BAR_COLOR, BAR_HIGHLIGHT_COLOR, TEXT_COLOR, TEXT_HIGHLIGHT_COLOR};
use window::{Window, WindowZOrder};

fn schedule(redraws: &mut Vec<Rect>, request: Rect) {
    let mut push = true;
    for rect in redraws.iter_mut() {
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

#[derive(Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
enum CursorKind {
    None,
    LeftPtr,
    BottomLeftCorner,
    BottomRightCorner,
    BottomSide,
    LeftSide,
    RightSide,
}

enum DragMode {
    None,
    Title(usize, i32, i32),
    LeftBorder(usize, i32, i32),
    RightBorder(usize, i32),
    BottomBorder(usize, i32),
    BottomLeftBorder(usize, i32, i32, i32),
    BottomRightBorder(usize, i32, i32),
}

pub struct OrbitalScheme {
    window_max: Image,
    window_max_unfocused: Image,
    window_close: Image,
    window_close_unfocused: Image,
    cursors: BTreeMap<CursorKind, Image>,
    cursor_i: CursorKind,
    cursor_x: i32,
    cursor_y: i32,
    cursor_left: bool,
    cursor_middle: bool,
    cursor_right: bool,
    dragging: DragMode,
    win_key: bool,
    win_tabbing: bool,
    next_id: isize,
    order: VecDeque<usize>,
    zbuffer: Vec<(usize, WindowZOrder, usize)>,
    pub windows: BTreeMap<usize, Window>,
    redraws: Vec<Rect>,
    font: orbfont::Font,
    clipboard: Vec<u8>,
    scale: i32,
}

impl OrbitalScheme {
    pub fn new(width: i32, height: i32, config: &Config) -> OrbitalScheme {
        let scale = (height / 1600) + 1;

        let mut cursors = BTreeMap::new();
        cursors.insert(CursorKind::None, Image::new(0, 0));
        cursors.insert(CursorKind::LeftPtr, Image::from_path_scale(&config.cursor, scale).unwrap_or(Image::new(0, 0)));
        cursors.insert(CursorKind::BottomLeftCorner, Image::from_path_scale(&config.bottom_left_corner, scale).unwrap_or(Image::new(0, 0)));
        cursors.insert(CursorKind::BottomRightCorner, Image::from_path_scale(&config.bottom_right_corner, scale).unwrap_or(Image::new(0, 0)));
        cursors.insert(CursorKind::BottomSide, Image::from_path_scale(&config.bottom_side, scale).unwrap_or(Image::new(0, 0)));
        cursors.insert(CursorKind::LeftSide, Image::from_path_scale(&config.left_side, scale).unwrap_or(Image::new(0, 0)));
        cursors.insert(CursorKind::RightSide, Image::from_path_scale(&config.right_side, scale).unwrap_or(Image::new(0, 0)));

        OrbitalScheme {
            window_max: Image::from_path_scale(&config.window_max, scale).unwrap_or(Image::new(0, 0)),
            window_max_unfocused: Image::from_path_scale(&config.window_max_unfocused, scale).unwrap_or(Image::new(0, 0)),
            window_close: Image::from_path_scale(&config.window_close, scale).unwrap_or(Image::new(0, 0)),
            window_close_unfocused: Image::from_path_scale(&config.window_close_unfocused, scale).unwrap_or(Image::new(0, 0)),
            cursors: cursors,
            cursor_i: CursorKind::LeftPtr,
            cursor_x: 0,
            cursor_y: 0,
            cursor_left: false,
            cursor_middle: false,
            cursor_right: false,
            dragging: DragMode::None,
            win_key: false,
            // Is the user currently switching windows with win-tab
            // Set true when win-tab is pressed, set false when win is released.
            // While it is true, redraw() calls draw_window_list()
            win_tabbing: false,
            next_id: 1,
            order: VecDeque::new(),
            zbuffer: Vec::new(),
            windows: BTreeMap::new(),
            redraws: vec![Rect::new(0, 0, width, height)],
            font: orbfont::Font::find(Some("Sans"), None, None).unwrap(),
            clipboard: Vec::new(),
            scale,
        }
    }

    pub fn with_orbital<'a>(&'a mut self, orb: &'a mut Orbital) -> OrbitalSchemeEvent<'a> {
        OrbitalSchemeEvent {
            scheme: self,
            orb: orb
        }
    }

    fn cursor_rect(&self) -> Rect {
        let cursor = &self.cursors[&self.cursor_i];
        let (off_x, off_y) = match self.cursor_i {
            CursorKind::None => (0, 0),
            CursorKind::LeftPtr => (0, 0),
            CursorKind::BottomLeftCorner => (0, -cursor.height()),
            CursorKind::BottomRightCorner => (-cursor.width(), -cursor.height()),
            CursorKind::BottomSide => (-cursor.width()/2, -cursor.height()),
            CursorKind::LeftSide => (0, -cursor.height()/2),
            CursorKind::RightSide => (-cursor.width(), -cursor.height()/2),
        };
        Rect::new(self.cursor_x + off_x, self.cursor_y + off_y, cursor.width(), cursor.height())
    }

    fn rezbuffer(&mut self) {
        self.zbuffer.clear();

        for (i, id) in self.order.iter().enumerate() {
            if let Some(window) = self.windows.get(id) {
                self.zbuffer.push((*id, window.zorder, i));
            }
        }

        self.zbuffer.sort_by(|a, b| b.1.cmp(&a.1));
    }
}
impl Handler for OrbitalScheme {
    fn should_delay(&mut self, packet: &Packet) -> bool {
        packet.a == SYS_READ &&
            self.windows.get(&packet.b)
                .map(|window| !window.async)
                .unwrap_or(true)
    }

    fn handle_scheme(&mut self, orb: &mut Orbital, packets: &mut [Packet]) -> io::Result<()> {
        self.with_orbital(orb).scheme_event(packets)
    }
    fn handle_display(&mut self, orb: &mut Orbital, events: &mut [Event]) -> io::Result<()> {
        self.with_orbital(orb).display_event(events)
    }
    fn handle_after(&mut self, orb: &mut Orbital) -> io::Result<()> {
        self.with_orbital(orb).redraw();
        Ok(())
    }

    fn handle_window_new(&mut self, orb: &mut Orbital,
                         x: i32, y: i32, width: i32, height: i32,
                         parts: &str, title: String) -> syscall::Result<usize> {
        self.with_orbital(orb).window_new(x, y, width, height, parts, title)
    }
    fn handle_window_read(&mut self, _orb: &mut Orbital, id: usize, buf: &mut [Event]) -> syscall::Result<usize>
    {
        if let Some(window) = self.windows.get_mut(&id) {
            Ok(window.read(buf))
        } else {
            Err(Error::new(EBADF))
        }
    }
    fn handle_window_mouse_cursor(&mut self, _orb: &mut Orbital, id: usize, visible: bool) -> syscall::Result<()> {
        if let Some(window) = self.windows.get_mut(&id) {
            window.mouse_cursor = visible;
            Ok(())
        } else {
            Err(Error::new(EBADF))
        }
    }
    fn handle_window_mouse_grab(&mut self, _orb: &mut Orbital, id: usize, grab: bool) -> syscall::Result<()> {
        if let Some(window) = self.windows.get_mut(&id) {
            window.mouse_grab = grab;
            Ok(())
        } else {
            Err(Error::new(EBADF))
        }
    }
    fn handle_window_mouse_relative(&mut self, _orb: &mut Orbital, id: usize, relative: bool) -> syscall::Result<()> {
        if let Some(window) = self.windows.get_mut(&id) {
            window.mouse_relative = relative;
            Ok(())
        } else {
            Err(Error::new(EBADF))
        }
    }
    fn handle_window_position(&mut self, _orb: &mut Orbital, id: usize, x: Option<i32>, y: Option<i32>) -> syscall::Result<()> {
        if let Some(window) = self.windows.get_mut(&id) {
            window.x = x.unwrap_or(window.x);
            window.y = y.unwrap_or(window.y);

            schedule(&mut self.redraws, window.title_rect());
            schedule(&mut self.redraws, window.rect());

            Ok(())
        } else {
            Err(Error::new(EBADF))
        }
    }
    fn handle_window_resize(&mut self, _orb: &mut Orbital, id: usize, w: Option<i32>, h: Option<i32>) -> syscall::Result<()> {
        if let Some(window) = self.windows.get_mut(&id) {
            schedule(&mut self.redraws, window.title_rect());
            schedule(&mut self.redraws, window.rect());

            let w = w.unwrap_or(window.width());
            let h = h.unwrap_or(window.height());

            window.set_size(w, h);

            schedule(&mut self.redraws, window.title_rect());
            schedule(&mut self.redraws, window.rect());

            Ok(())
        } else {
            Err(Error::new(EBADF))
        }
    }
    fn handle_window_title(&mut self, _orb: &mut Orbital, id: usize, title: String) -> syscall::Result<()> {
        if let Some(window) = self.windows.get_mut(&id) {
            window.title = title;
            window.render_title(&self.font);

            schedule(&mut self.redraws, window.title_rect());

            Ok(())
        } else {
            Err(Error::new(EBADF))
        }
    }
    fn handle_window_clear_notified(&mut self, _orb: &mut Orbital, id: usize) -> syscall::Result<()> {
        if let Some(window) = self.windows.get_mut(&id) {
            window.notified_read = false;
            Ok(())
        } else {
            Err(syscall::Error::new(EBADF))
        }
    }
    fn handle_window_map(&mut self, _orb: &mut Orbital, id: usize) -> syscall::Result<&mut [Color]> {
        if let Some(window) = self.windows.get_mut(&id) {
            Ok(window.map())
        } else {
            Err(Error::new(EBADF))
        }
    }
    fn handle_window_properties(&mut self, _orb: &mut Orbital, id: usize) -> syscall::Result<Properties> {
        if let Some(window) = self.windows.get(&id) {
            Ok(window.properties())
        } else {
            Err(Error::new(EBADF))
        }
    }
    fn handle_window_sync(&mut self, _orb: &mut Orbital, id: usize) -> syscall::Result<usize> {
        if let Some(window) = self.windows.get(&id) {
            schedule(&mut self.redraws, window.rect());
            Ok(0)
        } else {
            Err(Error::new(EBADF))
        }
    }
    fn handle_window_close(&mut self, _orb: &mut Orbital, id: usize) -> syscall::Result<usize> {
        self.order.retain(|&e| e != id);

        if let Some(id) = self.order.front() {
            if let Some(window) = self.windows.get(&id){
                schedule(&mut self.redraws, window.title_rect());
                schedule(&mut self.redraws, window.rect());
            }
        }

        let res = if let Some(window) = self.windows.remove(&id) {
            schedule(&mut self.redraws, window.title_rect());
            schedule(&mut self.redraws, window.rect());
            Ok(0)
        } else {
            Err(Error::new(EBADF))
        };

        res
    }

    fn handle_clipboard_new(&mut self, _orb: &mut Orbital, id: usize) -> syscall::Result<usize> {
        //TODO: implement better clipboard mechanism
        if let Some(window) = self.windows.get_mut(&id) {
            window.clipboard_seek = 0;
            Ok(id)
        } else {
            Err(Error::new(EBADF))
        }
    }

    fn handle_clipboard_read(&mut self, _orb: &mut Orbital, id: usize, buf: &mut [u8]) -> syscall::Result<usize> {
        //TODO: implement better clipboard mechanism
        if let Some(window) = self.windows.get_mut(&id) {
            let mut i = 0;
            while i < buf.len() && window.clipboard_seek < self.clipboard.len() {
                buf[i] = self.clipboard[i];
                i += 1;
                window.clipboard_seek += 1;
            }
            Ok(i)
        } else {
            Err(Error::new(EBADF))
        }
    }

    fn handle_clipboard_write(&mut self, _orb: &mut Orbital, id: usize, buf: &[u8]) -> syscall::Result<usize> {
        //TODO: implement better clipboard mechanism
        if let Some(window) = self.windows.get_mut(&id) {
            let mut i = 0;
            self.clipboard.truncate(window.clipboard_seek);
            while i < buf.len() {
                self.clipboard.push(buf[i]);
                i += 1;
                window.clipboard_seek += 1;
            }
            Ok(i)
        } else {
            Err(Error::new(EBADF))
        }
    }

    fn handle_clipboard_close(&mut self, _orb: &mut Orbital, id: usize) -> syscall::Result<usize> {
        //TODO: implement better clipboard mechanism
        if self.windows.contains_key(&id) {
            Ok(0)
        } else {
            Err(Error::new(EBADF))
        }
    }
}
pub struct OrbitalSchemeEvent<'a> {
    scheme: &'a mut OrbitalScheme,
    orb: &'a mut Orbital
}
impl<'a> OrbitalSchemeEvent<'a> {
    pub fn redraw(&mut self) {
        self.scheme.rezbuffer();

        let screen_rect = self.orb.screen_rect();
        let cursor_rect = self.scheme.cursor_rect();

        for mut rect in self.scheme.redraws.drain(..) {
            rect = rect.intersection(&screen_rect);

            if ! rect.is_empty() {
                self.orb.image.rect(rect.left(), rect.top(),
                                rect.width() as u32, rect.height() as u32,
                                BACKGROUND_COLOR);

                for entry in self.scheme.zbuffer.iter().rev() {
                    let id = entry.0;
                    let i = entry.2;
                    if let Some(window) = self.scheme.windows.get_mut(&id) {
                        window.draw_title(&mut self.orb.image, &rect, i == 0, if i == 0 {
                            &mut self.scheme.window_max
                        } else {
                            &mut self.scheme.window_max_unfocused
                        }, if i == 0 {
                            &mut self.scheme.window_close
                        } else {
                            &mut self.scheme.window_close_unfocused
                        });
                        window.draw(&mut self.orb.image, &rect);
                    }
                }

                let cursor_intersect = rect.intersection(&cursor_rect);
                if ! cursor_intersect.is_empty() {
                    if let Some(cursor) = self.scheme.cursors.get_mut(&self.scheme.cursor_i) {
                        self.orb.image.roi(&cursor_intersect).blend(&cursor.roi(&cursor_intersect.offset(-cursor_rect.left(), -cursor_rect.top())));
                    }
                }
            }
        }

        if self.scheme.win_tabbing {
            self.draw_window_list();
        }

        self.orb.display_sync().unwrap();
    }

    fn win_tab(&mut self) {
        if self.scheme.order.len() > 1 {
            // Disable dragging
            self.scheme.dragging = DragMode::None;

            // Redraw old focused window
            if let Some(id) = self.scheme.order.pop_front() {
                if let Some(window) = self.scheme.windows.get_mut(&id) {
                    schedule(&mut self.scheme.redraws, window.title_rect());
                    schedule(&mut self.scheme.redraws, window.rect());
                    window.event(FocusEvent {
                        focused: false
                    }.to_event());
                }
                self.scheme.order.push_back(id);
            }
            // Redraw new focused window
            if let Some(id) = self.scheme.order.front() {
                if let Some(window) = self.scheme.windows.get_mut(&id){
                    schedule(&mut self.scheme.redraws, window.title_rect());
                    schedule(&mut self.scheme.redraws, window.rect());
                    window.event(FocusEvent {
                        focused: true
                    }.to_event());
                }
            }
        }
    }

    /// Draws a list of currently open windows in the middle of the screen
    fn draw_window_list(&mut self) {
        //TODO: HiDPI

        let mut rendered_text: Vec<orbfont::Text> = vec![];
        for id in self.scheme.order.iter() {
            if let Some(window) = self.scheme.windows.get(&id) {
                if window.title.is_empty() {
                    rendered_text.push(self.scheme.font.render(&format!("[unnamed #{}]", id), 16.0));
                } else {
                    rendered_text.push(self.scheme.font.render(&format!("{}", &window.title), 16.0));
                }
            }
        }

        let list_h = rendered_text.len() as i32 * 20 + 4;
        let list_w = 400;
        let target_rect = Rect::new(self.orb.image.width()/2 - list_w/2,
                                    self.orb.image.height()/2 - list_h/2,
                                    list_w, list_h);
        // Color copied over from orbtk's window background
        let mut image = Image::from_color(list_w, list_h, BAR_COLOR);
        for (i, text) in rendered_text.iter().enumerate() {
            if i == 0 {
                image.rect(0, i as i32 * 20 + 2, list_w as u32, 20, BAR_HIGHLIGHT_COLOR);
                text.draw(&mut image, 4, i as i32 * 20 + 4, TEXT_HIGHLIGHT_COLOR);
            } else {
                text.draw(&mut image, 4, i as i32 * 20 + 4, TEXT_COLOR);
            }
        }
        self.orb.image.roi(&target_rect).blit(&image.roi(&Rect::new(0, 0, list_w, list_h)));
        schedule(&mut self.scheme.redraws, target_rect);
    }

    fn key_event(&mut self, event: KeyEvent) {
        if event.scancode == 0x38 {
            self.scheme.win_key = event.pressed;
            // If the win key was released, stop drawing the win-tab window switcher
            if !self.scheme.win_key {
                self.scheme.win_tabbing = false;
            }
        } else if self.scheme.win_key {
            match event.scancode {
                orbclient::K_ESC => if event.pressed {
                    if let Some(id) = self.scheme.order.front() {
                        if let Some(window) = self.scheme.windows.get_mut(&id) {
                            window.event(QuitEvent.to_event());
                        }
                    }
                },
                orbclient::K_TAB => if event.pressed {
                    // Start drawing the window switcher. It's drawn by redraw()
                    self.scheme.win_tabbing = true;
                    self.win_tab();
                },
                orbclient::K_UP | orbclient::K_DOWN | orbclient::K_LEFT | orbclient::K_RIGHT => if event.pressed {
                    if let Some(id) = self.scheme.order.front() {
                        if let Some(window) = self.scheme.windows.get_mut(&id) {
                            if ! window.borderless {
                                schedule(&mut self.scheme.redraws, window.title_rect());
                                schedule(&mut self.scheme.redraws, window.rect());

                                match event.scancode {
                                    orbclient::K_LEFT => window.x -= 1,
                                    orbclient::K_RIGHT => window.x += 1,
                                    orbclient::K_UP => window.y -= 1,
                                    orbclient::K_DOWN => window.y += 1,
                                    _ => ()
                                }

                                let move_event = MoveEvent {
                                    x: window.x,
                                    y: window.y
                                }.to_event();
                                window.event(move_event);

                                schedule(&mut self.scheme.redraws, window.title_rect());
                                schedule(&mut self.scheme.redraws, window.rect());
                            }
                        }
                    }
                },
                orbclient::K_C => if event.pressed {
                    if let Some(id) = self.scheme.order.front() {
                        if let Some(window) = self.scheme.windows.get_mut(&id) {
                            //TODO: set window's clipboard to primary
                            let clipboard_event = ClipboardEvent {
                                kind: orbclient::CLIPBOARD_COPY,
                                size: 0,
                            }.to_event();
                            window.event(clipboard_event);
                        }
                    }
                },
                orbclient::K_X => if event.pressed {
                    if let Some(id) = self.scheme.order.front() {
                        if let Some(window) = self.scheme.windows.get_mut(&id) {
                            //TODO: set window's clipboard to primary
                            let clipboard_event = ClipboardEvent {
                                kind: orbclient::CLIPBOARD_CUT,
                                size: 0,
                            }.to_event();
                            window.event(clipboard_event);
                        }
                    }
                },
                orbclient::K_V => if event.pressed {
                    if let Some(id) = self.scheme.order.front() {
                        if let Some(window) = self.scheme.windows.get_mut(&id) {
                            //TODO: set window's clipboard to primary
                            let clipboard_event = ClipboardEvent {
                                kind: orbclient::CLIPBOARD_PASTE,
                                size: 0,
                            }.to_event();
                            window.event(clipboard_event);
                        }
                    }
                },
                _ => if event.pressed {
                    println!("WIN+{:X}", event.scancode);
                }
            }
        } else if let Some(id) = self.scheme.order.front() {
            if let Some(window) = self.scheme.windows.get_mut(&id) {
                window.event(event.to_event());
            }
        }
    }

    fn mouse_event(&mut self, event: MouseEvent) {
        let mut new_cursor = CursorKind::LeftPtr;

        // Check for focus switch, dragging, and forward mouse events to applications
        match self.scheme.dragging {
            DragMode::None => {
                for entry in self.scheme.zbuffer.iter() {
                    let id = entry.0;
                    if let Some(window) = self.scheme.windows.get_mut(&id) {
                        if window.rect().contains(event.x, event.y) {
                            if ! window.mouse_cursor {
                                new_cursor = CursorKind::None;
                            }
                            if ! self.scheme.win_key {
                                let mut window_event = event.to_event();
                                window_event.a -= window.x as i64;
                                window_event.b -= window.y as i64;
                                window.event(window_event);
                            }
                            break;
                        } else if window.title_rect().contains(event.x, event.y) {
                            break;
                        } else if window.left_border_rect().contains(event.x, event.y) {
                            new_cursor = CursorKind::LeftSide;
                            break;
                        } else if window.right_border_rect().contains(event.x, event.y) {
                            new_cursor = CursorKind::RightSide;
                            break;
                        } else if window.bottom_border_rect().contains(event.x, event.y) {
                            new_cursor = CursorKind::BottomSide;
                            break;
                        } else if window.bottom_left_border_rect().contains(event.x, event.y) {
                            new_cursor = CursorKind::BottomLeftCorner;
                            break;
                        } else if window.bottom_right_border_rect().contains(event.x, event.y) {
                            new_cursor = CursorKind::BottomRightCorner;
                            break;
                        }
                    }
                }
            },
            DragMode::Title(window_id, drag_x, drag_y) => {
                if let Some(window) = self.scheme.windows.get_mut(&window_id) {
                    if drag_x != event.x || drag_y != event.y {
                        schedule(&mut self.scheme.redraws, window.title_rect());
                        schedule(&mut self.scheme.redraws, window.rect());

                        //TODO: Min and max
                        window.x += event.x - drag_x;
                        window.y += event.y - drag_y;

                        let move_event = MoveEvent {
                            x: window.x,
                            y: window.y
                        }.to_event();
                        window.event(move_event);

                        self.scheme.dragging = DragMode::Title(window_id, event.x, event.y);

                        schedule(&mut self.scheme.redraws, window.title_rect());
                        schedule(&mut self.scheme.redraws, window.rect());
                    }
                } else {
                    self.scheme.dragging = DragMode::None;
                }
            },
            DragMode::LeftBorder(window_id, off_x, right_x) => {
                if let Some(window) = self.scheme.windows.get_mut(&window_id) {
                    new_cursor = CursorKind::LeftSide;

                    let x = event.x - off_x;
                    let w = right_x - x;

                    if w > 0 {
                        if x != window.x {
                            schedule(&mut self.scheme.redraws, window.title_rect());
                            schedule(&mut self.scheme.redraws, window.rect());

                            window.x = x;
                            let move_event = MoveEvent {
                                x: x,
                                y: window.y
                            }.to_event();
                            window.event(move_event);

                            schedule(&mut self.scheme.redraws, window.title_rect());
                            schedule(&mut self.scheme.redraws, window.rect());
                        }

                        if w != window.width()  {
                            let resize_event = ResizeEvent {
                                width: w as u32,
                                height: window.height() as u32
                            }.to_event();
                            window.event(resize_event);
                        }
                    }
                } else {
                    self.scheme.dragging = DragMode::None;
                }
            },
            DragMode::RightBorder(window_id, off_x) => {
                if let Some(window) = self.scheme.windows.get_mut(&window_id) {
                    new_cursor = CursorKind::RightSide;
                    let w = event.x - off_x - window.x;
                    if w > 0 && w != window.width()  {
                        let resize_event = ResizeEvent {
                            width: w as u32,
                            height: window.height() as u32
                        }.to_event();
                        window.event(resize_event);
                    }
                } else {
                    self.scheme.dragging = DragMode::None;
                }
            },
            DragMode::BottomBorder(window_id, off_y) => {
                if let Some(window) = self.scheme.windows.get_mut(&window_id) {
                    new_cursor = CursorKind::BottomSide;
                    let h = event.y - off_y - window.y;
                    if h > 0 && h != window.height()  {
                        let resize_event = ResizeEvent {
                            width: window.width() as u32,
                            height: h as u32
                        }.to_event();
                        window.event(resize_event);
                    }
                } else {
                    self.scheme.dragging = DragMode::None;
                }
            },
            DragMode::BottomLeftBorder(window_id, off_x, off_y, right_x) => {
                if let Some(window) = self.scheme.windows.get_mut(&window_id) {
                    new_cursor = CursorKind::BottomLeftCorner;

                    let x = event.x - off_x;
                    let h = event.y - off_y - window.y;
                    let w = right_x - x;

                    if w > 0 && h > 0 {
                        if x != window.x {
                            schedule(&mut self.scheme.redraws, window.title_rect());
                            schedule(&mut self.scheme.redraws, window.rect());

                            window.x = x;
                            let move_event = MoveEvent {
                                x: x,
                                y: window.y
                            }.to_event();
                            window.event(move_event);

                            schedule(&mut self.scheme.redraws, window.title_rect());
                            schedule(&mut self.scheme.redraws, window.rect());
                        }

                        if w != window.width() || h != window.height() {
                            let resize_event = ResizeEvent {
                                width: w as u32,
                                height: h as u32
                            }.to_event();
                            window.event(resize_event);
                        }
                    }
                } else {
                    self.scheme.dragging = DragMode::None;
                }
            },
            DragMode::BottomRightBorder(window_id, off_x, off_y) => {
                if let Some(window) = self.scheme.windows.get_mut(&window_id) {
                    new_cursor = CursorKind::BottomRightCorner;
                    let w = event.x - off_x - window.x;
                    let h = event.y - off_y - window.y;
                    if w > 0 && h > 0 && (w != window.width() || h != window.height())  {
                        let resize_event = ResizeEvent {
                            width: w as u32,
                            height: h as u32
                        }.to_event();
                        window.event(resize_event);
                    }
                } else {
                    self.scheme.dragging = DragMode::None;
                }
            }
        }

        if new_cursor != self.scheme.cursor_i {
            let cursor_rect = self.scheme.cursor_rect();
            schedule(&mut self.scheme.redraws, cursor_rect);

            self.scheme.cursor_i = new_cursor;

            let cursor_rect = self.scheme.cursor_rect();
            schedule(&mut self.scheme.redraws, cursor_rect);
        }

        // Update saved mouse information
        if event.x != self.scheme.cursor_x || event.y != self.scheme.cursor_y {
            let cursor_rect = self.scheme.cursor_rect();
            schedule(&mut self.scheme.redraws, cursor_rect);

            self.scheme.cursor_x = event.x;
            self.scheme.cursor_y = event.y;

            let cursor_rect = self.scheme.cursor_rect();
            schedule(&mut self.scheme.redraws, cursor_rect);
        }
    }

    fn mouse_relative_event(&mut self, event: MouseRelativeEvent) {
        if let Some(id) = self.scheme.order.front() {
            if let Some(window) = self.scheme.windows.get_mut(&id) {
                if window.mouse_relative {
                    window.event(event.to_event());
                    return;
                }
            }
        }

        let x = cmp::max(0, cmp::min(self.orb.image.width(), self.scheme.cursor_x + event.dx));
        let y = cmp::max(0, cmp::min(self.orb.image.height(), self.scheme.cursor_y + event.dy));
        self.mouse_event(MouseEvent { x, y });
    }

    fn button_event(&mut self, event: ButtonEvent) {
        // Check for focus switch, dragging, and forward mouse events to applications
        match self.scheme.dragging {
            DragMode::None => {
                let mut focus = 0;
                for entry in self.scheme.zbuffer.iter() {
                    let id = entry.0;
                    let i = entry.2;
                    if let Some(window) = self.scheme.windows.get_mut(&id) {
                        if window.rect().contains(self.scheme.cursor_x, self.scheme.cursor_y) {
                            if self.scheme.win_key {
                                if event.left && ! self.scheme.cursor_left {
                                    focus = i;
                                    if ! window.borderless {
                                        self.scheme.dragging = DragMode::Title(id, self.scheme.cursor_x, self.scheme.cursor_y);
                                    }
                                }
                            } else {
                                window.event(event.to_event());
                                if event.left  && ! self.scheme.cursor_left
                                || event.middle && ! self.scheme.cursor_middle
                                || event.right && ! self.scheme.cursor_right {
                                    focus = i;
                                }
                            }
                            break;
                        } else if window.title_rect().contains(self.scheme.cursor_x, self.scheme.cursor_y) {
                            //TODO: Trigger max and exit on release
                            if event.left && ! self.scheme.cursor_left  {
                                focus = i;
                                if (window.max_contains(self.scheme.cursor_x, self.scheme.cursor_y)) && (window.resizable) {
                                    let max_restore_opt = window.max_restore.take();

                                    if max_restore_opt.is_none() {
                                        window.max_restore = Some(window.rect());
                                    }

                                    schedule(&mut self.scheme.redraws, window.title_rect());
                                    schedule(&mut self.scheme.redraws, window.rect());

                                    if let Some(max_restore) = max_restore_opt {
                                        window.x = max_restore.left();
                                        window.y = max_restore.top();
                                    } else {
                                        window.x = 0;
                                        window.y = window.title_rect().height();
                                    }

                                    let move_event = MoveEvent {
                                        x: window.x,
                                        y: window.y
                                    }.to_event();
                                    window.event(move_event);

                                    schedule(&mut self.scheme.redraws, window.title_rect());
                                    schedule(&mut self.scheme.redraws, window.rect());

                                    let (width, height) = if let Some(max_restore) = max_restore_opt {
                                        (max_restore.width(), max_restore.height())
                                    } else {
                                        (self.orb.image.width(), self.orb.image.height() - window.y)
                                    };
                                    let resize_event = ResizeEvent {
                                        width: width as u32,
                                        height: height as u32,
                                    }.to_event();
                                    window.event(resize_event);
                                } else if (window.close_contains(self.scheme.cursor_x, self.scheme.cursor_y)) && (!window.unclosable) {
                                    window.event(QuitEvent.to_event());
                                } else {
                                    self.scheme.dragging = DragMode::Title(id, self.scheme.cursor_x, self.scheme.cursor_y);
                                }
                            }
                            break;
                        } else if window.left_border_rect().contains(self.scheme.cursor_x, self.scheme.cursor_y) {
                            if event.left && ! self.scheme.cursor_left  {
                                focus = i;
                                self.scheme.dragging = DragMode::LeftBorder(id, self.scheme.cursor_x - window.x, window.x + window.width());
                            }
                            break;
                        } else if window.right_border_rect().contains(self.scheme.cursor_x, self.scheme.cursor_y) {
                            if event.left && ! self.scheme.cursor_left  {
                                focus = i;
                                self.scheme.dragging = DragMode::RightBorder(id, self.scheme.cursor_x - (window.x + window.width()));
                            }
                            break;
                        } else if window.bottom_border_rect().contains(self.scheme.cursor_x, self.scheme.cursor_y) {
                            if event.left && ! self.scheme.cursor_left  {
                                focus = i;
                                self.scheme.dragging = DragMode::BottomBorder(id, self.scheme.cursor_y - (window.y + window.height()));
                            }
                            break;
                        } else if window.bottom_left_border_rect().contains(self.scheme.cursor_x, self.scheme.cursor_y) {
                            if event.left && ! self.scheme.cursor_left  {
                                focus = i;
                                self.scheme.dragging = DragMode::BottomLeftBorder(id, self.scheme.cursor_x - window.x, self.scheme.cursor_y - (window.y + window.height()), window.x + window.width());
                            }
                            break;
                        } else if window.bottom_right_border_rect().contains(self.scheme.cursor_x, self.scheme.cursor_y) {
                            if event.left && ! self.scheme.cursor_left  {
                                focus = i;
                                self.scheme.dragging = DragMode::BottomRightBorder(id, self.scheme.cursor_x - (window.x + window.width()), self.scheme.cursor_y - (window.y + window.height()));
                            }
                            break;
                        }
                    }
                }

                if focus > 0 {
                    // Redraw old focused window
                    if let Some(id) = self.scheme.order.front() {
                        if let Some(window) = self.scheme.windows.get_mut(&id){
                            schedule(&mut self.scheme.redraws, window.title_rect());
                            schedule(&mut self.scheme.redraws, window.rect());
                            window.event(FocusEvent {
                                focused: false
                            }.to_event());
                        }
                    }

                    // Reorder windows
                    if let Some(id) = self.scheme.order.remove(focus) {
                        if let Some(window) = self.scheme.windows.get(&id){
                            match window.zorder {
                                WindowZOrder::Front | WindowZOrder::Normal => {
                                    // Transfer focus if a front or normal window
                                    self.scheme.order.push_front(id);
                                },
                                WindowZOrder::Back => {
                                    // Return to original position if a background window
                                    self.scheme.order.insert(focus, id);
                                }
                            }
                        }
                    }

                    // Redraw new focused window
                    if let Some(id) = self.scheme.order.front() {
                        if let Some(window) = self.scheme.windows.get_mut(&id){
                            schedule(&mut self.scheme.redraws, window.title_rect());
                            schedule(&mut self.scheme.redraws, window.rect());
                            window.event(FocusEvent {
                                focused: true
                            }.to_event());
                        }
                    }
                }
            },
            _ => if ! event.left {
                self.scheme.dragging = DragMode::None;
            }
        }

        self.scheme.cursor_left = event.left;
        self.scheme.cursor_middle = event.middle;
        self.scheme.cursor_right = event.right;
    }

    fn resize_event(&mut self, event: ResizeEvent) {
        self.orb.resize(event.width as i32, event.height as i32);

        let screen_rect = self.orb.screen_rect();
        schedule(&mut self.scheme.redraws, screen_rect);

        let screen_event = ScreenEvent {
            width: self.orb.image.width() as u32,
            height: self.orb.image.height() as u32,
        }.to_event();
        for (_window_id, window) in self.scheme.windows.iter_mut() {
            window.event(screen_event);
        }
    }

    pub fn event(&mut self, event_union: Event){
        self.scheme.rezbuffer();

        match event_union.to_option() {
            EventOption::Key(event) => self.key_event(event),
            EventOption::Mouse(event) => self.mouse_event(event),
            EventOption::MouseRelative(event) => self.mouse_relative_event(event),
            EventOption::Button(event) => self.button_event(event),
            EventOption::Scroll(_) => {
                if let Some(entry) = self.scheme.zbuffer.first() {
                    let id = entry.0;
                    if let Some(window) = self.scheme.windows.get_mut(&id) {
                        window.event(event_union);
                    }
                }
            },
            EventOption::Resize(event) => self.resize_event(event),
            event => println!("orbital: unexpected event: {:?}", event)
        }
    }

    pub fn display_event(&mut self, events: &[Event]) -> io::Result<()> {
        for &event in events {
            self.event(event);
        }

        for (id, window) in self.scheme.windows.iter_mut() {
            if ! window.events.is_empty() {
                if !window.notified_read {
                    window.notified_read = true;
                    self.orb.scheme_write(&Packet {
                        id: 0,
                        pid: 0,
                        uid: 0,
                        gid: 0,
                        a: syscall::number::SYS_FEVENT,
                        b: *id,
                        c: syscall::flag::EVENT_READ,
                        d: window.events.len() * mem::size_of::<Event>()
                    })?;
                }
            } else {
                window.notified_read = false;
            }
        }

        // redrawn by handle_after

        Ok(())
    }

    pub fn scheme_event(&mut self, _packets: &mut [Packet]) -> io::Result<()> {
        for (id, window) in self.scheme.windows.iter_mut() {
            if ! window.events.is_empty() {
                if !window.notified_read {
                    window.notified_read = true;
                    self.orb.scheme_write(&Packet {
                        id: 0,
                        pid: 0,
                        uid: 0,
                        gid: 0,
                        a: syscall::number::SYS_FEVENT,
                        b: *id,
                        c: syscall::flag::EVENT_READ,
                        d: window.events.len() * mem::size_of::<Event>()
                    })?;
                }
            } else {
                window.notified_read = false;
            }
        }

        // redrawn by handle_after

        Ok(())
    }

    fn window_new(&mut self, mut x: i32, mut y: i32,
                  width: i32, height: i32,
                  flags: &str,
                  title: String) -> Result<usize> {
        let id = self.scheme.next_id as usize;
        self.scheme.next_id += 1;
        if self.scheme.next_id < 0 {
            self.scheme.next_id = 1;
        }

        if x < 0 && y < 0 {
            // Automatic placement
            x = cmp::max(0, (self.orb.image.width() - width)/2);
            y = cmp::max(28, (self.orb.image.height() - height)/2);
        }

        if let Some(id) = self.scheme.order.front() {
            if let Some(window) = self.scheme.windows.get(&id) {
                schedule(&mut self.scheme.redraws, window.title_rect());
                schedule(&mut self.scheme.redraws, window.rect());
            }
        }

        let mut window = Window::new(x, y, width, height, self.scheme.scale);

        for flag in flags.chars() {
            match flag {
                'a' => window.async = true,
                'b' => window.zorder = WindowZOrder::Back,
                'f' => window.zorder = WindowZOrder::Front,
                'l' => window.borderless = true,
                'r' => window.resizable = true,
                't' => window.transparent = true,
                'u' => window.unclosable = true,
                _ => ()
            }
        }

        window.title = title;
        window.render_title(&self.scheme.font);

        schedule(&mut self.scheme.redraws, window.title_rect());
        schedule(&mut self.scheme.redraws, window.rect());

        match window.zorder {
            WindowZOrder::Front | WindowZOrder::Normal => {
                self.scheme.order.push_front(id);
            },
            WindowZOrder::Back => {
                self.scheme.order.push_back(id);
            }
        }

        self.scheme.windows.insert(id, window);

        Ok(id)
    }
}
