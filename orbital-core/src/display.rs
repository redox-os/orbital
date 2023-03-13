use orbclient::{Color, Renderer};
use std::{
    convert::TryInto,
    fs::File,
    io,
    os::unix::io::AsRawFd,
    slice,
};
use log::error;

use crate::{
    image::{ImageRef, ImageRoi},
    rect::Rect,
};

unsafe fn display_fd_map(width: i32, height: i32, display_fd: usize) -> syscall::Result<ImageRef<'static>> {
    let display_ptr = syscall::fmap(display_fd, &syscall::Map {
        offset: 0,
        size: (width * height * 4) as usize,
        flags: syscall::PROT_READ | syscall::PROT_WRITE,
        address: 0,
    })?;
    let display_slice = slice::from_raw_parts_mut(display_ptr as *mut Color, (width * height) as usize);
    Ok(ImageRef::from_data(width, height, display_slice))
}

unsafe fn display_fd_unmap(image: &mut ImageRef) {
    let _ = syscall::funmap(image.data().as_ptr() as usize, (image.width() * image.height() * 4) as usize);
}

pub struct Display {
    pub x: i32,
    pub y: i32,
    pub scale: i32,
    pub file: File,
    pub image: ImageRef<'static>,
}
impl Display {
    pub fn new(x: i32, y: i32, width: i32, height: i32, file: File) -> io::Result<Self> {
        let scale = (height / 1600) + 1;
        let image = unsafe {
            display_fd_map(width, height, file.as_raw_fd() as usize)
                .map_err(|err| {
                    error!("failed to map display: {}", err);
                    io::Error::from_raw_os_error(err.errno)
                })?
        };
        Ok(Self {
            x,
            y,
            scale,
            file,
            image,
        })
    }

    pub fn rect(&mut self, rect: &Rect, color: Color) {
        self.image.rect(
            rect.left() - self.x,
            rect.top() - self.y,
            rect.width().try_into().unwrap_or(0),
            rect.height().try_into().unwrap_or(0),
            color
        );
    }

    pub unsafe fn resize(&mut self, width: i32, height: i32) {
        match display_fd_map(width, height, self.file.as_raw_fd() as usize) {
            Ok(ok) => {
                display_fd_unmap(&mut self.image);
                self.image = ok;
            },
            Err(err) => {
                error!("failed to resize display to {}x{}: {}", width, height, err);
            }
        }
    }

    pub fn roi(&mut self, rect: &Rect) -> ImageRoi {
        self.image.roi(&Rect::new(
            rect.left() - self.x,
            rect.top() - self.y,
            rect.width(),
            rect.height()
        ))
    }

    pub fn screen_rect(&self) -> Rect {
        Rect::new(self.x, self.y, self.image.width(), self.image.height())
    }
}
impl Drop for Display {
    fn drop(&mut self) {
        unsafe {
            display_fd_unmap(&mut self.image);
        }
    }
}