use orbclient::{Color, Renderer};
use orbimage;
use std::{cmp, mem};
use std::path::Path;

use rect::Rect;

#[cfg(target_arch = "x86_64")]
#[inline(always)]
#[cold]
pub unsafe fn fast_copy(dst: *mut u8, src: *const u8, len: usize) {
    asm!("cld
        rep movsb"
        :
        : "{rdi}"(dst as usize), "{rsi}"(src as usize), "{rcx}"(len)
        : "cc", "memory", "rdi", "rsi", "rcx"
        : "intel", "volatile");
}

pub struct ImageRoiRows<'a> {
    rect: Rect,
    w: i32,
    data: &'a [Color],
    i: i32,
}

impl<'a> Iterator for ImageRoiRows<'a> {
    type Item = &'a [Color];
    fn next(&mut self) -> Option<Self::Item> {
        if self.i < self.rect.height() {
            let start = (self.rect.top() + self.i) * self.w + self.rect.left();
            let end = start + self.rect.width();
            self.i += 1;
            Some(unsafe { mem::transmute(& self.data[start as usize .. end as usize]) })
        } else {
            None
        }
    }
}

pub struct ImageRoiRowsMut<'a> {
    rect: Rect,
    w: i32,
    data: &'a mut [Color],
    i: i32,
}

impl<'a> Iterator for ImageRoiRowsMut<'a> {
    type Item = &'a mut [Color];
    fn next(&mut self) -> Option<Self::Item> {
        if self.i < self.rect.height() {
            let start = (self.rect.top() + self.i) * self.w + self.rect.left();
            let end = start + self.rect.width();
            self.i += 1;
            // it does not appear to be possible to do this in safe rust
            Some(unsafe { mem::transmute(&mut self.data[start as usize .. end as usize]) })
        } else {
            None
        }
    }
}

pub struct ImageRoi<'a> {
    rect: Rect,
    w: i32,
    data: &'a mut [Color]
}

impl<'a> ImageRoi<'a> {
    pub fn rows(&'a self) -> ImageRoiRows<'a> {
        ImageRoiRows {
            rect: self.rect,
            w: self.w,
            data: self.data,
            i: 0
        }
    }

    pub fn rows_mut(&'a mut self) -> ImageRoiRowsMut<'a> {
        ImageRoiRowsMut {
            rect: self.rect,
            w: self.w,
            data: self.data,
            i: 0
        }
    }

    pub fn blend(&'a mut self, other: &ImageRoi) {
        for (mut self_row, other_row) in self.rows_mut().zip(other.rows()) {
            for(mut old, new) in self_row.iter_mut().zip(other_row.iter()) {
                let alpha = (new.data >> 24) & 0xFF;
                if alpha >= 255 {
                    old.data = new.data;
                } else if alpha > 0 {
                    let n_r = (((new.data >> 16) & 0xFF) * alpha) >> 8;
                    let n_g = (((new.data >> 8) & 0xFF) * alpha) >> 8;
                    let n_b = ((new.data & 0xFF) * alpha) >> 8;

                    let n_alpha = 255 - alpha;

                    let o_r = (((old.data >> 16) & 0xFF) * n_alpha) >> 8;
                    let o_g = (((old.data >> 8) & 0xFF) * n_alpha) >> 8;
                    let o_b = ((old.data & 0xFF) * n_alpha) >> 8;

                    old.data = ((o_r << 16) | (o_g << 8) | o_b) + ((n_r << 16) | (n_g << 8) | n_b);
                }
            }
        }
    }

    pub fn blit(&'a mut self, other: &ImageRoi) {
        for (mut self_row, other_row) in self.rows_mut().zip(other.rows()) {
            let len = cmp::min(self_row.len(), other_row.len());
            unsafe { fast_copy(self_row.as_mut_ptr() as *mut u8, other_row.as_ptr() as *const u8, len * 4); }
        }
    }
}

pub struct ImageRef<'a> {
    w: i32,
    h: i32,
    data: &'a mut [Color]
}

impl<'a> ImageRef<'a> {
    pub fn from_data(width: i32, height: i32, data: &'a mut [Color]) -> ImageRef {
        ImageRef {
            w: width,
            h: height,
            data: data
        }
    }

    pub fn width(&self) -> i32 {
        self.w
    }

    pub fn height(&self) -> i32 {
        self.h
    }

    pub fn roi(&mut self, rect: &Rect) -> ImageRoi {
        ImageRoi {
            rect: *rect,
            w: self.w,
            data: self.data
        }
    }
}

impl<'a> Renderer for ImageRef<'a> {
    /// Get the width of the image in pixels
    fn width(&self) -> u32 {
        self.w as u32
    }

    /// Get the height of the image in pixels
    fn height(&self) -> u32 {
        self.h as u32
    }

    /// Return a reference to a slice of colors making up the image
    fn data(&self) -> &[Color] {
        &self.data[..]
    }

    /// Return a mutable reference to a slice of colors making up the image
    fn data_mut(&mut self) -> &mut [Color] {
        &mut self.data[..]
    }

    fn sync(&mut self) -> bool {
        true
    }
}

#[derive(Clone)]
pub struct Image {
    w: i32,
    h: i32,
    data: Box<[Color]>
}

impl Image {
    pub fn new(width: i32, height: i32) -> Image {
        Image::from_color(width, height, Color::rgb(0, 0, 0))
    }

    pub fn from_color(width: i32, height: i32, color: Color) -> Image {
        Image::from_data(width, height, vec![color; width as usize * height as usize].into_boxed_slice())
    }

    pub fn from_data(width: i32, height: i32, data: Box<[Color]>) -> Image {
        Image {
            w: width,
            h: height,
            data: data
        }
    }

    pub fn from_path<P: AsRef<Path>>(path: P) -> Option<Image> {
        match orbimage::Image::from_path(path) {
            Ok(orb_image) => {
                let width = orb_image.width();
                let height = orb_image.height();
                let data = orb_image.into_data();
                Some(Image::from_data(width as i32, height as i32, unsafe { mem::transmute(data) }))
            },
            Err(err) => {
                println!("orbital Image::from_path: {}", err);
                None
            }
        }
    }

    pub fn width(&self) -> i32 {
        self.w
    }

    pub fn height(&self) -> i32 {
        self.h
    }

    pub fn data(&self) -> &[Color] {
        &self.data
    }

    pub fn data_mut(&mut self) -> &mut [Color] {
        &mut self.data
    }

    pub fn roi(&mut self, rect: &Rect) -> ImageRoi {
        ImageRoi {
            rect: *rect,
            w: self.w,
            data: &mut self.data
        }
    }
}

impl Renderer for Image {
    /// Get the width of the image in pixels
    fn width(&self) -> u32 {
        self.w as u32
    }

    /// Get the height of the image in pixels
    fn height(&self) -> u32 {
        self.h as u32
    }

    /// Return a reference to a slice of colors making up the image
    fn data(&self) -> &[Color] {
        &self.data[..]
    }

    /// Return a mutable reference to a slice of colors making up the image
    fn data_mut(&mut self) -> &mut [Color] {
        &mut self.data[..]
    }

    fn sync(&mut self) -> bool {
        true
    }
}
