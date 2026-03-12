use drm::buffer::{Buffer as _, DrmFourcc};
use drm::control::dumbbuffer::{DumbBuffer, DumbMapping};
use drm::control::{Device as _, framebuffer};
use drm::{ClientCapability, Device as _};
use graphics_ipc::v2::{V2GraphicsHandle, ipc};
use log::error;
use orbclient::{Color, Renderer};
use std::mem;
use std::os::fd::{AsFd, AsRawFd};
use std::{convert::TryInto, io, slice};

use crate::core::{
    image::{ImageRef, ImageRoiMut},
    rect::Rect,
};

pub struct V2DisplayMap {
    fb: framebuffer::Handle,
    buffer: DumbBuffer,
    mapping: DumbMapping<'static>,
}

impl V2DisplayMap {
    pub fn new(display_handle: &V2GraphicsHandle, width: u32, height: u32) -> io::Result<Self> {
        let mut buffer =
            display_handle.create_dumb_buffer((width, height), DrmFourcc::Argb8888, 32)?;
        let fb = display_handle.add_framebuffer(&buffer, 24, 32)?;

        let map = display_handle.map_dumb_buffer(&mut buffer)?;
        let map = unsafe { mem::transmute::<DumbMapping<'_>, DumbMapping<'static>>(map) };

        Ok(Self {
            fb,
            buffer,
            mapping: map,
        })
    }
}

pub struct Display {
    x: i32,
    y: i32,
    scale: i32,
    handle: V2GraphicsHandle,
    map: V2DisplayMap,
}

impl Display {
    pub fn new(x: i32, y: i32, display_handle: V2GraphicsHandle) -> io::Result<Self> {
        let (width, height) = display_handle
            .get_connector(display_handle.first_display().unwrap(), true)
            .unwrap()
            .modes()[0]
            .size();

        let scale = (height as i32 / 1600) + 1;

        let map = V2DisplayMap::new(&display_handle, width as u32, height as u32)?;
        Ok(Self {
            x,
            y,
            scale,
            handle: display_handle,
            map,
        })
    }

    pub fn supports_hw_cursor(&mut self) -> bool {
        self.handle
            .set_client_capability(ClientCapability::CursorPlaneHotspot, true)
            .is_ok()
    }

    pub fn scale(&self) -> i32 {
        self.scale
    }

    fn image_mut(&mut self) -> ImageRef<'_> {
        let width = self.map.buffer.size().0;
        let height = self.map.buffer.size().1;
        let display_slice = unsafe {
            slice::from_raw_parts_mut(
                self.map.mapping.as_mut_ptr() as *mut Color,
                (width * height) as usize,
            )
        };
        ImageRef::from_data(width as i32, height as i32, display_slice)
    }

    pub fn rect(&mut self, rect: &Rect, color: Color) {
        let x = self.x;
        let y = self.y;
        self.image_mut().rect(
            rect.left() - x,
            rect.top() - y,
            rect.width().try_into().unwrap_or(0),
            rect.height().try_into().unwrap_or(0),
            color,
        );
    }

    pub fn resize(&mut self, width: i32, height: i32) {
        match V2DisplayMap::new(&self.handle, width as u32, height as u32) {
            Ok(map) => {
                self.map = map;
            }
            Err(err) => {
                error!("failed to resize display to {}x{}: {}", width, height, err);
            }
        }
    }

    pub fn roi_mut(&mut self, rect: &Rect) -> ImageRoiMut<'_> {
        let x = self.x;
        let y = self.y;
        self.image_mut().roi_mut(&Rect::new(
            rect.left() - x,
            rect.top() - y,
            rect.width(),
            rect.height(),
        ))
    }

    pub fn screen_rect(&self) -> Rect {
        Rect::new(
            self.x,
            self.y,
            self.map.buffer.size().0 as i32,
            self.map.buffer.size().1 as i32,
        )
    }

    pub fn cursor_command(&mut self, cmd: &graphics_ipc::v2::ipc::UpdateCursor) -> io::Result<()> {
        libredox::call::call_wo(
            self.handle.as_fd().as_raw_fd() as usize,
            unsafe { plain::as_bytes(cmd) },
            syscall::CallFlags::empty(),
            &[ipc::UPDATE_CURSOR, 0, 0],
        )?;
        Ok(())
    }

    pub fn sync_rect(&mut self, rect: Rect) -> io::Result<()> {
        let sync_rect = graphics_ipc::v1::Damage {
            x: (rect.left() - self.x) as u32,
            y: (rect.top() - self.y) as u32,
            width: (rect.width()) as u32,
            height: (rect.height()) as u32,
        };

        self.handle
            .update_plane(0, self.map.fb.into(), sync_rect)
            .map(|_| ())
    }
}

impl Drop for Display {
    fn drop(&mut self) {}
}
