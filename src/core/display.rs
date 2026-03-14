use drm::buffer::{Buffer as _, DrmFourcc};
use drm::control::connector::State;
use drm::control::dumbbuffer::{DumbBuffer, DumbMapping};
use drm::control::{Device as _, crtc, framebuffer};
use drm::{ClientCapability, Device as _, DriverCapability};
use graphics_ipc::v2::V2GraphicsHandle;
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

    fn image_mut(&mut self) -> ImageRef<'_> {
        let width = self.buffer.size().0;
        let height = self.buffer.size().1;
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
                    last.screen_rect().left() + last.screen_rect().width()
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
    connector: usize,
    crtc: crtc::Handle,
    x: i32,
    y: i32,
    scale: i32,
    map: V2DisplayMap,
    cursor_map: Option<V2DisplayMap>,
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

        // FIXME do something smarter that avoids conflicts
        let crtc = display_handle.resource_handles()?.filter_crtcs(
            display_handle
                .get_encoder(connector.encoders()[0])?
                .possible_crtcs(),
        )[0];

        debug!("Display at {}, {}, {}, {}", x, y, width, height);

        let scale = (height as i32 / 1600) + 1;

        let map = V2DisplayMap::new(&display_handle, width as u32, height as u32)?;
        let cursor_map = hw_cursor
            .map(|(width, height)| V2DisplayMap::new(&display_handle, width as u32, height as u32))
            .transpose()?;
        Ok(Self {
            connector: connector_id,
            crtc,
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

    fn image_mut(&mut self) -> ImageRef<'_> {
        self.map.image_mut()
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

    pub fn resize(&mut self, display_handle: &V2GraphicsHandle, width: i32, height: i32) {
        match V2DisplayMap::new(display_handle, width as u32, height as u32) {
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

    pub fn move_cursor(
        &mut self,
        display_handle: &V2GraphicsHandle,
        x: i32,
        y: i32,
    ) -> io::Result<()> {
        #[allow(deprecated)]
        display_handle.move_cursor(self.crtc, (x, y))
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
            self.crtc,
            Some(&self.cursor_map.as_ref().unwrap().buffer),
            (hot_x, hot_y),
        )
    }

    pub fn sync_rect(&mut self, display_handle: &V2GraphicsHandle, rect: Rect) -> io::Result<()> {
        let sync_rect = graphics_ipc::v2::Damage {
            x: (rect.left() - self.x) as u32,
            y: (rect.top() - self.y) as u32,
            width: (rect.width()) as u32,
            height: (rect.height()) as u32,
        };

        display_handle
            .update_plane(self.connector, self.map.fb.into(), sync_rect)
            .map(|_| ())
    }
}

impl Drop for Display {
    fn drop(&mut self) {}
}
