use libredox::{call::MmapArgs, flag};
use log::error;
use orbclient::{Color, Renderer};
use std::io::{Read, Write};
use std::{convert::TryInto, fs::File, io, os::unix::io::AsRawFd, slice};

use crate::core::{
    image::{ImageRef, ImageRoiMut},
    rect::Rect,
};

fn display_fd_map(
    width: i32,
    height: i32,
    display_fd: usize,
) -> libredox::error::Result<ImageRef<'static>> {
    unsafe {
        let display_ptr = libredox::call::mmap(MmapArgs {
            fd: display_fd,
            offset: 0,
            length: (width * height * 4) as usize,
            prot: flag::PROT_READ | flag::PROT_WRITE,
            flags: flag::MAP_SHARED,
            addr: core::ptr::null_mut(),
        })?;
        let display_slice =
            slice::from_raw_parts_mut(display_ptr as *mut Color, (width * height) as usize);
        Ok(ImageRef::from_data(width, height, display_slice))
    }
}

fn display_fd_unmap(image: &mut ImageRef) {
    unsafe {
        let _ = libredox::call::munmap(
            image.data().as_ptr() as *mut (),
            (image.width() * image.height() * 4) as usize,
        );
    }
}

pub struct Display {
    x: i32,
    y: i32,
    scale: i32,
    file: File,
    image: ImageRef<'static>,
}

impl Display {
    pub fn new(x: i32, y: i32, width: i32, height: i32, file: File) -> io::Result<Self> {
        let scale = (height / 1600) + 1;
        let image = display_fd_map(width, height, file.as_raw_fd() as usize).map_err(|err| {
            error!("failed to map display: {}", err);
            io::Error::from_raw_os_error(err.errno())
        })?;
        Ok(Self {
            x,
            y,
            scale,
            file,
            image,
        })
    }

    pub fn supports_hw_cursor(&mut self) -> bool {
        let mut buf_array = [0; 1];
        let buf: &mut [u8] = &mut buf_array;
        let _ret = self.file.read(buf);

        buf[0] == 1
    }

    pub fn scale(&self) -> i32 {
        self.scale
    }

    pub fn rect(&mut self, rect: &Rect, color: Color) {
        self.image.rect(
            rect.left() - self.x,
            rect.top() - self.y,
            rect.width().try_into().unwrap_or(0),
            rect.height().try_into().unwrap_or(0),
            color,
        );
    }

    pub fn resize(&mut self, width: i32, height: i32) {
        match display_fd_map(width, height, self.file.as_raw_fd() as usize) {
            Ok(ok) => {
                display_fd_unmap(&mut self.image);
                self.image = ok;
            }
            Err(err) => {
                error!("failed to resize display to {}x{}: {}", width, height, err);
            }
        }
    }

    pub fn roi_mut(&mut self, rect: &Rect) -> ImageRoiMut<'_> {
        self.image.roi_mut(&Rect::new(
            rect.left() - self.x,
            rect.top() - self.y,
            rect.width(),
            rect.height(),
        ))
    }

    pub fn screen_rect(&self) -> Rect {
        Rect::new(self.x, self.y, self.image.width(), self.image.height())
    }

    pub fn cursor_command(&mut self, cmd: &graphics_ipc::v1::CursorDamage) -> io::Result<()> {
        self.file
            .write(unsafe {
                slice::from_raw_parts(
                    cmd as *const graphics_ipc::v1::CursorDamage as *const u8,
                    size_of::<graphics_ipc::v1::CursorDamage>(),
                )
            })
            .map(|_| ())
    }

    pub fn sync_rect(&mut self, rect: Rect) -> io::Result<()> {
        let sync_rect = graphics_ipc::v1::Damage {
            x: (rect.left() - self.x) as u32,
            y: (rect.top() - self.y) as u32,
            width: (rect.width()) as u32,
            height: (rect.height()) as u32,
        };

        self.file
            .write(unsafe {
                slice::from_raw_parts(
                    &sync_rect as *const graphics_ipc::v1::Damage as *const u8,
                    size_of::<graphics_ipc::v1::Damage>(),
                )
            })
            .map(|_| ())
    }
}

impl Drop for Display {
    fn drop(&mut self) {
        display_fd_unmap(&mut self.image);
    }
}
