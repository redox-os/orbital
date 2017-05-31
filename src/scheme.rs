use orbclient::{self, Color, Event, EventOption, KeyEvent, MouseEvent, ButtonEvent, FocusEvent, QuitEvent, MoveEvent, ResizeEvent, ScreenEvent, Renderer};
use orbfont;
use syscall;

use std::{cmp, io, mem, slice, str};
use std::collections::{BTreeMap, VecDeque};
use std::fs::File;
use std::io::{Read, Write};
use std::os::unix::io::AsRawFd;
use syscall::data::Packet;
use syscall::error::{Error, Result, EBADF, EINVAL};
use syscall::number::SYS_READ;
use syscall::scheme::SchemeMut;

use config::Config;
use image::{Image, ImageRef};
use rect::Rect;
use theme::{BACKGROUND_COLOR, BAR_COLOR, BAR_HIGHLIGHT_COLOR, TEXT_COLOR, TEXT_HIGHLIGHT_COLOR};
use window::{Window, WindowZOrder};

pub fn read_type<R: Read, T: Copy>(r: &mut R, buf: &mut [T]) -> io::Result<usize> {
    r.read(unsafe { slice::from_raw_parts_mut(
        buf.as_mut_ptr() as *mut u8,
        buf.len() * mem::size_of::<T>())
    }).map(|count| count/mem::size_of::<T>())
}

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

unsafe fn display_fd_map(width: i32, height: i32, display_fd: usize) -> ImageRef<'static> {
    let display_ptr = syscall::fmap(display_fd, 0, (width * height * 4) as usize).unwrap();
    let display_slice = slice::from_raw_parts_mut(display_ptr as *mut Color, (width * height) as usize);
    ImageRef::from_data(width, height, display_slice)
}

unsafe fn display_fd_unmap(image: &mut ImageRef) {
    let _ = syscall::funmap(image.data().as_ptr() as usize);
}

#[derive(Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
enum CursorKind {
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
    socket: File,
    display: File,
    image: ImageRef<'static>,
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
    pub windows: BTreeMap<usize, Window>,
    redraws: Vec<Rect>,
    pub todo: Vec<Packet>,
    font: orbfont::Font
}

impl OrbitalScheme {
    pub fn new(width: i32, height: i32, socket: File, display: File, config: &Config) -> OrbitalScheme {
        let display_fd = display.as_raw_fd();

        let mut cursors = BTreeMap::new();
        cursors.insert(CursorKind::LeftPtr, Image::from_path(&config.cursor).unwrap_or(Image::new(0, 0)));
        cursors.insert(CursorKind::BottomLeftCorner, Image::from_path(&config.bottom_left_corner).unwrap_or(Image::new(0, 0)));
        cursors.insert(CursorKind::BottomRightCorner, Image::from_path(&config.bottom_right_corner).unwrap_or(Image::new(0, 0)));
        cursors.insert(CursorKind::BottomSide, Image::from_path(&config.bottom_side).unwrap_or(Image::new(0, 0)));
        cursors.insert(CursorKind::LeftSide, Image::from_path(&config.left_side).unwrap_or(Image::new(0, 0)));
        cursors.insert(CursorKind::RightSide, Image::from_path(&config.right_side).unwrap_or(Image::new(0, 0)));

        OrbitalScheme {
            socket: socket,
            display: display,
            image: unsafe { display_fd_map(width, height, display_fd) },
            window_max: Image::from_path(&config.window_max).unwrap_or(Image::new(0, 0)),
            window_max_unfocused: Image::from_path(&config.window_max_unfocused).unwrap_or(Image::new(0, 0)),
            window_close: Image::from_path(&config.window_close).unwrap_or(Image::new(0, 0)),
            window_close_unfocused: Image::from_path(&config.window_close_unfocused).unwrap_or(Image::new(0, 0)),
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
            windows: BTreeMap::new(),
            redraws: vec![Rect::new(0, 0, width, height)],
            todo: Vec::new(),
            font: orbfont::Font::find(Some("Sans"), None, None).unwrap()
        }
    }

    fn cursor_rect(&self) -> Rect {
        let cursor = &self.cursors[&self.cursor_i];
        let (off_x, off_y) = match self.cursor_i {
            CursorKind::LeftPtr => (0, 0),
            CursorKind::BottomLeftCorner => (0, -cursor.height()),
            CursorKind::BottomRightCorner => (-cursor.width(), -cursor.height()),
            CursorKind::BottomSide => (-cursor.width()/2, -cursor.height()),
            CursorKind::LeftSide => (0, -cursor.height()/2),
            CursorKind::RightSide => (-cursor.width(), -cursor.height()/2),
        };
        Rect::new(self.cursor_x + off_x, self.cursor_y + off_y, cursor.width(), cursor.height())
    }

    fn screen_rect(&self) -> Rect {
        Rect::new(0, 0, self.image.width(), self.image.height())
    }

    pub fn redraw(&mut self){
        let screen_rect = self.screen_rect();
        let cursor_rect = self.cursor_rect();

        for mut rect in self.redraws.drain(..) {
            rect = rect.intersection(&screen_rect);

            if ! rect.is_empty() {
                self.image.rect(rect.left(), rect.top(),
                                rect.width() as u32, rect.height() as u32,
                                BACKGROUND_COLOR);

                for (i, id) in self.order.iter().enumerate().rev() {
                    if let Some(mut window) = self.windows.get_mut(&id) {
                        window.draw_title(&mut self.image, &rect, i == 0, if i == 0 {
                            &mut self.window_max
                        } else {
                            &mut self.window_max_unfocused
                        }, if i == 0 {
                            &mut self.window_close
                        } else {
                            &mut self.window_close_unfocused
                        });
                        window.draw(&mut self.image, &rect);
                    }
                }

                let cursor_intersect = rect.intersection(&cursor_rect);
                if ! cursor_intersect.is_empty() {
                    if let Some(cursor) = self.cursors.get_mut(&self.cursor_i) {
                        self.image.roi(&cursor_intersect).blend(&cursor.roi(&cursor_intersect.offset(-cursor_rect.left(), -cursor_rect.top())));
                    }
                }
            }
        }

        if self.win_tabbing {
            self.draw_window_list();
        }

        self.display.sync_all().unwrap();
    }

    fn win_tab(&mut self) {
        if self.order.len() > 1 {
            // Disable dragging
            self.dragging = DragMode::None;

            //Redraw old focused window
            if let Some(id) = self.order.pop_front() {
                if let Some(mut window) = self.windows.get_mut(&id) {
                    schedule(&mut self.redraws, window.title_rect());
                    schedule(&mut self.redraws, window.rect());
                    window.event(FocusEvent {
                        focused: false
                    }.to_event());
                }
                self.order.push_back(id);
            }
            //Redraw new focused window
            if let Some(id) = self.order.front() {
                if let Some(mut window) = self.windows.get_mut(&id){
                    schedule(&mut self.redraws, window.title_rect());
                    schedule(&mut self.redraws, window.rect());
                    window.event(FocusEvent {
                        focused: true
                    }.to_event());
                }
            }
        }
    }

    /// Draws a list of currently open windows in the middle of the screen
    fn draw_window_list(&mut self) {
        use orbfont;
        let mut rendered_text: Vec<orbfont::Text> = vec![];
        for id in self.order.iter() {
            if let Some(window) = self.windows.get(id) {
                if window.title.is_empty() {
                    rendered_text.push(self.font.render(&format!("[unnamed #{}]", id), 16.0));
                } else {
                    rendered_text.push(self.font.render(&format!("{}", &window.title), 16.0));
                }
            }
        }

        let list_h = rendered_text.len() as i32 * 20 + 4;
        let list_w = 400;
        let target_rect = Rect::new(self.image.width()/2 - list_w/2,
                                    self.image.height()/2 - list_h/2,
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
        self.image.roi(&target_rect).blit(&image.roi(&Rect::new(0, 0, list_w, list_h)));
        schedule(&mut self.redraws, target_rect);
    }

    fn key_event(&mut self, event: KeyEvent) {
        if event.scancode == 0x38 {
            self.win_key = event.pressed;
            // If the win key was released, stop drawing the win-tab window switcher
            if !self.win_key {
                self.win_tabbing = false;
            }
        } else if self.win_key {
            match event.scancode {
                orbclient::K_ESC => if event.pressed {
                    if let Some(id) = self.order.front() {
                        if let Some(mut window) = self.windows.get_mut(&id) {
                            window.event(QuitEvent.to_event());
                        }
                    }
                },
                orbclient::K_TAB => if event.pressed {
                    // Start drawing the window switcher. It's drawn by redraw()
                    self.win_tabbing = true;
                    self.win_tab();
                },
                orbclient::K_UP | orbclient::K_DOWN | orbclient::K_LEFT | orbclient::K_RIGHT => if event.pressed {
                    if let Some(id) = self.order.front() {
                        if let Some(mut window) = self.windows.get_mut(&id) {
                            schedule(&mut self.redraws, window.title_rect());
                            schedule(&mut self.redraws, window.rect());

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

                            schedule(&mut self.redraws, window.title_rect());
                            schedule(&mut self.redraws, window.rect());
                        }
                    }
                },
                _ => if event.pressed {
                    println!("WIN+{:X}", event.scancode);
                }
            }
        } else if let Some(id) = self.order.front() {
            if let Some(mut window) = self.windows.get_mut(&id) {
                window.event(event.to_event());
            }
        }
    }

    fn mouse_event(&mut self, event: MouseEvent) {
        let mut new_cursor = CursorKind::LeftPtr;

        // Check for focus switch, dragging, and forward mouse events to applications
        match self.dragging {
            DragMode::None => {
                for &id in self.order.iter() {
                    if let Some(mut window) = self.windows.get_mut(&id) {
                        if window.rect().contains(event.x, event.y) {
                            if ! self.win_key {
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
                if let Some(mut window) = self.windows.get_mut(&window_id) {
                    if drag_x != event.x || drag_y != event.y {
                        schedule(&mut self.redraws, window.title_rect());
                        schedule(&mut self.redraws, window.rect());

                        //TODO: Min and max
                        window.x += event.x - drag_x;
                        window.y += event.y - drag_y;

                        let move_event = MoveEvent {
                            x: window.x,
                            y: window.y
                        }.to_event();
                        window.event(move_event);

                        self.dragging = DragMode::Title(window_id, event.x, event.y);

                        schedule(&mut self.redraws, window.title_rect());
                        schedule(&mut self.redraws, window.rect());
                    }
                } else {
                    self.dragging = DragMode::None;
                }
            },
            DragMode::LeftBorder(window_id, off_x, right_x) => {
                if let Some(mut window) = self.windows.get_mut(&window_id) {
                    new_cursor = CursorKind::LeftSide;

                    let x = event.x - off_x;
                    let w = right_x - x;

                    if w > 0 {
                        if x != window.x {
                            schedule(&mut self.redraws, window.title_rect());
                            schedule(&mut self.redraws, window.rect());

                            window.x = x;
                            let move_event = MoveEvent {
                                x: x,
                                y: window.y
                            }.to_event();
                            window.event(move_event);

                            schedule(&mut self.redraws, window.title_rect());
                            schedule(&mut self.redraws, window.rect());
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
                    self.dragging = DragMode::None;
                }
            },
            DragMode::RightBorder(window_id, off_x) => {
                if let Some(mut window) = self.windows.get_mut(&window_id) {
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
                    self.dragging = DragMode::None;
                }
            },
            DragMode::BottomBorder(window_id, off_y) => {
                if let Some(mut window) = self.windows.get_mut(&window_id) {
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
                    self.dragging = DragMode::None;
                }
            },
            DragMode::BottomLeftBorder(window_id, off_x, off_y, right_x) => {
                if let Some(mut window) = self.windows.get_mut(&window_id) {
                    new_cursor = CursorKind::BottomLeftCorner;

                    let x = event.x - off_x;
                    let h = event.y - off_y - window.y;
                    let w = right_x - x;

                    if w > 0 && h > 0 {
                        if x != window.x {
                            schedule(&mut self.redraws, window.title_rect());
                            schedule(&mut self.redraws, window.rect());

                            window.x = x;
                            let move_event = MoveEvent {
                                x: x,
                                y: window.y
                            }.to_event();
                            window.event(move_event);

                            schedule(&mut self.redraws, window.title_rect());
                            schedule(&mut self.redraws, window.rect());
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
                    self.dragging = DragMode::None;
                }
            },
            DragMode::BottomRightBorder(window_id, off_x, off_y) => {
                if let Some(mut window) = self.windows.get_mut(&window_id) {
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
                    self.dragging = DragMode::None;
                }
            }
        }

        if new_cursor != self.cursor_i {
            let cursor_rect = self.cursor_rect();
            schedule(&mut self.redraws, cursor_rect);

            self.cursor_i = new_cursor;

            let cursor_rect = self.cursor_rect();
            schedule(&mut self.redraws, cursor_rect);
        }

        // Update saved mouse information
        if event.x != self.cursor_x || event.y != self.cursor_y {
            let cursor_rect = self.cursor_rect();
            schedule(&mut self.redraws, cursor_rect);

            self.cursor_x = event.x;
            self.cursor_y = event.y;

            let cursor_rect = self.cursor_rect();
            schedule(&mut self.redraws, cursor_rect);
        }
    }

    fn button_event(&mut self, event: ButtonEvent) {
        // Check for focus switch, dragging, and forward mouse events to applications
        match self.dragging {
            DragMode::None => {
                let mut focus = 0;
                let mut i = 0;
                for &id in self.order.iter() {
                    if let Some(mut window) = self.windows.get_mut(&id) {
                        if window.rect().contains(self.cursor_x, self.cursor_y) {
                            if self.win_key {
                                if event.left && ! self.cursor_left {
                                    focus = i;
                                    self.dragging = DragMode::Title(id, self.cursor_x, self.cursor_y);
                                }
                            } else {
                                window.event(event.to_event());
                                if event.left  && ! self.cursor_left
                                || event.middle && ! self.cursor_middle
                                || event.right && ! self.cursor_right {
                                    focus = i;
                                }
                            }
                            break;
                        } else if window.title_rect().contains(self.cursor_x, self.cursor_y) {
                            //TODO: Trigger max and exit on release
                            if event.left && ! self.cursor_left  {
                                focus = i;
                                if (window.max_contains(self.cursor_x, self.cursor_y)) && (window.resizable) {
                                    let max_restore_opt = window.max_restore.take();

                                    if max_restore_opt.is_none() {
                                        window.max_restore = Some(window.rect());
                                    }

                                    schedule(&mut self.redraws, window.title_rect());
                                    schedule(&mut self.redraws, window.rect());

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

                                    schedule(&mut self.redraws, window.title_rect());
                                    schedule(&mut self.redraws, window.rect());

                                    let (width, height) = if let Some(max_restore) = max_restore_opt {
                                        (max_restore.width(), max_restore.height())
                                    } else {
                                        (self.image.width(), self.image.height() - window.y)
                                    };
                                    let resize_event = ResizeEvent {
                                        width: width as u32,
                                        height: height as u32,
                                    }.to_event();
                                    window.event(resize_event);
                                } else if (window.close_contains(self.cursor_x, self.cursor_y)) && (!window.unclosable) {
                                    window.event(QuitEvent.to_event());
                                } else {
                                    self.dragging = DragMode::Title(id, self.cursor_x, self.cursor_y);
                                }
                            }
                            break;
                        } else if window.left_border_rect().contains(self.cursor_x, self.cursor_y) {
                            if event.left && ! self.cursor_left  {
                                focus = i;
                                self.dragging = DragMode::LeftBorder(id, self.cursor_x - window.x, window.x + window.width());
                            }
                            break;
                        } else if window.right_border_rect().contains(self.cursor_x, self.cursor_y) {
                            if event.left && ! self.cursor_left  {
                                focus = i;
                                self.dragging = DragMode::RightBorder(id, self.cursor_x - (window.x + window.width()));
                            }
                            break;
                        } else if window.bottom_border_rect().contains(self.cursor_x, self.cursor_y) {
                            if event.left && ! self.cursor_left  {
                                focus = i;
                                self.dragging = DragMode::BottomBorder(id, self.cursor_y - (window.y + window.height()));
                            }
                            break;
                        } else if window.bottom_left_border_rect().contains(self.cursor_x, self.cursor_y) {
                            if event.left && ! self.cursor_left  {
                                focus = i;
                                self.dragging = DragMode::BottomLeftBorder(id, self.cursor_x - window.x, self.cursor_y - (window.y + window.height()), window.x + window.width());
                            }
                            break;
                        } else if window.bottom_right_border_rect().contains(self.cursor_x, self.cursor_y) {
                            if event.left && ! self.cursor_left  {
                                focus = i;
                                self.dragging = DragMode::BottomRightBorder(id, self.cursor_x - (window.x + window.width()), self.cursor_y - (window.y + window.height()));
                            }
                            break;
                        }
                    }
                    i += 1;
                }

                if focus > 0 {
                    // Redraw old focused window
                    if let Some(id) = self.order.front() {
                        if let Some(mut window) = self.windows.get_mut(&id){
                            schedule(&mut self.redraws, window.title_rect());
                            schedule(&mut self.redraws, window.rect());
                            window.event(FocusEvent {
                                focused: false
                            }.to_event());
                        }
                    }

                    // Reorder windows
                    if let Some(id) = self.order.remove(focus) {
                        if let Some(window) = self.windows.get(&id){
                            match window.zorder {
                                WindowZOrder::Front | WindowZOrder::Normal => {
                                    // Transfer focus if a front or normal window
                                    self.order.push_front(id);
                                },
                                WindowZOrder::Back => {
                                    // Return to original position if a background window
                                    self.order.insert(focus, id);
                                }
                            }
                        }
                    }

                    // Redraw new focused window
                    if let Some(id) = self.order.front() {
                        if let Some(mut window) = self.windows.get_mut(&id){
                            schedule(&mut self.redraws, window.title_rect());
                            schedule(&mut self.redraws, window.rect());
                            window.event(FocusEvent {
                                focused: true
                            }.to_event());
                        }
                    }
                }
            },
            _ => if ! event.left {
                self.dragging = DragMode::None;
            }
        }

        self.cursor_left = event.left;
        self.cursor_middle = event.middle;
        self.cursor_right = event.right;
    }

    fn resize_event(&mut self, event: ResizeEvent) {
        unsafe {
            display_fd_unmap(&mut self.image);
            self.image = display_fd_map(event.width as i32, event.height as i32, self.display.as_raw_fd());
        }

        let screen_rect = self.screen_rect();
        schedule(&mut self.redraws, screen_rect);

        let screen_event = ScreenEvent {
            width: self.image.width() as u32,
            height: self.image.height() as u32,
        }.to_event();
        for (_window_id, window) in self.windows.iter_mut() {
            window.event(screen_event);
        }
    }

    pub fn event(&mut self, event_union: Event){
        match event_union.to_option() {
            EventOption::Key(event) => self.key_event(event),
            EventOption::Mouse(event) => self.mouse_event(event),
            EventOption::Button(event) => self.button_event(event),
            EventOption::Scroll(_) => {
                if let Some(id) = self.order.front() {
                    if let Some(mut window) = self.windows.get_mut(&id) {
                        window.event(event_union);
                    }
                }
            },
            EventOption::Resize(event) => self.resize_event(event),
            event => println!("orbital: unexpected event: {:?}", event)
        }
    }

    pub fn display_event(&mut self) -> io::Result<()> {
        loop {
            let mut events = [Event::new(); 16];

            let count = read_type(&mut self.display, &mut events)?;
            if count == 0 {
                break;
            }

            for &event in events[.. count].iter() {
                self.event(event);
            }

            let mut i = 0;
            while i < self.todo.len() {
                let mut packet = self.todo[i].clone();

                let delay = if packet.a == SYS_READ {
                    if let Some(window) = self.windows.get(&packet.b) {
                        window.async == false
                    } else {
                        true
                    }
                } else {
                    false
                };

                self.handle(&mut packet);

                if delay && packet.a == 0 {
                    i += 1;
                }else{
                    self.todo.remove(i);
                    self.socket.write(&packet)?;
                }
            }

            for (id, window) in self.windows.iter() {
                if ! window.events.is_empty() {
                    self.socket.write(&Packet {
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
            }
        }

        self.redraw();

        Ok(())
    }

    pub fn scheme_event(&mut self) -> io::Result<()> {
        loop {
            let mut packets = [Packet::default(); 16];

            let count = read_type(&mut self.socket, &mut packets)?;
            if count == 0 {
                break;
            }

            for mut packet in packets[.. count].iter_mut() {
                let delay = if packet.a == SYS_READ {
                    if let Some(window) = self.windows.get(&packet.b) {
                        window.async == false
                    } else {
                        true
                    }
                } else {
                    false
                };

                self.handle(packet);

                if delay && packet.a == 0 {
                    self.todo.push(*packet);
                } else {
                    self.socket.write(&packet)?;
                }
            }

            for (id, window) in self.windows.iter() {
                if ! window.events.is_empty() {
                    self.socket.write(&Packet {
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
            }
        }

        self.redraw();

        Ok(())
    }
}

impl SchemeMut for OrbitalScheme {
    fn open(&mut self, url: &[u8], _flags: usize, _uid: u32, _gid: u32) -> Result<usize> {
        let path = try!(str::from_utf8(url).or(Err(Error::new(EINVAL))));
        let mut parts = path.split("/");

        let flags = parts.next().unwrap_or("");

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
            // Automatic placement
            x = cmp::max(0, (self.image.width() - width)/2);
            y = cmp::max(28, (self.image.height() - height)/2);
        }

        if let Some(id) = self.order.front() {
            if let Some(window) = self.windows.get(&id){
                schedule(&mut self.redraws, window.title_rect());
                schedule(&mut self.redraws, window.rect());
            }
        }

        let mut window = Window::new(x, y, width, height);

        for flag in flags.chars() {
            match flag {
                'a' => window.async = true,
                'b' => window.zorder = WindowZOrder::Back,
                'f' => window.zorder = WindowZOrder::Front,
                'r' => window.resizable = true,
                'u' => window.unclosable = true,
                _ => ()
            }
        }

        window.title = title;
        window.render_title(&self.font);

        schedule(&mut self.redraws, window.title_rect());
        schedule(&mut self.redraws, window.rect());

        match window.zorder {
            WindowZOrder::Front | WindowZOrder::Normal => {
                self.order.push_front(id);
            },
            WindowZOrder::Back => {
                self.order.push_back(id);
            }
        }

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
            if let Ok(msg) = str::from_utf8(buf) {
                let mut parts = msg.split(',');
                match parts.next() {
                    Some("P") => {
                        schedule(&mut self.redraws, window.title_rect());
                        schedule(&mut self.redraws, window.rect());

                        let x = parts.next().unwrap_or("").parse::<i32>().unwrap_or(window.x);
                        let y = parts.next().unwrap_or("").parse::<i32>().unwrap_or(window.y);

                        window.x = x;
                        window.y = y;

                        schedule(&mut self.redraws, window.title_rect());
                        schedule(&mut self.redraws, window.rect());

                        Ok(buf.len())
                    },
                    Some("S") => {
                        schedule(&mut self.redraws, window.title_rect());
                        schedule(&mut self.redraws, window.rect());

                        let w = parts.next().unwrap_or("").parse::<i32>().unwrap_or(window.width());
                        let h = parts.next().unwrap_or("").parse::<i32>().unwrap_or(window.height());

                        window.set_size(w, h);

                        schedule(&mut self.redraws, window.title_rect());
                        schedule(&mut self.redraws, window.rect());

                        Ok(buf.len())
                    },
                    Some("T") => {
                        window.title = parts.next().unwrap_or("").to_string();
                        window.render_title(&self.font);

                        schedule(&mut self.redraws, window.title_rect());

                        Ok(buf.len())
                    },
                    _ => Err(Error::new(EINVAL))
                }
            } else {
                Err(Error::new(EINVAL))
            }
        } else {
            Err(Error::new(EBADF))
        }
    }

    fn fevent(&mut self, id: usize, _flags: usize) -> Result<usize> {
        if self.windows.contains_key(&id) {
            Ok(id)
        } else {
            Err(Error::new(EBADF))
        }
    }

    fn fmap(&mut self, id: usize, offset: usize, size: usize) -> Result<usize> {
        if let Some(mut window) = self.windows.get_mut(&id) {
            window.map(offset, size)
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

    fn fsync(&mut self, id: usize) -> Result<usize> {
        if let Some(window) = self.windows.get(&id) {
            schedule(&mut self.redraws, window.rect());
            Ok(0)
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

impl Drop for OrbitalScheme {
    fn drop(&mut self){
        unsafe { display_fd_unmap(&mut self.image); }
    }
}
