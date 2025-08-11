use std::io::{Read, Write};
use std::sync::Arc;
use std::time::Instant;
use std::{mem, slice};

use log::{error, info};

use crate::core::display::Display;
use crate::core::image::Image;
use crate::core::rect::Rect;

#[repr(C, packed)]
struct CursorCommand {
    //header flag that indicates update_cursor or move_cursor
    header: u32,
    x: i32,
    y: i32,
    hot_x: i32,
    hot_y: i32,
    w: i32,
    h: i32,
    cursor_img: [u32; 4096],
}

pub struct Compositor {
    displays: Vec<Display>,

    redraws: Vec<Rect>,

    hw_cursor: bool,
    //QEMU UIs do not grab the pointer in case an absolute pointing device is present
    //and since releasing our gpu cursor makes it disappear, updating it every second fixes it
    update_cursor_timer: Instant,
    cursor: Arc<Image>,
    cursor_x: i32,
    cursor_y: i32,
    cursor_hot_x: i32,
    cursor_hot_y: i32,
}

impl Compositor {
    pub fn new(mut displays: Vec<Display>) -> Self {
        let mut redraws = Vec::new();
        for display in displays.iter() {
            redraws.push(display.screen_rect());
        }

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

            redraws,

            hw_cursor,
            update_cursor_timer: Instant::now(),
            cursor: Arc::new(Image::new(0, 0)),
            cursor_x: 0,
            cursor_y: 0,
            cursor_hot_x: 0,
            cursor_hot_y: 0,
        }
    }

    pub fn displays(&self) -> &[Display] {
        &self.displays
    }

    /// Return the screen rectangle
    pub fn screen_rect(&self) -> Rect {
        self.displays[0].screen_rect()
    }

    /// Find the display that a window (`rect`) most overlaps and return it's screen_rect
    pub fn get_screen_rect_for_window(&self, rect: &Rect) -> Rect {
        let mut screen_rect = self.displays[0].screen_rect();
        let mut max_intersection_area = 0;
        for display in &self.displays {
            let intersect = display.screen_rect().intersection(rect);
            if intersect.area() > max_intersection_area {
                screen_rect = display.screen_rect();
                max_intersection_area = intersect.area();
            }
        }
        screen_rect
    }

    /// Reduce the rect height based on orblauncher bar height
    pub fn get_window_rect_from_screen_rect(&self, screen_rect: &Rect) -> Rect {
        let mut height = screen_rect.height();
        // TODO: This is a hack, orblauncher should
        // talk with orbital to register this value
        height -= 48 * ((height / 1600) + 1);
        Rect::new(
            screen_rect.left(),
            screen_rect.top(),
            screen_rect.width(),
            height,
        )
    }

    /// Resize the inner image buffer.
    pub fn resize(&mut self, width: i32, height: i32) {
        //TODO: should other screens be moved after a resize?
        //TODO: support resizing other screens?
        self.displays[0].resize(width, height);

        self.schedule(self.screen_rect());
    }

    pub fn schedule(&mut self, request: Rect) {
        let mut push = true;
        for rect in self.redraws.iter_mut() {
            //If contained, ignore new redraw request
            let container = rect.container(&request);
            if container.area() <= rect.area() + request.area() {
                *rect = container;
                push = false;
                break;
            }
        }

        if push {
            self.redraws.push(request);
        }
    }

    fn cursor_rect(&self) -> Rect {
        Rect::new(
            self.cursor_x - self.cursor_hot_x,
            self.cursor_y - self.cursor_hot_y,
            self.cursor.width(),
            self.cursor.height(),
        )
    }

    pub fn update_cursor(&mut self, x: i32, y: i32, hot_x: i32, hot_y: i32, cursor: &Arc<Image>) {
        if !self.hw_cursor {
            self.schedule(self.cursor_rect());
        }

        if self.hw_cursor {
            if Arc::ptr_eq(&self.cursor, cursor)
                && self.cursor_hot_x == hot_x
                && self.cursor_hot_y == hot_y
            {
                self.send_cursor_command(&CursorCommand {
                    header: 0,
                    x,
                    y,
                    hot_x: 0,
                    hot_y: 0,
                    w: 0,
                    h: 0,
                    cursor_img: [0; 4096],
                });
            } else {
                self.send_cursor_command(&CursorCommand {
                    header: 1,
                    x,
                    y,
                    hot_x,
                    hot_y,
                    w: cursor.width(),
                    h: cursor.height(),
                    cursor_img: cursor.get_cursor_data(),
                });
            }
        }

        self.cursor_x = x;
        self.cursor_y = y;
        self.cursor_hot_x = hot_x;
        self.cursor_hot_y = hot_y;
        self.cursor = cursor.clone();

        if !self.hw_cursor {
            self.schedule(self.cursor_rect());
        }
    }

    fn send_cursor_command(&mut self, cmd: &CursorCommand) {
        for (i, display) in self.displays.iter_mut().enumerate() {
            match display.file.write(unsafe {
                slice::from_raw_parts(
                    cmd as *const CursorCommand as *const u8,
                    mem::size_of::<CursorCommand>(),
                )
            }) {
                Ok(_) => (),
                Err(err) => error!("failed to sync display {}: {}", i, err),
            }
        }
    }

    pub fn redraw_windows(
        &mut self,
        total_redraw_opt: &mut Option<Rect>,
        draw_windows: impl Fn(&mut Display, Rect),
    ) {
        // go through the list of rectangles pending a redraw and expand the total redraw rectangle
        // to encompass all of them
        for original_rect in self.redraws.drain(..) {
            if !original_rect.is_empty() {
                *total_redraw_opt = Some(
                    total_redraw_opt
                        .unwrap_or(original_rect)
                        .container(&original_rect),
                );
            }

            for display in self.displays.iter_mut() {
                let rect = original_rect.intersection(&display.screen_rect());
                if rect.is_empty() {
                    continue;
                }

                draw_windows(display, rect);
            }
        }
    }

    pub fn redraw_cursor(&mut self, total_redraw: Option<Rect>) {
        if self.hw_cursor {
            if self.hw_cursor && self.update_cursor_timer.elapsed().as_millis() > 1000 {
                self.send_cursor_command(&CursorCommand {
                    header: 1,
                    x: self.cursor_x,
                    y: self.cursor_y,
                    hot_x: self.cursor_hot_x,
                    hot_y: self.cursor_hot_y,
                    w: self.cursor.width(),
                    h: self.cursor.height(),
                    cursor_img: self.cursor.get_cursor_data(),
                });
                self.update_cursor_timer = Instant::now();
            }

            return;
        }

        let Some(total_redraw) = total_redraw else {
            return;
        };

        let cursor_rect = self.cursor_rect();

        for display in self.displays.iter_mut() {
            let rect = total_redraw.intersection(&display.screen_rect());
            if !rect.is_empty() {
                let cursor_intersect = rect.intersection(&cursor_rect);
                if !cursor_intersect.is_empty() {
                    display.roi_mut(&cursor_intersect).blend(
                        &self
                            .cursor
                            .roi(&cursor_intersect.offset(-cursor_rect.left(), -cursor_rect.top())),
                    );
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
