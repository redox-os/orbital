use drm::buffer::{Buffer as _, DrmFourcc};
use drm::control::connector::{self, State};
use drm::control::dumbbuffer::{DumbBuffer, DumbMapping};
use drm::control::{ClipRect, Device as _, crtc, framebuffer};
use drm::{ClientCapability, Device as _, DriverCapability};
use graphics_ipc::{CpuBackedBuffer, V2GraphicsHandle};
use log::{debug, error};
use orbclient::{Color, Renderer};
use std::mem;
use std::{convert::TryInto, io, slice};

use crate::core::{
    image::{Image, ImageRef, ImageRoiMut},
    rect::Rect,
};

pub struct V2DisplayMap {
    fb: framebuffer::Handle,
    connector: connector::Handle,
    crtc: crtc::Handle,
    buffer: CpuBackedBuffer,
}

impl V2DisplayMap {
    fn new(display_handle: &V2GraphicsHandle) -> io::Result<Self> {
        let connector = display_handle.first_display().unwrap();
        let connector_info = display_handle.get_connector(connector, true).unwrap();

        let mode = connector_info.modes()[0];
        let (width, height) = mode.size();

        // FIXME do something smarter that avoids conflicts
        let crtc = display_handle.resource_handles().unwrap().filter_crtcs(
            display_handle
                .get_encoder(connector_info.encoders()[0])
                .unwrap()
                .possible_crtcs(),
        )[0];

        let buffer = CpuBackedBuffer::new(
            display_handle,
            (width.into(), height.into()),
            DrmFourcc::Argb8888,
            32,
        )?;
        let fb = display_handle.add_framebuffer(buffer.buffer(), 32, 32)?;

        display_handle.set_crtc(crtc, Some(fb), (0, 0), &[connector], Some(mode))?;

        Ok(Self {
            fb,
            connector,
            crtc,
            buffer,
        })
    }

    fn resize_if_necessary(&mut self, display_handle: &V2GraphicsHandle) -> io::Result<bool> {
        let connector_info = display_handle.get_connector(self.connector, false).unwrap();

        let mode = connector_info.modes()[0];
        let (width, height) = mode.size();

        if (u32::from(width), u32::from(height)) == self.buffer.buffer().size() {
            return Ok(false);
        }

        let new_buffer = CpuBackedBuffer::new(
            display_handle,
            (u32::from(mode.size().0), u32::from(mode.size().1)),
            DrmFourcc::Argb8888,
            32,
        )?;
        let new_fb = display_handle.add_framebuffer(new_buffer.buffer(), 32, 32)?;

        let old_buffer = mem::replace(&mut self.buffer, new_buffer);
        old_buffer.destroy(display_handle)?;

        let old_fb = mem::replace(&mut self.fb, new_fb);
        display_handle.set_crtc(
            self.crtc,
            Some(self.fb),
            (0, 0),
            &[self.connector],
            Some(mode),
        )?;
        let _ = display_handle.destroy_framebuffer(old_fb);

        Ok(true)
    }

    fn image_mut(&mut self) -> ImageRef<'_> {
        let (width, height) = self.buffer.buffer().size();
        let display_slice = unsafe {
            slice::from_raw_parts_mut(
                self.buffer.shadow_buf().as_mut_ptr() as *mut Color,
                (width * height) as usize,
            )
        };
        ImageRef::from_data(width as i32, height as i32, display_slice)
    }
}

struct CursorMap {
    buffer: DumbBuffer,
    mapping: DumbMapping<'static>,
}

impl CursorMap {
    fn new(display_handle: &V2GraphicsHandle, width: u32, height: u32) -> io::Result<Self> {
        let mut buffer =
            display_handle.create_dumb_buffer((width, height), DrmFourcc::Argb8888, 32)?;

        let map = display_handle.map_dumb_buffer(&mut buffer)?;
        let map = unsafe { mem::transmute::<DumbMapping<'_>, DumbMapping<'static>>(map) };

        Ok(Self {
            buffer,
            mapping: map,
        })
    }

    fn image_mut(&mut self) -> ImageRef<'_> {
        let (width, height) = self.buffer.size();
        let display_slice = unsafe {
            slice::from_raw_parts_mut(
                self.mapping.as_mut_ptr() as *mut Color,
                (width * height) as usize,
            )
        };
        ImageRef::from_data(width as i32, height as i32, display_slice)
    }
}

pub struct Displays {
    pub display_handle: V2GraphicsHandle,
    supports_hw_cursor: bool,
    pub displays: Vec<Display>,
}

impl Displays {
    pub fn new(display_handle: V2GraphicsHandle) -> io::Result<Self> {
        display_handle.set_client_capability(ClientCapability::CursorPlaneHotspot, true)?;

        let cursor_width = display_handle.get_driver_capability(DriverCapability::CursorHeight);
        let cursor_height = display_handle.get_driver_capability(DriverCapability::CursorWidth);
        // We only support 64x64 cursors currently
        let hw_cursor = cursor_width.ok().zip(cursor_height.ok());

        let mut displays: Vec<Display> = vec![];
        for (i, &connector) in display_handle
            .resource_handles()
            .unwrap()
            .connectors()
            .iter()
            .enumerate()
        {
            if display_handle.get_connector(connector, true)?.state() == State::Connected {
                let x = if let Some(last) = displays.last() {
                    last.screen_rect().right()
                } else {
                    0
                };
                let y = 0;

                displays.push(Display::new(x, y, &display_handle, i, hw_cursor)?);
            }
        }

        Ok(Displays {
            display_handle,
            supports_hw_cursor: hw_cursor.is_some(),
            displays,
        })
    }

    pub fn supports_hw_cursor(&self) -> bool {
        self.supports_hw_cursor
    }
}

pub struct Display {
    x: i32,
    y: i32,
    scale: i32,
    map: V2DisplayMap,
    cursor_map: Option<CursorMap>,
}

impl Display {
    pub fn new(
        x: i32,
        y: i32,
        display_handle: &V2GraphicsHandle,
        connector_id: usize,
        hw_cursor: Option<(u64, u64)>,
    ) -> io::Result<Self> {
        let connector = display_handle.get_connector(
            display_handle.resource_handles().unwrap().connectors()[connector_id],
            true,
        )?;
        let (width, height) = connector.modes()[0].size();

        debug!("Display at {}, {}, {}, {}", x, y, width, height);

        let scale = (height as i32 / 1600) + 1;

        let map = V2DisplayMap::new(display_handle)?;
        let cursor_map = hw_cursor
            .map(|(width, height)| CursorMap::new(&display_handle, width as u32, height as u32))
            .transpose()?;
        Ok(Self {
            x,
            y,
            scale,
            map,
            cursor_map,
        })
    }

    pub fn scale(&self) -> i32 {
        self.scale
    }

    pub fn rect(&mut self, rect: &Rect, color: Color) {
        let x = self.x;
        let y = self.y;
        self.map.image_mut().rect(
            rect.left() - x,
            rect.top() - y,
            rect.width().try_into().unwrap_or(0),
            rect.height().try_into().unwrap_or(0),
            color,
        );
    }

    pub fn resize_if_necessary(&mut self, display_handle: &V2GraphicsHandle) -> bool {
        match self.map.resize_if_necessary(display_handle) {
            Ok(resized) => resized,
            Err(err) => {
                error!("failed to resize display: {err}");
                false
            }
        }
    }

    pub fn roi_mut(&mut self, rect: &Rect) -> ImageRoiMut<'_> {
        let x = self.x;
        let y = self.y;
        self.map.image_mut().roi_mut(&Rect::new(
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
            self.map.buffer.buffer().size().0 as i32,
            self.map.buffer.buffer().size().1 as i32,
        )
    }

    pub fn move_cursor(
        &mut self,
        display_handle: &V2GraphicsHandle,
        x: i32,
        y: i32,
    ) -> io::Result<()> {
        #[allow(deprecated)]
        display_handle.move_cursor(self.map.crtc, (x, y))
    }

    pub fn set_cursor(
        &mut self,
        display_handle: &V2GraphicsHandle,
        hot_x: i32,
        hot_y: i32,
        image: &Image,
    ) -> io::Result<()> {
        let cursor_map = self.cursor_map.as_mut().unwrap();

        let mut cursor_image = cursor_map.image_mut();
        cursor_image.set(Color::rgba(0, 0, 0, 0));
        cursor_image.image(
            0,
            0,
            image.width() as u32,
            image.height() as u32,
            image.data(),
        );

        #[allow(deprecated)]
        display_handle.set_cursor2(
            self.map.crtc,
            Some(&self.cursor_map.as_ref().unwrap().buffer),
            (hot_x, hot_y),
        )
    }

    pub fn sync_rect(&mut self, display_handle: &V2GraphicsHandle, rect: Rect) -> io::Result<()> {
        let x1 = (rect.left() - self.x) as usize;
        let y1 = (rect.top() - self.y) as usize;
        let x2 = (rect.right() - self.x) as usize;
        let y2 = (rect.bottom() - self.y) as usize;

        let pitch = self.map.buffer.buffer().pitch() as usize;
        self.map
            .buffer
            .sync_range((y1..y2).map(|row| row * pitch + x1 * 4..row * pitch + x2 * 4));

        display_handle
            .dirty_framebuffer(
                self.map.fb,
                &[ClipRect::new(x1 as u16, y1 as u16, x2 as u16, y2 as u16)],
            )
            .map(|_| ())
    }
}

impl Drop for Display {
    fn drop(&mut self) {}
}
