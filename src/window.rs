use orbclient::{Color, Event, Renderer};
use orbfont::Font;
use orbital_core::{
    Properties,
    image::{Image, ImageRef},
    rect::Rect,
    self
};
use std::cmp::{min, max};
use std::collections::VecDeque;

use theme::{BAR_COLOR, BAR_HIGHLIGHT_COLOR, TEXT_COLOR, TEXT_HIGHLIGHT_COLOR};

#[derive(Copy, Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum WindowZOrder {
    Back,
    Normal,
    Front,
}

pub struct Window {
    pub x: i32,
    pub y: i32,
    pub title: String,
    pub async: bool,
    pub borderless: bool,
    pub resizable: bool,
    pub transparent: bool,
    pub unclosable: bool,
    pub zorder: WindowZOrder,
    pub max_restore: Option<Rect>,
    image: Image,
    title_image: Image,
    title_image_unfocused: Image,
    pub events: VecDeque<Event>,

    pub notified_read: bool
}

impl Window {
    pub fn new(x: i32, y: i32, w: i32, h: i32) -> Window {
        Window {
            x: x,
            y: y,
            title: String::new(),
            async: false,
            borderless: false,
            resizable: false,
            transparent: false,
            unclosable: false,
            zorder: WindowZOrder::Normal,
            max_restore: None,
            image: Image::new(w, h),
            title_image: Image::new(0, 0),
            title_image_unfocused: Image::new(0, 0),
            events: VecDeque::new(),

            notified_read: false
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
            Rect::new(self.x, self.y - 28, self.width(), 28)
        }
    }

    pub fn bottom_border_rect(&self) -> Rect {
        if self.resizable {
            Rect::new(self.x, self.y + self.height(), self.width(), 8)
        } else {
            Rect::new(-1, -1, 0, 0)
        }
    }

    pub fn bottom_left_border_rect(&self) -> Rect {
        if self.resizable {
            Rect::new(self.x - 8, self.y + self.height(), 8, 8)
        } else {
            Rect::new(-1, -1, 0, 0)
        }
    }

    pub fn bottom_right_border_rect(&self) -> Rect {
        if self.resizable {
            Rect::new(self.x + self.width(), self.y + self.height(), 8, 8)
        } else {
            Rect::new(-1, -1, 0, 0)
        }
    }

    pub fn left_border_rect(&self) -> Rect {
        if self.resizable {
            Rect::new(self.x - 8, self.y, 8, self.height())
        } else {
            Rect::new(-1, -1, 0, 0)
        }
    }

    pub fn right_border_rect(&self) -> Rect {
        if self.resizable {
            Rect::new(self.x + self.width(), self.y, 8, self.height())
        } else {
            Rect::new(-1, -1, 0, 0)
        }
    }

    pub fn max_contains(&self, x: i32, y: i32) -> bool {
        ! self.borderless && x >= max(self.x + 6, self.x + self.width() - 36)  && y >= self.y - 28 && x < self.x + self.width() - 18 && y < self.y
    }

    pub fn close_contains(&self, x: i32, y: i32) -> bool {
        ! self.borderless && x >= max(self.x + 6, self.x + self.width() - 18)  && y >= self.y - 28 && x < self.x + self.width() && y < self.y
    }

    pub fn draw_title(&mut self, image: &mut ImageRef, rect: &Rect, focused: bool, window_max: &mut Image, window_close: &mut Image) {
        let title_rect = self.title_rect();
        let title_intersect = rect.intersection(&title_rect);
        if ! title_intersect.is_empty() {
            image.rect(title_intersect.left(), title_intersect.top(),
                       title_intersect.width() as u32, title_intersect.height() as u32,
                       if focused { BAR_HIGHLIGHT_COLOR } else { BAR_COLOR });

            let mut x = self.x + 6;
            let w = max(self.x + 6, self.x + self.width() - 18) - x;
            if w > 0 {
                let title_image = if focused { &mut self.title_image } else { &mut self.title_image_unfocused };
                let image_rect = Rect::new(x, title_rect.top() + 6, min(w, title_image.width()), title_image.height());
                let image_intersect = rect.intersection(&image_rect);
                if ! image_intersect.is_empty() {
                    image.roi(&image_intersect).blend(&title_image.roi(&image_intersect.offset(-image_rect.left(), -image_rect.top())));
                }
            }

            if self.resizable {
                x = max(self.x + 6, self.x + self.width() - 36);
                if x + 36 <= self.x + self.width() {
                    let image_rect = Rect::new(x, title_rect.top() + 7, window_max.width(), window_max.height());
                    let image_intersect = rect.intersection(&image_rect);
                    if ! image_intersect.is_empty() {
                        image.roi(&image_intersect).blend(&window_max.roi(&image_intersect.offset(-image_rect.left(), -image_rect.top())));
                    }
                }
            }

            if !self.unclosable {
                x = max(self.x + 6, self.x + self.width() - 18);
                if x + 18 <= self.x + self.width() {
                    let image_rect = Rect::new(x, title_rect.top() + 7, window_close.width(), window_close.height());
                    let image_intersect = rect.intersection(&image_rect);
                    if ! image_intersect.is_empty() {
                        image.roi(&image_intersect).blend(&window_close.roi(&image_intersect.offset(-image_rect.left(), -image_rect.top())));
                    }
                }
            }
        }
    }

    pub fn draw(&mut self, image: &mut ImageRef, rect: &Rect) {
        let self_rect = self.rect();
        let intersect = self_rect.intersection(&rect);
        if ! intersect.is_empty() {
            if self.transparent {
                image.roi(&intersect).blend(&self.image.roi(&intersect.offset(-self_rect.left(), -self_rect.top())));
            } else {
                image.roi(&intersect).blit(&self.image.roi(&intersect.offset(-self_rect.left(), -self_rect.top())));
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
            properties: properties,
            x: self.x,
            y: self.y,
            width: self.width(),
            height: self.height(),
            title: &self.title
        }
    }

    pub fn render_title(&mut self, font: &Font) {
        let title_render = font.render(&self.title, 16.0);

        self.title_image = Image::from_color(title_render.width() as i32, title_render.height() as i32, BAR_HIGHLIGHT_COLOR);
        title_render.draw(&mut self.title_image, 0, 0, TEXT_HIGHLIGHT_COLOR);

        self.title_image_unfocused = Image::from_color(title_render.width() as i32, title_render.height() as i32, BAR_COLOR);
        title_render.draw(&mut self.title_image_unfocused, 0, 0, TEXT_COLOR);
    }

    pub fn set_size(&mut self, w: i32, h: i32) {
        let mut new_image = Image::from_color(w, h, Color::rgba(0, 0, 0, 0));
        let new_rect = Rect::new(0, 0, w, h);

        let rect = Rect::new(0, 0, self.image.width(), self.image.height());
        let intersect = new_rect.intersection(&rect);
        if ! intersect.is_empty() {
            new_image.roi(&intersect).blit(&self.image.roi(&intersect));
        }

        self.image = new_image;
    }
}
