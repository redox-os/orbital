use orbclient::{self, Color, Event, EventOption, KeyEvent, MouseEvent, ButtonEvent, FocusEvent, QuitEvent, MoveEvent, ResizeEvent, Renderer};
use orbfont;
use resize;

use std::collections::{BTreeMap, VecDeque};
use std::path::Path;
use std::{slice, str};
use syscall::data::Packet;
use syscall::error::{Error, Result, EBADF, EINVAL};
use syscall::scheme::SchemeMut;

use config::Config;
use image::{Image, ImageRef};
use rect::Rect;
use socket::Socket;
use theme::{BACKGROUND_COLOR, BAR_COLOR, BAR_HIGHLIGHT_COLOR, TEXT_COLOR, TEXT_HIGHLIGHT_COLOR};
use window::Window;

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

#[derive(Clone, Copy)]
enum BackgroundMode {
    /// Do not resize the image, just center it
    Center,
    /// Resize the image to the display size
    Fill,
    /// Resize the image - keeping its aspect ratio, and fit it to the display with blank space
    Scale,
    /// Resize the image - keeping its aspect ratio, and crop to remove all blank space
    Zoom,
}

impl BackgroundMode {
    fn from_str(string: &str) -> BackgroundMode {
        match string {
            "fill" => BackgroundMode::Fill,
            "scale" => BackgroundMode::Scale,
            "zoom" => BackgroundMode::Zoom,
            _ => BackgroundMode::Center
        }
    }
}

fn resize_image(image: Image, mode: BackgroundMode, display_width: i32, display_height: i32) -> Image {
    let (width, height) = match mode {
        BackgroundMode::Center => {
            return image;
        },
        BackgroundMode::Fill => {
            (display_width, display_height)
        },
        BackgroundMode::Scale => {
            let d_w = display_width as f64;
            let d_h = display_height as f64;
            let i_w = image.width() as f64;
            let i_h = image.height() as f64;

            let scale = if d_w / d_h > i_w / i_h {
                d_h / i_h
            } else {
                d_w / i_w
            };

            ((i_w * scale) as i32, (i_h * scale) as i32)
        },
        BackgroundMode::Zoom => {
            let d_w = display_width as f64;
            let d_h = display_height as f64;
            let i_w = image.width() as f64;
            let i_h = image.height() as f64;

            let scale = if d_w / d_h < i_w / i_h {
                d_h / i_h
            } else {
                d_w / i_w
            };

            ((i_w * scale) as i32, (i_h * scale) as i32)
        }
    };

    if width == image.width() && height == image.height() {
        return image;
    }

    let src_color = image.data();
    let mut dst_color = vec![Color::rgb(0, 0, 0); width as usize * height as usize].into_boxed_slice();

    let src = unsafe {
        slice::from_raw_parts(src_color.as_ptr() as *const u8, src_color.len() * 4)
    };
    let mut dst = unsafe {
        slice::from_raw_parts_mut(dst_color.as_mut_ptr() as *mut u8, dst_color.len() * 4)
    };

    let mut resizer = resize::new(image.width() as usize, image.height() as usize,
                                  width as usize, height as usize,
                                  resize::Pixel::RGBA, resize::Type::Lanczos3);
    resizer.resize(&src, &mut dst);

    Image::from_data(width, height, dst_color)
}

fn load_backgrounds(configs: &Vec<String>, mode: BackgroundMode, display_width: i32, display_height: i32) -> Vec<Image> {
    let mut paths = Vec::new();

    for config in configs.iter() {
        let path = Path::new(&config);
        if path.is_dir() {
            if let Ok(read_dir) = path.read_dir() {
                for entry_res in read_dir {
                    if let Ok(entry) = entry_res {
                        paths.push(entry.path());
                    }
                }
            }
        } else {
            paths.push(path.to_path_buf());
        }
    }

    paths.sort();

    let mut backgrounds = Vec::new();
    for path in paths.iter() {
        println!("orbital: loading {}", path.display());
        if let Some(image) = Image::from_path(path) {
            println!("orbital: resizing {}", path.display());
            backgrounds.push(resize_image(image, mode, display_width, display_height));
        }
    }

    backgrounds
}

#[derive(Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
enum CursorKind {
    LeftPtr,
    BottomRightCorner,
    BottomSide,
    RightSide,
}

enum DragMode {
    None,
    Title(usize, i32, i32),
    RightBorder(usize, i32),
    BottomBorder(usize, i32),
    BottomRightBorder(usize, i32, i32),
}

pub struct OrbitalScheme {
    image: ImageRef<'static>,
    backgrounds: Vec<Image>,
    background_i: usize,
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
    next_x: i32,
    next_y: i32,
    order: VecDeque<usize>,
    pub windows: BTreeMap<usize, Window>,
    redraws: Vec<Rect>,
    pub todo: Vec<Packet>,
    font: orbfont::Font
}

impl OrbitalScheme {
    pub fn new(width: i32, height: i32, data: &'static mut [Color], config: &Config) -> OrbitalScheme {
        let mut cursors = BTreeMap::new();
        cursors.insert(CursorKind::LeftPtr, Image::from_path(&config.cursor).unwrap_or(Image::new(0, 0)));
        cursors.insert(CursorKind::BottomRightCorner, Image::from_path(&config.bottom_right_corner).unwrap_or(Image::new(0, 0)));
        cursors.insert(CursorKind::BottomSide, Image::from_path(&config.bottom_side).unwrap_or(Image::new(0, 0)));
        cursors.insert(CursorKind::RightSide, Image::from_path(&config.right_side).unwrap_or(Image::new(0, 0)));

        OrbitalScheme {
            image: ImageRef::from_data(width, height, data),
            backgrounds: load_backgrounds(&config.background,
                                     BackgroundMode::from_str(&config.background_mode),
                                     width, height),
            background_i: 0,
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
            next_x: 4,
            next_y: 32,
            order: VecDeque::new(),
            windows: BTreeMap::new(),
            redraws: vec![Rect::new(0, 0, width, height)],
            todo: Vec::new(),
            font: orbfont::Font::find(Some("Sans"), None, None).unwrap()
        }
    }

    fn background_rect(&self) -> Rect {
        if let Some(background) = self.backgrounds.get(self.background_i) {
            let w = background.width();
            let h = background.height();
            let x = self.image.width()/2 - w/2;
            let y = self.image.height()/2 - h/2;
            Rect::new(x, y, w, h)
        } else {
            Rect::new(-1, -1, 0, 0)
        }
    }

    fn cursor_rect(&self) -> Rect {
        let cursor = &self.cursors[&self.cursor_i];
        let (off_x, off_y) = match self.cursor_i {
            CursorKind::LeftPtr => (0, 0),
            CursorKind::BottomRightCorner => (-cursor.width(), -cursor.height()),
            CursorKind::BottomSide => (-cursor.width()/2, -cursor.height()),
            CursorKind::RightSide => (-cursor.width(), -cursor.height()/2),
        };
        Rect::new(self.cursor_x + off_x, self.cursor_y + off_y, cursor.width(), cursor.height())
    }

    fn screen_rect(&self) -> Rect {
        Rect::new(0, 0, self.image.width(), self.image.height())
    }

    pub fn redraw(&mut self, display: &Socket){
        let screen_rect = self.screen_rect();
        let background_rect = self.background_rect();
        let cursor_rect = self.cursor_rect();

        for mut rect in self.redraws.drain(..) {
            rect = rect.intersection(&screen_rect);

            if ! rect.is_empty() {
                //TODO: only clear area not covered by background
                self.image.rect(rect.left(), rect.top(),
                                rect.width() as u32, rect.height() as u32,
                                BACKGROUND_COLOR);

                let background_intersect = rect.intersection(&background_rect);
                if ! background_intersect.is_empty(){
                    if let Some(mut background) = self.backgrounds.get_mut(self.background_i) {
                        self.image.roi(&background_intersect).blit(&background.roi(&background_intersect.offset(-background_rect.left(), -background_rect.top())));
                    }
                }

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

        display.sync().unwrap();
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
                orbclient::K_BKSP => if event.pressed {
                    // Switch backgrounds
                    let bg_rect = self.background_rect();
                    schedule(&mut self.redraws, bg_rect);

                    self.background_i += 1;
                    if self.background_i >= self.backgrounds.len() {
                        self.background_i = 0;
                    }

                    let bg_rect = self.background_rect();
                    schedule(&mut self.redraws, bg_rect);
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
                        } else if window.right_border_rect().contains(event.x, event.y) {
                            new_cursor = CursorKind::RightSide;
                            break;
                        } else if window.bottom_border_rect().contains(event.x, event.y) {
                            new_cursor = CursorKind::BottomSide;
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
                                if window.max_contains(self.cursor_x, self.cursor_y) {
                                    schedule(&mut self.redraws, window.title_rect());
                                    schedule(&mut self.redraws, window.rect());

                                    window.x = 0;
                                    window.y = window.title_rect().height();

                                    let move_event = MoveEvent {
                                        x: window.x,
                                        y: window.y
                                    }.to_event();
                                    window.event(move_event);

                                    schedule(&mut self.redraws, window.title_rect());
                                    schedule(&mut self.redraws, window.rect());

                                    let resize_event = ResizeEvent {
                                        width: self.image.width() as u32,
                                        height: (self.image.height() - window.y) as u32,
                                    }.to_event();
                                    window.event(resize_event);
                                } else if window.close_contains(self.cursor_x, self.cursor_y) {
                                    window.event(QuitEvent.to_event());
                                } else {
                                    self.dragging = DragMode::Title(id, self.cursor_x, self.cursor_y);
                                }
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
            },
            _ => if ! event.left {
                self.dragging = DragMode::None;
            }
        }

        self.cursor_left = event.left;
        self.cursor_middle = event.middle;
        self.cursor_right = event.right;
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
            event => println!("orbital: unexpected event: {:?}", event)
        }
    }
}

impl SchemeMut for OrbitalScheme {
    fn open(&mut self, url: &[u8], _flags: usize, _uid: u32, _gid: u32) -> Result<usize> {
        let path = try!(str::from_utf8(url).or(Err(Error::new(EINVAL))));
        let mut parts = path.split("/");

        let flags = parts.next().unwrap_or("");

        let mut async = false;
        let mut resizable = false;
        for flag in flags.chars() {
            match flag {
                'a' => async = true,
                'r' => resizable = true,
                _ => ()
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

        let window = Window::new(x, y, width, height, title, async, resizable, &self.font);
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
