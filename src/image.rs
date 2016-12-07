use orbclient::color::Color;
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

#[cfg(target_arch = "x86_64")]
#[inline(always)]
#[cold]
pub unsafe fn fast_set32(dst: *mut u32, src: u32, len: usize) {
    asm!("cld
        rep stosd"
        :
        : "{rdi}"(dst as usize), "{eax}"(src), "{rcx}"(len)
        : "cc", "memory", "rdi", "rcx"
        : "intel", "volatile");
}

pub struct ImageRoiRows<'a> {
    rect: Rect,
    w: i32,
    data: &'a [u32],
    i: i32,
}

impl<'a> Iterator for ImageRoiRows<'a> {
    type Item = &'a [u32];
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
    data: &'a mut [u32],
    i: i32,
}

impl<'a> Iterator for ImageRoiRowsMut<'a> {
    type Item = &'a mut [u32];
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
    data: &'a mut [u32]
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
                let alpha = (*new >> 24) & 0xFF;
                if alpha >= 255 {
                    *old = *new;
                } else if alpha > 0 {
                    let n_r = (((*new >> 16) & 0xFF) * alpha) >> 8;
                    let n_g = (((*new >> 8) & 0xFF) * alpha) >> 8;
                    let n_b = ((*new & 0xFF) * alpha) >> 8;

                    let n_alpha = 255 - alpha;

                    let o_r = (((*old >> 16) & 0xFF) * n_alpha) >> 8;
                    let o_g = (((*old >> 8) & 0xFF) * n_alpha) >> 8;
                    let o_b = ((*old & 0xFF) * n_alpha) >> 8;

                    *old = ((o_r << 16) | (o_g << 8) | o_b) + ((n_r << 16) | (n_g << 8) | n_b);
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

    pub fn set(&'a mut self, color: Color) {
        let new = color.data;

        let alpha = (new >> 24) & 0xFF;
        if alpha >= 255 {
            for mut self_row in self.rows_mut() {
                unsafe { fast_set32(self_row.as_mut_ptr() as *mut u32, new, self_row.len()); }
            }
        } else if alpha > 0 {
            let n_r = (((new >> 16) & 0xFF) * alpha) >> 8;
            let n_g = (((new >> 8) & 0xFF) * alpha) >> 8;
            let n_b = ((new & 0xFF) * alpha) >> 8;

            let n_alpha = 255 - alpha;

            for mut self_row in self.rows_mut() {
                for mut old in self_row.iter_mut() {
                    let o_r = (((*old >> 16) & 0xFF) * n_alpha) >> 8;
                    let o_g = (((*old >> 8) & 0xFF) * n_alpha) >> 8;
                    let o_b = ((*old & 0xFF) * n_alpha) >> 8;

                    *old = ((o_r << 16) | (o_g << 8) | o_b) + ((n_r << 16) | (n_g << 8) | n_b);
                }
            }
        }
    }
}

pub struct ImageRef<'a> {
    w: i32,
    h: i32,
    data: &'a mut [u32]
}

impl<'a> ImageRef<'a> {
    pub fn from_data(width: i32, height: i32, data: &'a mut [u32]) -> ImageRef {
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

pub struct Image {
    w: i32,
    h: i32,
    data: Box<[u32]>
}

impl Image {
    pub fn new(width: i32, height: i32) -> Image {
        Image::from_color(width, height, Color::rgb(0, 0, 0))
    }

    pub fn from_color(width: i32, height: i32, color: Color) -> Image {
        Image::from_data(width, height, vec![color.data; width as usize * height as usize].into_boxed_slice())
    }

    pub fn from_data(width: i32, height: i32, data: Box<[u32]>) -> Image {
        Image {
            w: width,
            h: height,
            data: data
        }
    }

    pub fn from_path<P: AsRef<Path>>(path: P) -> Image {
        match orbimage::Image::from_path(path) {
            Ok(orb_image) => {
                let width = orb_image.width();
                let height = orb_image.height();
                let data = orb_image.into_data();
                Image::from_data(width as i32, height as i32, unsafe { mem::transmute(data) })
            },
            Err(err) => {
                println!("orbital Image::from_path: {}", err);
                Image::new(0, 0)
            }
        }
    }

    pub fn width(&self) -> i32 {
        self.w
    }

    pub fn height(&self) -> i32 {
        self.h
    }

    pub fn data(&self) -> &[u32] {
        &self.data
    }

    pub fn data_mut(&mut self) -> &mut [u32] {
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
