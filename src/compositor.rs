use std::sync::Arc;
use std::time::Instant;

use log::{error, info};

use crate::core::display::{Display, Displays};
use crate::core::image::Image;
use crate::core::rect::Rect;

pub struct Compositor {
    displays: Displays,

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
    pub fn new(displays: Displays) -> Self {
        let mut redraws = Vec::new();
        for display in displays.displays.iter() {
            redraws.push(display.screen_rect());
        }

        let hw_cursor: bool = displays.supports_hw_cursor();
        if hw_cursor {
            info!("Hardware cursor detected");
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
        &self.displays.displays
    }

    /// Return the screen rectangle
    pub fn screen_rect(&self) -> Rect {
        self.displays()[0].screen_rect()
    }

    /// Find the display that a window (`rect`) most overlaps and return it's screen_rect
    pub fn get_screen_rect_for_window(&self, rect: &Rect) -> Rect {
        let mut screen_rect = self.displays()[0].screen_rect();
        let mut max_intersection_area = 0;
        for display in self.displays() {
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
        self.displays.displays[0].resize(&self.displays.display_handle, width, height);

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
                match self.displays.displays[0].move_cursor(&self.displays.display_handle, x, y) {
                    Ok(_) => (),
                    Err(err) => error!("failed to move cursor: {}", err),
                }
            } else {
                match self.displays.displays[0].set_cursor(
                    &self.displays.display_handle,
                    hot_x,
                    hot_y,
                    cursor,
                ) {
                    Ok(_) => (),
                    Err(err) => error!("failed to update cursor: {}", err),
                }

                match self.displays.displays[0].move_cursor(&self.displays.display_handle, x, y) {
                    Ok(_) => (),
                    Err(err) => error!("failed to move cursor: {}", err),
                }
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

            for display in self.displays.displays.iter_mut() {
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
            if self.update_cursor_timer.elapsed().as_millis() > 1000 {
                match self.displays.displays[0].set_cursor(
                    &self.displays.display_handle,
                    self.cursor_hot_x,
                    self.cursor_hot_y,
                    &self.cursor,
                ) {
                    Ok(_) => (),
                    Err(err) => error!("failed to update cursor: {}", err),
                }

                match self.displays.displays[0].move_cursor(
                    &self.displays.display_handle,
                    self.cursor_x,
                    self.cursor_y,
                ) {
                    Ok(_) => (),
                    Err(err) => error!("failed to move cursor: {}", err),
                }

                self.update_cursor_timer = Instant::now();
            }

            return;
        }

        let Some(total_redraw) = total_redraw else {
            return;
        };

        let cursor_rect = self.cursor_rect();

        for display in self.displays.displays.iter_mut() {
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
        for (i, display) in self.displays.displays.iter_mut().enumerate() {
            let display_redraw = total_redraw.intersection(&display.screen_rect());
            if !display_redraw.is_empty() {
                match display.sync_rect(&self.displays.display_handle, display_redraw) {
                    Ok(()) => (),
                    Err(err) => error!("failed to sync display {}: {}", i, err),
                }
            }
        }
    }
}
