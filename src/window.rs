use orbclient::{Color, Event, Renderer};
use std::cmp::{min, max};
use std::collections::VecDeque;
use std::mem::size_of;
use std::{ptr, slice};

use image::{fast_copy, Image, ImageRef};
use rect::Rect;

use syscall::error::{Error, Result, EINVAL};

use theme::{BAR_COLOR, BAR_HIGHLIGHT_COLOR, TEXT_COLOR, TEXT_HIGHLIGHT_COLOR};

pub struct Window {
    pub x: i32,
    pub y: i32,
    pub async: bool,
    image: Image,
    char_image: Image,
    title: String,
    pub events: VecDeque<Event>,
}

impl Window {
    pub fn new(x: i32, y: i32, w: i32, h: i32, title: String, async: bool) -> Window {
        Window {
            x: x,
            y: y,
            image: Image::new(w, h),
            char_image: Image::new(8, 16),
            title: title,
            async: async,
            events: VecDeque::new()
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
        if self.title.is_empty() {
            Rect::default()
        } else {
            Rect::new(self.x, self.y - 18, self.width(), 18)
        }
    }

    pub fn exit_contains(&self, x: i32, y: i32) -> bool {
        ! self.title.is_empty() && x >= max(self.x, self.x + self.width() - 10)  && y >= self.y - 18 && x < self.x + self.width() && y < self.y
    }

    pub fn draw_title(&mut self, image: &mut ImageRef, rect: &Rect, focused: bool) {
        let title_rect = self.title_rect();
        let title_intersect = rect.intersection(&title_rect);
        if ! title_intersect.is_empty() {
            let bar_color = if focused { BAR_HIGHLIGHT_COLOR } else { BAR_COLOR };
            let text_color = if focused { TEXT_HIGHLIGHT_COLOR } else { TEXT_COLOR };

            image.rect(title_intersect.left(), title_intersect.top(),
                       title_intersect.width() as u32, title_intersect.height() as u32,
                       bar_color);

            let mut x = self.x + 2;
            for c in self.title.chars() {
                if x < max(self.x + 2, self.x + self.width() - 10) {
                    let image_rect = Rect::new(x, title_rect.top() + 1, 8, 16);
                    let image_intersect = rect.intersection(&image_rect);
                    if ! image_intersect.is_empty() {
                        self.char_image.set(Color::rgba(0, 0, 0, 0));
                        self.char_image.char(0, 0, c, text_color);
                        image.roi(&image_intersect).blend(&self.char_image.roi(&image_intersect.offset(-image_rect.left(), -image_rect.top())));
                    }
                    x += 8;
                } else {
                    break;
                }
            }

            x = max(self.x + 2, self.x + self.width() - 10);
            if x + 10 <= self.x + self.width() {
                let image_rect = Rect::new(x, title_rect.top() + 1, 8, 16);
                let image_intersect = rect.intersection(&image_rect);
                if ! image_intersect.is_empty() {
                    self.char_image.set(Color::rgba(0, 0, 0, 0));
                    self.char_image.char(0, 0, 'X', text_color);
                    image.roi(&image_intersect).blend(&self.char_image.roi(&image_intersect.offset(-image_rect.left(), -image_rect.top())));
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

    pub fn map(&self, offset: usize, size: usize) -> Result<usize> {
        if offset + size <= self.image.data().len() * 4 {
            Ok(self.image.data().as_ptr() as usize + offset)
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

    pub fn write(&mut self, buf: &[u8]) -> Result<usize> {
        let old = self.image.data_mut();
        let new = unsafe { slice::from_raw_parts(buf.as_ptr() as *const u32, buf.len() / 4) };

        let len = min(old.len(), new.len());
        unsafe {
            fast_copy(old.as_mut_ptr() as *mut u8, new.as_ptr() as *const u8, len * 4);
        }

        Ok(len)
    }

    pub fn path(&self, buf: &mut [u8]) -> Result<usize> {
        let mut i = 0;
        let path_str = format!("orbital:{}/{}/{}/{}/{}/{}", if self.async { "a" } else { "" }, self.x, self.y, self.width(), self.height(), self.title);
        let path = path_str.as_bytes();
        while i < buf.len() && i < path.len() {
            buf[i] = path[i];
            i += 1;
        }
        Ok(i)
    }
}
