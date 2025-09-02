use crate::{
    core::{
        display::Display,
        image::{Image, ImageAligned},
        rect::Rect,
        Properties,
    },
    scheme::TilePosition,
};
use orbclient::{Color, Event, Renderer};
use orbfont::Font;

use std::cmp::{max, min};
use std::collections::VecDeque;

use std::rc::Rc;

// use theme::{BAR_COLOR, BAR_HIGHLIGHT_COLOR, TEXT_COLOR, TEXT_HIGHLIGHT_COLOR};
use crate::config::Config;

//TODO: move to orbclient?
pub const ORBITAL_FLAG_ASYNC: char = 'a';
pub const ORBITAL_FLAG_BACK: char = 'b';
pub const ORBITAL_FLAG_FRONT: char = 'f';
pub const ORBITAL_FLAG_HIDDEN: char = 'h';
pub const ORBITAL_FLAG_BORDERLESS: char = 'l';
pub const ORBITAL_FLAG_MAXIMIZED: char = 'm';
pub const ORBITAL_FLAG_FULLSCREEN: char = 'M';
pub const ORBITAL_FLAG_RESIZABLE: char = 'r';
pub const ORBITAL_FLAG_TRANSPARENT: char = 't';
pub const ORBITAL_FLAG_UNCLOSABLE: char = 'u';

#[derive(Copy, Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum WindowZOrder {
    Back,
    Normal,
    Front,
}

pub struct Window {
    pub x: i32,
    pub y: i32,
    pub scale: i32,
    pub title: String,
    pub asynchronous: bool,
    pub borderless: bool,
    pub hidden: bool,
    pub resizable: bool,
    pub transparent: bool,
    pub unclosable: bool,
    pub zorder: WindowZOrder,
    pub restore: Option<(Rect, TilePosition)>,
    image: ImageAligned,
    title_image: Image,
    title_image_unfocused: Image,
    pub events: VecDeque<Event>,
    pub notified_read: bool,
    //TODO: implement better clipboard mechanism
    pub clipboard_seek: usize,
    pub mouse_cursor: bool,
    pub mouse_grab: bool,
    pub mouse_relative: bool,
    pub maps: usize,

    config: Rc<Config>,
}

const TITLE_HEIGHT: i32 = 28;
const TITLE_TEXT_HEIGHT: i32 = 16;

impl Window {
    // TODO Consider creating Rect for the title area, max and close areas and removing a lot
    // of the inline size calculations below
    pub fn new(x: i32, y: i32, w: i32, h: i32, scale: i32, config: Rc<Config>) -> Window {
        Window {
            x,
            y,
            scale,
            title: String::new(),
            asynchronous: false,
            borderless: false,
            hidden: false,
            resizable: false,
            transparent: false,
            unclosable: false,
            zorder: WindowZOrder::Normal,
            restore: None,
            // TODO: get a system constant for the page size
            image: ImageAligned::new(w, h, 4096), // Ensure that image data is page aligned at beginning and end
            title_image: Image::new(0, 0),
            title_image_unfocused: Image::new(0, 0),
            events: VecDeque::new(),
            notified_read: false,
            //TODO: implement better clipboard mechanism
            clipboard_seek: 0,
            mouse_cursor: true,
            mouse_grab: false,
            mouse_relative: false,
            maps: 0,
            config,
        }
    }

    pub fn width(&self) -> i32 {
        self.image.width()
    }

    pub fn height(&self) -> i32 {
        self.image.height()
    }

    pub fn rect(&self) -> Rect {
        if self.hidden {
            Rect::new(self.x, self.y, 0, 0)
        } else {
            Rect::new(self.x, self.y, self.width(), self.height())
        }
    }

    pub fn title_rect(&self) -> Rect {
        if self.borderless || self.hidden {
            Rect::new(self.x, self.y, 0, 0)
        } else {
            Rect::new(
                self.x,
                self.y - TITLE_HEIGHT * self.scale,
                self.width(),
                TITLE_HEIGHT * self.scale,
            )
        }
    }

    pub fn cascade_rect(&self) -> Rect {
        let title_rect = self.title_rect();
        Rect::new(title_rect.left(), title_rect.top(), 32, 32)
    }

    pub fn bottom_border_rect(&self) -> Rect {
        if self.resizable {
            Rect::new(self.x, self.y + self.height(), self.width(), 8 * self.scale)
        } else {
            Rect::new(-1, -1, 0, 0)
        }
    }

    pub fn bottom_left_border_rect(&self) -> Rect {
        if self.resizable {
            Rect::new(
                self.x - 8 * self.scale,
                self.y + self.height(),
                8 * self.scale,
                8 * self.scale,
            )
        } else {
            Rect::new(-1, -1, 0, 0)
        }
    }

    pub fn bottom_right_border_rect(&self) -> Rect {
        if self.resizable {
            Rect::new(
                self.x + self.width(),
                self.y + self.height(),
                8 * self.scale,
                8 * self.scale,
            )
        } else {
            Rect::new(-1, -1, 0, 0)
        }
    }

    pub fn left_border_rect(&self) -> Rect {
        if self.resizable {
            Rect::new(
                self.x - 8 * self.scale,
                self.y,
                8 * self.scale,
                self.height(),
            )
        } else {
            Rect::new(-1, -1, 0, 0)
        }
    }

    pub fn right_border_rect(&self) -> Rect {
        if self.resizable {
            Rect::new(self.x + self.width(), self.y, 8 * self.scale, self.height())
        } else {
            Rect::new(-1, -1, 0, 0)
        }
    }

    pub fn max_contains(&self, x: i32, y: i32) -> bool {
        !self.borderless
            && x >= max(
                self.x + 6 * self.scale,
                self.x + self.width() - 36 * self.scale,
            )
            && y >= self.y - TITLE_HEIGHT * self.scale
            && x < self.x + self.width() - 18 * self.scale
            && y < self.y
    }

    pub fn close_contains(&self, x: i32, y: i32) -> bool {
        !self.borderless
            && x >= max(
                self.x + 6 * self.scale,
                self.x + self.width() - 18 * self.scale,
            )
            && y >= self.y - TITLE_HEIGHT * self.scale
            && x < self.x + self.width()
            && y < self.y
    }

    pub fn draw_title(
        &self,
        display: &mut Display,
        rect: &Rect,
        focused: bool,
        window_max: &Image,
        window_close: &Image,
    ) {
        let bar_color = Color::from(self.config.bar_color);
        let bar_highlight_color = Color::from(self.config.bar_highlight_color);

        let title_rect = self.title_rect();
        let title_intersect = rect.intersection(&title_rect);
        if !title_intersect.is_empty() {
            display.rect(
                &title_intersect,
                if focused {
                    bar_highlight_color
                } else {
                    bar_color
                },
            );

            let mut x = self.x + 6 * self.scale;
            let w = max(
                self.x + 6 * self.scale,
                self.x + self.width() - 18 * self.scale,
            ) - x;
            if w > 0 {
                let title_image = if focused {
                    &self.title_image
                } else {
                    &self.title_image_unfocused
                };
                let image_rect = Rect::new(
                    x,
                    title_rect.top() + 6 * self.scale,
                    min(w, title_image.width()),
                    title_image.height(),
                );
                let image_intersect = rect.intersection(&image_rect);
                if !image_intersect.is_empty() {
                    display.roi_mut(&image_intersect).blend(
                        &title_image
                            .roi(&image_intersect.offset(-image_rect.left(), -image_rect.top())),
                    );
                }
            }

            if self.resizable {
                x = max(self.x + 6, self.x + self.width() - 36 * self.scale);
                if x + 36 * self.scale <= self.x + self.width() {
                    let image_rect = Rect::new(
                        x,
                        title_rect.top() + 7 * self.scale,
                        window_max.width(),
                        window_max.height(),
                    );
                    let image_intersect = rect.intersection(&image_rect);
                    if !image_intersect.is_empty() {
                        display.roi_mut(&image_intersect).blend(
                            &window_max.roi(
                                &image_intersect.offset(-image_rect.left(), -image_rect.top()),
                            ),
                        );
                    }
                }
            }

            if !self.unclosable {
                x = max(
                    self.x + 6 * self.scale,
                    self.x + self.width() - 18 * self.scale,
                );
                if x + 18 * self.scale <= self.x + self.width() {
                    let image_rect = Rect::new(
                        x,
                        title_rect.top() + 7 * self.scale,
                        window_close.width(),
                        window_close.height(),
                    );
                    let image_intersect = rect.intersection(&image_rect);
                    if !image_intersect.is_empty() {
                        display.roi_mut(&image_intersect).blend(
                            &window_close.roi(
                                &image_intersect.offset(-image_rect.left(), -image_rect.top()),
                            ),
                        );
                    }
                }
            }
        }
    }

    pub fn draw(&self, display: &mut Display, rect: &Rect) {
        let self_rect = self.rect();
        let intersect = self_rect.intersection(rect);
        if !intersect.is_empty() {
            if self.transparent {
                display.roi_mut(&intersect).blend(
                    &self
                        .image
                        .roi(&intersect.offset(-self_rect.left(), -self_rect.top())),
                );
            } else {
                display.roi_mut(&intersect).blit(
                    &self
                        .image
                        .roi(&intersect.offset(-self_rect.left(), -self_rect.top())),
                );
            }
        }
    }

    pub fn event(&mut self, event: Event) {
        // Combine or replace the last event for some event types where it improves latency without disrupting logic
        if let Some(last_event) = self.events.back_mut() {
            if last_event.code == event.code {
                match event.code {
                    // Absolute mouse events, window move, window resize, and screen report events can be replaced
                    orbclient::EVENT_MOUSE
                    | orbclient::EVENT_MOVE
                    | orbclient::EVENT_RESIZE
                    | orbclient::EVENT_SCREEN => {
                        *last_event = event;
                        return;
                    }
                    // Relative mouse events and scroll events can be combined with addition
                    orbclient::EVENT_MOUSE_RELATIVE | orbclient::EVENT_SCROLL => {
                        last_event.a += event.a;
                        last_event.b += event.b;
                        return;
                    }
                    // Other events cannot be combined or replaced
                    _ => {}
                }
            }
        }

        // Push event if not combined or replaced
        self.events.push_back(event);
    }

    pub fn map(&mut self) -> &mut [Color] {
        self.image.data_mut()
    }

    pub fn read(&mut self, buf: &mut [Event]) -> usize {
        for (i, event) in buf.iter_mut().enumerate() {
            *event = match self.events.pop_front() {
                Some(item) => item,
                None => return i,
            };
        }
        buf.len()
    }

    pub fn properties(&self) -> Properties {
        //TODO: avoid allocation
        let mut flags = String::with_capacity(9);
        if self.asynchronous {
            flags.push(ORBITAL_FLAG_ASYNC);
        }
        if self.borderless {
            flags.push(ORBITAL_FLAG_BORDERLESS);
        }
        if self.hidden {
            flags.push(ORBITAL_FLAG_HIDDEN);
        }
        if let Some((_, position)) = &self.restore {
            flags.push(ORBITAL_FLAG_MAXIMIZED);
            if matches!(position, TilePosition::FullScreen) {
                flags.push(ORBITAL_FLAG_FULLSCREEN);
            }
        }
        if self.resizable {
            flags.push(ORBITAL_FLAG_RESIZABLE);
        }
        if self.transparent {
            flags.push(ORBITAL_FLAG_TRANSPARENT);
        }
        if self.unclosable {
            flags.push(ORBITAL_FLAG_UNCLOSABLE);
        }
        match self.zorder {
            WindowZOrder::Back => flags.push(ORBITAL_FLAG_BACK),
            WindowZOrder::Normal => {}
            WindowZOrder::Front => flags.push(ORBITAL_FLAG_FRONT),
        }
        Properties {
            flags,
            x: self.x,
            y: self.y,
            width: self.width(),
            height: self.height(),
            title: &self.title,
        }
    }

    pub fn render_title(&mut self, font: &Font) {
        let text_color = self.config.text_color;
        let text_highlight_color = self.config.text_highlight_color;

        let title_render = font.render(&self.title, (TITLE_TEXT_HEIGHT * self.scale) as f32);

        let color_blank = Color::rgba(0, 0, 0, 0);

        self.title_image = Image::from_color(
            title_render.width() as i32,
            title_render.height() as i32,
            color_blank,
        );
        self.title_image.mode().set(orbclient::Mode::Overwrite);
        title_render.draw(&mut self.title_image, 0, 0, text_highlight_color.into());

        self.title_image_unfocused = Image::from_color(
            title_render.width() as i32,
            title_render.height() as i32,
            color_blank,
        );
        self.title_image_unfocused
            .mode()
            .set(orbclient::Mode::Overwrite);
        title_render.draw(&mut self.title_image_unfocused, 0, 0, text_color.into());
    }

    pub fn set_flag(&mut self, flag: char, value: bool) {
        match flag {
            ORBITAL_FLAG_ASYNC => self.asynchronous = value,
            ORBITAL_FLAG_BACK => {
                self.zorder = if value {
                    WindowZOrder::Back
                } else {
                    WindowZOrder::Normal
                }
            }
            ORBITAL_FLAG_FRONT => {
                self.zorder = if value {
                    WindowZOrder::Front
                } else {
                    WindowZOrder::Normal
                }
            }
            ORBITAL_FLAG_HIDDEN => self.hidden = value,
            ORBITAL_FLAG_BORDERLESS => self.borderless = value,
            ORBITAL_FLAG_RESIZABLE => self.resizable = value,
            ORBITAL_FLAG_TRANSPARENT => self.transparent = value,
            ORBITAL_FLAG_UNCLOSABLE => self.unclosable = value,
            _ => {
                log::warn!("unknown window flag {:?}", flag);
            }
        }
    }

    pub fn set_size(&mut self, w: i32, h: i32) {
        if self.maps > 0 {
            log::warn!("resized while {} mapping(s) still held", self.maps);
        }

        //TODO: Invalidate old mappings
        let mut new_image = ImageAligned::new(w, h, 4096);
        let new_rect = Rect::new(0, 0, w, h);

        let rect = Rect::new(0, 0, self.image.width(), self.image.height());
        let intersect = new_rect.intersection(&rect);
        if !intersect.is_empty() {
            new_image
                .roi_mut(&intersect)
                .blit(&self.image.roi(&intersect));
        }

        self.image = new_image;
    }
}

#[cfg(test)]
mod test {
    use crate::config::Config;
    use crate::window::Window;
    use orbclient::{Color, Event};
    use std::rc::Rc;

    // create a default config that can be used to create Windows for testing
    // TODO implement or derive Default for orbclient::Color and then just use Config::default()
    fn test_config() -> Config {
        Config {
            cursor: String::default(),
            bottom_left_corner: String::default(),
            bottom_right_corner: String::default(),
            bottom_side: String::default(),
            left_side: String::default(),
            right_side: String::default(),
            window_max: String::default(),
            window_max_unfocused: String::default(),
            window_close: String::default(),
            window_close_unfocused: String::default(),

            background_color: Color::rgba(1, 2, 3, 200).into(),
            bar_color: Color::rgba(1, 2, 3, 200).into(),
            bar_highlight_color: Color::rgba(1, 2, 3, 200).into(),
            text_color: Color::rgba(1, 2, 3, 200).into(),
            text_highlight_color: Color::rgba(1, 2, 3, 200).into(),
        }
    }

    #[test]
    fn read_limited_to_buffer_size() {
        // create a test Window
        let dummy_config = test_config();
        let mut window = Window::new(0, 0, 100, 100, 1, Rc::new(dummy_config));

        // Add three events to the window's queue of events
        let mut event_1 = Event::new();
        event_1.code = 1;
        window.events.push_back(event_1);
        let mut event_2 = Event::new();
        event_2.code = 2;
        window.events.push_back(event_2);
        let mut event_3 = Event::new();
        event_3.code = 3;
        window.events.push_back(event_3);

        // Our buffer (elements must be initialized!) will only have a length of 2
        let mut buf: Vec<Event> = vec![Event::new(), Event::new()]; // code = 0
        assert_eq!(
            buf.as_mut_slice().len(),
            2,
            "Buffer is not of length 2 as expected"
        );

        // let's try and read three events from the queue into the buffer of size two
        assert_eq!(
            window.read(buf.as_mut_slice()),
            2,
            "Did not read two events as expected"
        );
        // we should not crash with an indexing error beyond the length of the vectors/slices passed to read()

        // buf contains the correct events in the correct order
        let code = buf[0].code; // avoid misaligned access for packed Event :-(
        assert_eq!(code, 1);
        let code = buf[1].code; // avoid misaligned access for packed Event :-(
        assert_eq!(code, 2);
    }

    #[test]
    fn read_limited_to_available_events() {
        // create a test Window
        let dummy_config = test_config();
        let mut window = Window::new(0, 0, 100, 100, 1, Rc::new(dummy_config));

        // Add two events to the window's queue of events
        let mut event_1 = Event::new();
        event_1.code = 1;
        window.events.push_back(event_1);
        let mut event_2 = Event::new();
        event_2.code = 2;
        window.events.push_back(event_2);

        // Our buffer (elements must be initialized!) will have a length of 4
        let mut buf: Vec<Event> = vec![Event::new(), Event::new(), Event::new(), Event::new()];
        assert_eq!(
            buf.as_mut_slice().len(),
            4,
            "Buffer is not of length 4 as expected"
        );

        // let's try and read 2 events from the queue into the buffer
        assert_eq!(
            window.read(buf.as_mut_slice()),
            2,
            "Did not read two events as expected"
        );
        // we should not panic with an indexing error beyond the length of the windows event queue

        // buf contains the correct events in the correct order
        let code = buf[0].code; // avoid misaligned access for packed Event :-(
        assert_eq!(code, 1);
        let code = buf[1].code; // avoid misaligned access for packed Event :-(
        assert_eq!(code, 2);
    }

    #[test]
    fn read_empty_queue_returns_zero() {
        // create a test Window
        let dummy_config = test_config();
        let mut window = Window::new(0, 0, 100, 100, 1, Rc::new(dummy_config));

        // Our buffer (elements must be initialized!) will have a length of 2
        let mut buf: Vec<Event> = vec![Event::new(), Event::new()];
        assert_eq!(
            buf.as_mut_slice().len(),
            2,
            "Buffer is not of length 2 as expected"
        );

        // let's try and read events from the empty queue into the buffer
        assert_eq!(
            window.read(buf.as_mut_slice()),
            0,
            "Did not expect to read any events"
        );
    }
}
