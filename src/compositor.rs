use std::io::{Read, Write};
use std::{mem, slice};

use log::{error, info};

use crate::core::display::Display;
use crate::core::image::{Image, ImageRef};
use crate::core::rect::Rect;

pub struct Compositor {
    // FIXME make these private once possible
    pub displays: Vec<Display>,
    pub hw_cursor: bool,
    pub hw_cursor_initialized: bool,
}

impl Compositor {
    pub fn new(mut displays: Vec<Display>) -> Self {
        //Reading display file is only used to check if GPU cursor is supported
        let mut buf_array = [0; 1];
        let buf: &mut [u8] = &mut buf_array;
        let _ret = displays[0].file.read(buf);

        let mut hw_cursor: bool = false;

        if buf[0] == 1 {
            info!("Hardware cursor detected");
            hw_cursor = true;
        }

        Compositor {
            displays,
            hw_cursor,
            hw_cursor_initialized: false,
        }
    }

    pub fn image(&self) -> &ImageRef<'static> {
        &self.displays[0].image
    }
    pub fn image_mut(&mut self) -> &mut ImageRef<'static> {
        &mut self.displays[0].image
    }
    /// Return the screen rectangle
    pub fn screen_rect(&self) -> Rect {
        self.displays[0].screen_rect()
    }

    /// Resize the inner image buffer. You're responsible for redrawing.
    pub fn resize(&mut self, width: i32, height: i32) {
        //TODO: should other screens be moved after a resize?
        //TODO: support resizing other screens?
        self.displays[0].resize(width, height);
    }

    pub fn redraw_cursor(&mut self, total_redraw: Rect, cursor_rect: Rect, cursor: &mut Image) {
        if self.hw_cursor {
            return;
        }

        for display in self.displays.iter_mut() {
            let rect = total_redraw.intersection(&display.screen_rect());
            if !rect.is_empty() {
                let cursor_intersect = rect.intersection(&cursor_rect);
                if !cursor_intersect.is_empty() {
                    display
                        .roi(&cursor_intersect)
                        .blend(&cursor.roi(
                            &cursor_intersect.offset(-cursor_rect.left(), -cursor_rect.top()),
                        ));
                }
            }
        }
    }

    pub fn sync_rect(&mut self, total_redraw: Rect) {
        // Sync any parts of displays that changed
        for (i, display) in self.displays.iter_mut().enumerate() {
            let display_redraw = total_redraw.intersection(&display.screen_rect());
            if !display_redraw.is_empty() {
                // Keep synced with vesad
                #[allow(dead_code)]
                #[repr(packed)]
                struct SyncRect {
                    x: i32,
                    y: i32,
                    w: i32,
                    h: i32,
                }

                let sync_rect = SyncRect {
                    x: display_redraw.left() - display.x,
                    y: display_redraw.top() - display.y,
                    w: display_redraw.width(),
                    h: display_redraw.height(),
                };

                match display.file.write(unsafe {
                    slice::from_raw_parts(
                        &sync_rect as *const SyncRect as *const u8,
                        mem::size_of::<SyncRect>(),
                    )
                }) {
                    Ok(_) => (),
                    Err(err) => error!("failed to sync display {}: {}", i, err),
                }
            }
        }
    }
}
