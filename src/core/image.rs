use orbclient::{Color, Mode, Renderer};
use std::{cmp, mem, ptr, slice};
use std::cell::Cell;
use std::cmp::Ordering;
use std::path::Path;
use crate::core::rect::Rect;
use log::{debug, error};

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
            Some(& self.data[start as usize .. end as usize])
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
            let mut data = mem::take(&mut self.data);

            // skip section of data above top of rect
            if self.i == 0 {
                data = data.split_at_mut(self.rect.top() as usize * self.w as usize).1
            };

            // split after next row
            let (row, tail) = data.split_at_mut(self.w as usize);
            self.data = tail;                            // make data point to the remaining rows
            let start = self.rect.left() as usize;
            let end = self.rect.left() as usize + self.rect.width() as usize;
            self.i += 1;
            Some(&mut row[start .. end]) // return the rect part of the row
        } else {
            None
        }
    }
}

// ImageRoi seems to be a "window" onto an image, i.e. a Rectangular part of an image.
// `rect` defined the area within the larger image, we need to know the width of the image (`w`)
// to move through the data by rows, and `data` is a reference to the data in the actual image
pub struct ImageRoi<'a> {
    rect: Rect,
    w: i32,
    data: &'a mut [Color]
}


impl<'a> IntoIterator for ImageRoi<'a> {
    type Item = &'a [Color];
    type IntoIter = ImageRoiRows<'a>;

    fn into_iter(self) -> Self::IntoIter {
        let Self { rect, w, data } = self;
        let data = &mut data[rect.top() as usize * w as usize..][..rect.height() as usize * w as usize];
        ImageRoiRows { rect, w, data, i: 0}
    }
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
        for (self_row, other_row) in self.rows_mut().zip(other.rows()) {
            for (old, new) in self_row.iter_mut().zip(other_row.iter()) {
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
        for (self_row, other_row) in self.rows_mut().zip(other.rows()) {
            let len = cmp::min(self_row.len(), other_row.len());
            unsafe {
                ptr::copy(other_row.as_ptr(), self_row.as_mut_ptr(), len);
            }
        }
    }
}

pub struct ImageRef<'a> {
    w: i32,
    h: i32,
    data: &'a mut [Color],
    mode: Cell<Mode>
}

impl<'a> ImageRef<'a> {
    pub fn from_data(w: i32, h: i32, data: &'a mut [Color]) -> ImageRef {
        ImageRef { w, h, data, mode: Cell::new(Mode::Blend) }
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
        self.data
    }

    /// Return a mutable reference to a slice of colors making up the image
    fn data_mut(&mut self) -> &mut [Color] {
        self.data
    }

    fn sync(&mut self) -> bool {
        true
    }

    fn mode(&self) -> &Cell<Mode> {
        &self.mode
    }
}

#[derive(Clone)]
pub struct Image {
    w: i32,
    h: i32,
    data: Box<[Color]>,
    mode: Cell<Mode>,
}

impl Image {
    pub fn new(width: i32, height: i32) -> Image {
        Image::from_color(width, height, Color::rgb(0, 0, 0))
    }

    pub fn from_color(width: i32, height: i32, color: Color) -> Image {
        Image::from_data(width, height, vec![color; width as usize * height as usize].into_boxed_slice())
    }

    pub fn from_data(w: i32, h: i32, data: Box<[Color]>) -> Image {
        Image { w, h, data, mode: Cell::new(Mode::Blend) }
    }

    pub fn from_path_scale<P: AsRef<Path>>(path: P, scale: i32) -> Option<Image> {
        match orbimage::Image::from_path(path) {
            Ok(orb_image) => {
                let width = orb_image.width();
                let height = orb_image.height();
                let data = orb_image.into_data();
                match scale.cmp(&1) {
                    Ordering::Equal => Some(Image::from_data(
                        width as i32, height as i32, data
                    )),
                    Ordering::Greater => {
                        let mut new_data = vec![
                            Color::rgb(0, 0, 0);
                            data.len() * (scale * scale) as usize
                        ].into_boxed_slice();

                        for y in 0..height as i32 {
                            for x in 0..width as i32 {
                                let i = y * width as i32 + x;
                                let value = data[i as usize].data;
                                for y_s in 0..scale {
                                    for x_s in 0..scale {
                                        let new_i = (y * scale + y_s) * width as i32 * scale + x * scale + x_s;
                                        new_data[new_i as usize].data = value;
                                    }
                                }
                            }
                        }

                        Some(Image::from_data(
                            width as i32 * scale, height as i32 * scale, new_data
                        ))
                    },
                    Ordering::Less => {
                        debug!("Image::from_path_scale: scale {} < 1", scale);
                        None
                    }
                }
            },
            Err(err) => {
                error!("Image::from_path_scale: {}", err);
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
        &self.data
    }

    /// Return a mutable reference to a slice of colors making up the image
    fn data_mut(&mut self) -> &mut [Color] {
        &mut self.data
    }

    fn sync(&mut self) -> bool {
        true
    }

    fn mode(&self) -> &Cell<Mode> {
        &self.mode
    }
}

pub struct ImageAligned {
    w: i32,
    h: i32,
    data: &'static mut [Color],
    mode: Cell<Mode>,
}

impl Drop for ImageAligned {
    fn drop(&mut self) {
        unsafe { libc::free(self.data.as_mut_ptr() as *mut libc::c_void); }
    }
}

impl ImageAligned {
    pub fn new(w: i32, h: i32, align: usize) -> ImageAligned {
        let size = (w * h) as usize;
        let size_bytes = size * mem::size_of::<Color>();
        let size_alignments = (size_bytes + align - 1) / align;
        let size_aligned = size_alignments * align;
        let data;
        unsafe {
            let ptr = libc::memalign(align, size_aligned);
            libc::memset(ptr, 0, size_aligned);
            data = slice::from_raw_parts_mut(
                ptr as *mut Color,
                size_aligned / mem::size_of::<Color>()
            );
        }
        ImageAligned { w, h, data, mode: Cell::new(Mode::Blend) }
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

impl Renderer for ImageAligned {
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
        self.data
    }

    /// Return a mutable reference to a slice of colors making up the image
    fn data_mut(&mut self) -> &mut [Color] {
        self.data
    }

    fn sync(&mut self) -> bool {
        true
    }

    fn mode(&self) -> &Cell<Mode> {
        &self.mode
    }
}
