use orbclient::{Color, Event, Renderer};
use orbfont::Font;
use orbital_core::{
    Properties,
    display::Display,
    image::{Image, ImageAligned},
    rect::Rect,
    self
};
use std::cmp::{min, max};
use std::collections::VecDeque;

use std::rc::Rc;

// use theme::{BAR_COLOR, BAR_HIGHLIGHT_COLOR, TEXT_COLOR, TEXT_HIGHLIGHT_COLOR};
use crate::config::Config;

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
    pub async: bool,
    pub borderless: bool,
    pub resizable: bool,
    pub transparent: bool,
    pub unclosable: bool,
    pub zorder: WindowZOrder,
    pub max_restore: Option<Rect>,
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

    config: Rc<Config>
}

impl Window {
    pub fn new(x: i32, y: i32, w: i32, h: i32, scale: i32, config: Rc<Config>) -> Window {
        Window {
            x,
            y,
            scale,
            title: String::new(),
            async: false,
            borderless: false,
            resizable: false,
            transparent: false,
            unclosable: false,
            zorder: WindowZOrder::Normal,
            max_restore: None,
            image: unsafe { ImageAligned::new(w, h, 4096) }, // Ensure that image data is page aligned at beginning and end
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

            config
        }
    }

    pub fn width(&self) -> i32 {
        self.image.width()
    }

    pub fn height(&self) -> i32 {
        self.image.height()
    }

    pub fn rect(&self) -> Rect {
        Rect::new(self.x, self.y, self.width(), self.height())
    }

    pub fn title_rect(&self) -> Rect {
        if self.borderless {
            Rect::new(-1, -1, 0, 0)
        } else {
            Rect::new(self.x, self.y - 28 * self.scale, self.width(), 28 * self.scale)
        }
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
            Rect::new(self.x - 8 * self.scale, self.y + self.height(), 8 * self.scale, 8 * self.scale)
        } else {
            Rect::new(-1, -1, 0, 0)
        }
    }

    pub fn bottom_right_border_rect(&self) -> Rect {
        if self.resizable {
            Rect::new(self.x + self.width(), self.y + self.height(), 8 * self.scale, 8 * self.scale)
        } else {
            Rect::new(-1, -1, 0, 0)
        }
    }

    pub fn left_border_rect(&self) -> Rect {
        if self.resizable {
            Rect::new(self.x - 8 * self.scale, self.y, 8 * self.scale, self.height())
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
        ! self.borderless && x >= max(self.x + 6 * self.scale, self.x + self.width() - 36 * self.scale)  && y >= self.y - 28 * self.scale && x < self.x + self.width() - 18 * self.scale && y < self.y
    }

    pub fn close_contains(&self, x: i32, y: i32) -> bool {
        ! self.borderless && x >= max(self.x + 6 * self.scale, self.x + self.width() - 18 * self.scale)  && y >= self.y - 28 * self.scale && x < self.x + self.width() && y < self.y
    }

    pub fn draw_title(&mut self, display: &mut Display, rect: &Rect, focused: bool, window_max: &mut Image, window_close: &mut Image) {
        let bar_color = self.config.bar_color;
        let bar_highlight_color = self.config.bar_highlight_color;

        let title_rect = self.title_rect();
        let title_intersect = rect.intersection(&title_rect);
        if ! title_intersect.is_empty() {
            display.rect(&title_intersect, if focused { bar_highlight_color } else { bar_color });

            let mut x = self.x + 6 * self.scale;
            let w = max(self.x + 6 * self.scale, self.x + self.width() - 18 * self.scale) - x;
            if w > 0 {
                let title_image = if focused { &mut self.title_image } else { &mut self.title_image_unfocused };
                let image_rect = Rect::new(x, title_rect.top() + 6 * self.scale, min(w, title_image.width()), title_image.height());
                let image_intersect = rect.intersection(&image_rect);
                if ! image_intersect.is_empty() {
                    display.roi(&image_intersect).blend(&title_image.roi(&image_intersect.offset(-image_rect.left(), -image_rect.top())));
                }
            }

            if self.resizable {
                x = max(self.x + 6, self.x + self.width() - 36 * self.scale);
                if x + 36 * self.scale <= self.x + self.width() {
                    let image_rect = Rect::new(x, title_rect.top() + 7 * self.scale, window_max.width(), window_max.height());
                    let image_intersect = rect.intersection(&image_rect);
                    if ! image_intersect.is_empty() {
                        display.roi(&image_intersect).blend(&window_max.roi(&image_intersect.offset(-image_rect.left(), -image_rect.top())));
                    }
                }
            }

            if !self.unclosable {
                x = max(self.x + 6 * self.scale, self.x + self.width() - 18 * self.scale);
                if x + 18 * self.scale <= self.x + self.width() {
                    let image_rect = Rect::new(x, title_rect.top() + 7 * self.scale, window_close.width(), window_close.height());
                    let image_intersect = rect.intersection(&image_rect);
                    if ! image_intersect.is_empty() {
                        display.roi(&image_intersect).blend(&window_close.roi(&image_intersect.offset(-image_rect.left(), -image_rect.top())));
                    }
                }
            }
        }
    }

    pub fn draw(&mut self, display: &mut Display, rect: &Rect) {
        let self_rect = self.rect();
        let intersect = self_rect.intersection(rect);
        if ! intersect.is_empty() {
            if self.transparent {
                display.roi(&intersect).blend(&self.image.roi(&intersect.offset(-self_rect.left(), -self_rect.top())));
            } else {
                display.roi(&intersect).blit(&self.image.roi(&intersect.offset(-self_rect.left(), -self_rect.top())));
            }
        }
    }

    pub fn event(&mut self, event: Event) {
        self.events.push_back(event);
    }

    pub fn map(&mut self) -> &mut [Color] {
        self.image.data_mut()
    }

    pub fn read(&mut self, buf: &mut [Event]) -> usize {
        for i in 0..buf.len() {
            buf[i] = match self.events.pop_front() {
                Some(item) => item,
                None => return i
            };
        }
        buf.len()
    }

    pub fn properties(&self) -> Properties {
        let mut properties = 0;
        if self.async { properties |= orbital_core::PROPERTY_ASYNC; }
        if self.borderless { properties |= orbital_core::PROPERTY_BORDERLESS; }
        if self.resizable { properties |= orbital_core::PROPERTY_RESIZABLE; }
        if self.transparent { properties |= orbital_core::PROPERTY_TRANSPARENT; }
        if self.unclosable { properties |= orbital_core::PROPERTY_UNCLOSABLE; }
        Properties {
            properties,
            x: self.x,
            y: self.y,
            width: self.width(),
            height: self.height(),
            title: &self.title
        }
    }

    pub fn render_title(&mut self, font: &Font) {
        let text_color = self.config.text_color;
        let text_highlight_color = self.config.text_highlight_color;

        let title_render = font.render(&self.title, (16 * self.scale) as f32);

        let color_blank = Color::rgba(0, 0, 0, 0);

        self.title_image = Image::from_color(title_render.width() as i32, title_render.height() as i32, color_blank);
        self.title_image.mode().set(orbclient::Mode::Overwrite);
        title_render.draw(&mut self.title_image, 0, 0, text_highlight_color);

        self.title_image_unfocused = Image::from_color(title_render.width() as i32, title_render.height() as i32, color_blank);
        self.title_image_unfocused.mode().set(orbclient::Mode::Overwrite);
        title_render.draw(&mut self.title_image_unfocused, 0, 0, text_color);
    }

    pub fn set_size(&mut self, w: i32, h: i32) {
        if self.maps > 0 {
            log::warn!("orbital: resized while {} mapping(s) still held", self.maps);
        }

        //TODO: Invalidate old mappings
        let mut new_image = unsafe { ImageAligned::new(w, h, 4096) };
        let new_rect = Rect::new(0, 0, w, h);

        let rect = Rect::new(0, 0, self.image.width(), self.image.height());
        let intersect = new_rect.intersection(&rect);
        if ! intersect.is_empty() {
            new_image.roi(&intersect).blit(&self.image.roi(&intersect));
        }

        self.image = new_image;
    }
}

#[cfg(test)]
mod test {
    use orbclient::{Color, Event};
    use window::Window;
    use std::rc::Rc;
    use config::Config;

    // create a default config that can be used to create Windows for testing
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

            background_color: Color::rgba(1, 2, 3, 200),
            bar_color: Color::rgba(1, 2, 3, 200),
            bar_highlight_color: Color::rgba(1, 2, 3, 200),
            text_color: Color::rgba(1, 2, 3, 200),
            text_highlight_color: Color::rgba(1, 2, 3, 200),
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
        let mut buf: Vec<Event>= vec!(Event::new(), Event::new()); // code = 0
        assert_eq!(buf.as_mut_slice().len(), 2, "Buffer is not of length 2 as expected");

        // let's try and read three events from the queue into the buffer of size two
        assert_eq!(window.read(buf.as_mut_slice()), 2, "Did not read two events as expected");
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
        let mut buf: Vec<Event>= vec!(Event::new(), Event::new(), Event::new(), Event::new());
        assert_eq!(buf.as_mut_slice().len(), 4, "Buffer is not of length 4 as expected");

        // let's try and read 2 events from the queue into the buffer
        assert_eq!(window.read(buf.as_mut_slice()), 2, "Did not read two events as expected");
        // we should not panic with an indexing error beyond the length of the windows event queue

        // buf contains the correct events in the correct order
        let code = buf[0].code; // avoid misaligned access for packed Event :-(
        assert_eq!(code, 1);
        let code = buf[1].code; // avoid misaligned access for packed Event :-(
        assert_eq!(code, 2);
    }
}
