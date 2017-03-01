use orbclient::{Color, Event, Renderer};
use orbfont::Font;
use std::cmp::{min, max};
use std::collections::VecDeque;
use std::mem::size_of;
use std::{ptr, str};

use image::{Image, ImageRef};
use rect::Rect;
use theme::{BAR_COLOR, BAR_HIGHLIGHT_COLOR, TEXT_COLOR, TEXT_HIGHLIGHT_COLOR};

use syscall::error::{Error, Result, EINVAL};


pub struct Window {
    pub x: i32,
    pub y: i32,
    pub async: bool,
    pub resizable: bool,
    pub title: String,
    image: Image,
    title_image: Image,
    title_image_unfocused: Image,
    pub events: VecDeque<Event>,
}

impl Window {
    pub fn new(x: i32, y: i32, w: i32, h: i32, title: String, async: bool, resizable: bool, font: &Font) -> Window {
        let mut window = Window {
            x: x,
            y: y,
            async: async,
            resizable: resizable,
            title: title,
            image: Image::new(w, h),
            title_image: Image::new(0, 0),
            title_image_unfocused: Image::new(0, 0),
            events: VecDeque::new()
        };

        window.render_title(font);

        window
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
        if self.title.is_empty() {
            Rect::new(-1, -1, 0, 0)
        } else {
            Rect::new(self.x, self.y - 28, self.width(), 28)
        }
    }

    pub fn right_border_rect(&self) -> Rect {
        if self.resizable {
            Rect::new(self.x + self.width(), self.y, 8, self.height())
        } else {
            Rect::new(-1, -1, 0, 0)
        }
    }

    pub fn bottom_border_rect(&self) -> Rect {
        if self.resizable {
            Rect::new(self.x, self.y + self.height(), self.width(), 8)
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

    pub fn exit_contains(&self, x: i32, y: i32) -> bool {
        ! self.title.is_empty() && x >= max(self.x + 6, self.x + self.width() - 18)  && y >= self.y - 28 && x < self.x + self.width() && y < self.y
    }

    pub fn draw_title(&mut self, image: &mut ImageRef, rect: &Rect, focused: bool, window_close: &mut Image) {
        let title_rect = self.title_rect();
        let title_intersect = rect.intersection(&title_rect);
        if ! title_intersect.is_empty() {
            image.rect(title_intersect.left(), title_intersect.top(),
                       title_intersect.width() as u32, title_intersect.height() as u32,
                       if focused { BAR_HIGHLIGHT_COLOR } else { BAR_COLOR });

            let mut x = self.x + 6;
            let w = max(self.x + 6, self.x + self.width() - 18) - x;
            if w > 0 {
                let mut title_image = if focused { &mut self.title_image } else { &mut self.title_image_unfocused };
                let image_rect = Rect::new(x, title_rect.top() + 6, min(w, title_image.width()), title_image.height());
                let image_intersect = rect.intersection(&image_rect);
                if ! image_intersect.is_empty() {
                    image.roi(&image_intersect).blend(&title_image.roi(&image_intersect.offset(-image_rect.left(), -image_rect.top())));
                }
            }

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

    pub fn draw(&mut self, image: &mut ImageRef, rect: &Rect) {
        let self_rect = self.rect();
        let intersect = self_rect.intersection(&rect);
        if ! intersect.is_empty() {
            image.roi(&intersect).blend(&self.image.roi(&intersect.offset(-self_rect.left(), -self_rect.top())));
        }
    }

    pub fn event(&mut self, event: Event) {
        self.events.push_back(event);
    }

    pub fn map(&mut self, offset: usize, size: usize) -> Result<usize> {
        if offset + size <= self.image.data().len() * 4 {
            Ok(self.image.data_mut().as_mut_ptr() as usize + offset)
        } else {
            Err(Error::new(EINVAL))
        }
    }

    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        if buf.len() >= size_of::<Event>() {
            let mut i = 0;
            while i <= buf.len() - size_of::<Event>() {
                if let Some(event) = self.events.pop_front() {
                    unsafe { ptr::write(buf.as_mut_ptr().offset(i as isize) as *mut Event, event) };
                    i += size_of::<Event>();
                } else {
                    break;
                }
            }
            Ok(i)
        } else {
            Err(Error::new(EINVAL))
        }
    }

    pub fn path(&self, buf: &mut [u8]) -> Result<usize> {
        let mut i = 0;
        let path_str = format!(
            "orbital:{}{}/{}/{}/{}/{}/{}",
            if self.async { "a" } else { "" },
            if self.resizable { "r" } else { "" },
            self.x, self.y, self.width(), self.height(), self.title
        );
        let path = path_str.as_bytes();
        while i < buf.len() && i < path.len() {
            buf[i] = path[i];
            i += 1;
        }
        Ok(i)
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
