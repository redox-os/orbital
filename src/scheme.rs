use std::io::Read;
use std::rc::Rc;
use std::{
    cmp,
    collections::{BTreeMap, VecDeque},
    fs,
    io::{self, Write},
    mem, slice, str,
    time::Instant,
};

use log::{error, info, warn};
use orbclient::{
    self, ButtonEvent, ClipboardEvent, Color, Event, EventOption, FocusEvent, HoverEvent, KeyEvent,
    MouseEvent, MouseRelativeEvent, MoveEvent, QuitEvent, Renderer, ResizeEvent, ScreenEvent,
    TextInputEvent,
};
use redox_scheme::Response;
use syscall::error::{Error, Result, EBADF};
use syscall::EVENT_READ;

use crate::config::Config;
use crate::core::image::ImageRef;
use crate::core::{display::Display, image::Image, rect::Rect, Orbital, Properties};
use crate::scheme::TilePosition::{BottomHalf, FullScreen, LeftHalf, RightHalf, TopHalf};
use crate::window::{Window, WindowZOrder};

fn schedule(redraws: &mut Vec<Rect>, request: Rect) {
    let mut push = true;
    for rect in redraws.iter_mut() {
        //If contained, ignore new redraw request
        let container = rect.container(&request);
        if container.area() <= rect.area() + request.area() {
            *rect = container;
            push = false;
            break;
        }
    }

    if push {
        redraws.push(request);
    }
}

#[derive(Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
enum CursorKind {
    None,
    LeftPtr,
    BottomLeftCorner,
    BottomRightCorner,
    BottomSide,
    LeftSide,
    RightSide,
}

enum DragMode {
    None,
    Title(usize, i32, i32),
    LeftBorder(usize, i32, i32),
    RightBorder(usize, i32),
    BottomBorder(usize, i32),
    BottomLeftBorder(usize, i32, i32, i32),
    BottomRightBorder(usize, i32, i32),
}

enum Volume {
    Down,
    Up,
    Toggle,
}

#[derive(Debug)]
enum TilePosition {
    LeftHalf,
    TopHalf,
    RightHalf,
    BottomHalf,
    FullScreen,
}

const GRID_SIZE: i32 = 16;

const SHIFT_LEFT_MODIFIER: u8 = 1 << 0;
const SHIFT_RIGHT_MODIFIER: u8 = 1 << 1;
const SHIFT_ANY_MODIFIER: u8 = 1 << 2;
const CONTROL_MODIFIER: u8 = 1 << 3;
const ALT_MODIFIER: u8 = 1 << 4;
const ALT_GR_MODIFIER: u8 = 1 << 5;
const ALT_ANY_MODIFIER: u8 = 1 << 6;
const SUPER_MODIFIER: u8 = 1 << 7;

pub struct OrbitalScheme {
    displays: Vec<Display>,
    hw_cursor: bool,
    hw_cursor_initialized: bool,

    window_max: Image,
    window_max_unfocused: Image,
    window_close: Image,
    window_close_unfocused: Image,
    cursors: BTreeMap<CursorKind, Image>,
    cursor_i: CursorKind,
    cursor_x: i32,
    cursor_y: i32,
    cursor_left: bool,
    cursor_middle: bool,
    cursor_right: bool,
    dragging: DragMode,
    modifier_state: u8,
    volume_value: i32,
    volume_toggle: i32,
    next_id: isize,
    hover: Option<usize>,
    order: VecDeque<usize>,
    zbuffer: Vec<(usize, WindowZOrder, usize)>,
    windows: BTreeMap<usize, Window>,
    redraws: Vec<Rect>,
    font: orbfont::Font,
    clipboard: Vec<u8>,
    scale: i32,
    config: Rc<Config>,
    // Is the user currently switching windows with win-tab
    // Set true when win-tab is pressed, set false when win is released.
    // While it is true, redraw() calls draw_window_list()
    win_tabbing: bool,
    volume_osd: bool,
    shortcuts_osd: bool,
    popup_rect: Rect,
    update_cursor_timer: Instant, //QEMU UIs do not grab the pointer in case an absolute pointing device is present
                                  //and since releasing our gpu cursor makes it disappear, updating it every second fixes it
}

impl OrbitalScheme {
    pub(crate) fn new(
        mut displays: Vec<Display>,
        config: Rc<Config>,
    ) -> Result<OrbitalScheme, String> {
        let mut redraws = Vec::new();
        let mut scale = 1;
        for display in displays.iter() {
            redraws.push(display.screen_rect());
            scale = cmp::max(scale, display.scale);
        }

        let mut cursors = BTreeMap::new();
        cursors.insert(CursorKind::None, Image::new(0, 0));
        cursors.insert(
            CursorKind::LeftPtr,
            Image::from_path_scale(&config.cursor, scale).unwrap_or(Image::new(0, 0)),
        );
        cursors.insert(
            CursorKind::BottomLeftCorner,
            Image::from_path_scale(&config.bottom_left_corner, scale).unwrap_or(Image::new(0, 0)),
        );
        cursors.insert(
            CursorKind::BottomRightCorner,
            Image::from_path_scale(&config.bottom_right_corner, scale).unwrap_or(Image::new(0, 0)),
        );
        cursors.insert(
            CursorKind::BottomSide,
            Image::from_path_scale(&config.bottom_side, scale).unwrap_or(Image::new(0, 0)),
        );
        cursors.insert(
            CursorKind::LeftSide,
            Image::from_path_scale(&config.left_side, scale).unwrap_or(Image::new(0, 0)),
        );
        cursors.insert(
            CursorKind::RightSide,
            Image::from_path_scale(&config.right_side, scale).unwrap_or(Image::new(0, 0)),
        );

        let font = orbfont::Font::find(Some("Sans"), None, None)?;

        //Reading display file is only used to check if GPU cursor is supported
        let mut buf_array = [0; 1];
        let buf: &mut [u8] = &mut buf_array;
        let _ret = displays[0].file.read(buf);

        let mut hw_cursor: bool = false;

        if buf[0] == 1 {
            info!("Hardware cursor detected");
            hw_cursor = true;
        }

        Ok(OrbitalScheme {
            displays,
            hw_cursor,
            hw_cursor_initialized: false,

            window_max: Image::from_path_scale(&config.window_max, scale)
                .unwrap_or(Image::new(0, 0)),
            window_max_unfocused: Image::from_path_scale(&config.window_max_unfocused, scale)
                .unwrap_or(Image::new(0, 0)),
            window_close: Image::from_path_scale(&config.window_close, scale)
                .unwrap_or(Image::new(0, 0)),
            window_close_unfocused: Image::from_path_scale(&config.window_close_unfocused, scale)
                .unwrap_or(Image::new(0, 0)),
            cursors,
            cursor_i: CursorKind::LeftPtr,
            cursor_x: 0,
            cursor_y: 0,
            cursor_left: false,
            cursor_middle: false,
            cursor_right: false,
            dragging: DragMode::None,
            modifier_state: 0,
            volume_value: 0,
            volume_toggle: 0,
            next_id: 1,
            hover: None,
            order: VecDeque::new(),
            zbuffer: Vec::new(),
            windows: BTreeMap::new(),
            redraws,
            font,
            clipboard: Vec::new(),
            scale,
            config: Rc::clone(&config),
            win_tabbing: false,
            volume_osd: false,
            shortcuts_osd: false,
            popup_rect: Rect::default(),
            update_cursor_timer: Instant::now(),
        })
    }

    //TODO: replace these adapter functions
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

    fn cursor_rect(&self) -> Rect {
        let cursor = &self.cursors[&self.cursor_i];
        let (off_x, off_y) = match self.cursor_i {
            CursorKind::None => (0, 0),
            CursorKind::LeftPtr => (0, 0),
            CursorKind::BottomLeftCorner => (0, -cursor.height()),
            CursorKind::BottomRightCorner => (-cursor.width(), -cursor.height()),
            CursorKind::BottomSide => (-cursor.width() / 2, -cursor.height()),
            CursorKind::LeftSide => (0, -cursor.height() / 2),
            CursorKind::RightSide => (-cursor.width(), -cursor.height() / 2),
        };
        Rect::new(
            self.cursor_x + off_x,
            self.cursor_y + off_y,
            cursor.width(),
            cursor.height(),
        )
    }

    fn focus(&mut self, id: usize, focused: bool) {
        if let Some(window) = self.windows.get_mut(&id) {
            schedule(&mut self.redraws, window.title_rect());
            schedule(&mut self.redraws, window.rect());
            window.event(FocusEvent { focused }.to_event());
        }
    }

    fn rezbuffer(&mut self) {
        self.zbuffer.clear();

        for (i, id) in self.order.iter().enumerate() {
            if let Some(window) = self.windows.get(id) {
                self.zbuffer.push((*id, window.zorder, i));
            }
        }

        self.zbuffer.sort_by(|a, b| b.1.cmp(&a.1));
    }

    //TODO: update cursor in more places to ensure consistency:
    // - Window resizes
    // - Window sets cursor on/off
    // - Window moves
    fn update_cursor(&mut self, x: i32, y: i32, kind: CursorKind) {
        if kind != self.cursor_i {
            let cursor_rect = self.cursor_rect();
            schedule(&mut self.redraws, cursor_rect);

            self.cursor_i = kind;

            let cursor_rect = self.cursor_rect();
            schedule(&mut self.redraws, cursor_rect);
        }

        // Update saved mouse information
        if x != self.cursor_x || y != self.cursor_y {
            let cursor_rect = self.cursor_rect();
            schedule(&mut self.redraws, cursor_rect);

            self.cursor_x = x;
            self.cursor_y = y;

            let cursor_rect = self.cursor_rect();
            schedule(&mut self.redraws, cursor_rect);
        }
    }
}

impl OrbitalScheme {
    /// Return true if a packet should be delayed until a display event
    pub fn should_delay(&mut self, id: usize) -> bool {
        self.windows
            .get(&id)
            .map(|window| !window.asynchronous)
            .unwrap_or(true)
    }

    /// Called after a batch of scheme events have been handled
    pub fn handle_scheme_after(&mut self, orb: &mut Orbital) -> io::Result<()> {
        for (id, window) in self.windows.iter_mut() {
            if !window.events.is_empty() {
                if !window.notified_read || window.asynchronous {
                    window.notified_read = true;
                    orb.scheme_write(Response::post_fevent(*id, EVENT_READ.bits()))?;
                }
            } else {
                window.notified_read = false;
            }
        }

        // redrawn by handle_after

        Ok(())
    }

    /// Callback to handle events over the input handle
    pub fn handle_input(&mut self, orb: &mut Orbital, events: &mut [Event]) -> io::Result<()> {
        self.input_event(events)?;

        self.handle_scheme_after(orb)
    }

    /// Called after a batch of any events have been handled
    pub fn handle_after(&mut self) -> io::Result<()> {
        self.redraw();
        Ok(())
    }

    /// Called when a new window is requested by the scheme.
    /// Return a window ID that will be used to identify it later.
    #[allow(clippy::too_many_arguments)]
    pub fn handle_window_new(
        &mut self,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        parts: &str,
        title: String,
    ) -> Result<usize> {
        self.window_new(x, y, width, height, parts, title)
    }

    /// Called when the scheme is read for events
    pub fn handle_window_read(&mut self, id: usize, buf: &mut [Event]) -> Result<usize> {
        let window = self.windows.get_mut(&id).ok_or(Error::new(EBADF))?;
        Ok(window.read(buf))
    }

    /// Called when the window asks to set async
    pub fn handle_window_async(&mut self, id: usize, is_async: bool) -> Result<()> {
        let window = self.windows.get_mut(&id).ok_or(Error::new(EBADF))?;
        window.asynchronous = is_async;
        Ok(())
    }

    /// Called when the window asks to be dragged
    pub fn handle_window_drag(&mut self, id: usize /*TODO: resize sides */) -> Result<()> {
        let _window = self.windows.get_mut(&id).ok_or(Error::new(EBADF))?;
        if self.cursor_left {
            self.dragging = DragMode::Title(id, self.cursor_x, self.cursor_y);
        }
        Ok(())
    }

    /// Called when the window asks to set mouse cursor visibility
    pub fn handle_window_mouse_cursor(&mut self, id: usize, visible: bool) -> Result<()> {
        let window = self.windows.get_mut(&id).ok_or(Error::new(EBADF))?;
        window.mouse_cursor = visible;
        Ok(())
    }

    /// Called when the window asks to set mouse grabbing
    pub fn handle_window_mouse_grab(&mut self, id: usize, grab: bool) -> Result<()> {
        let window = self.windows.get_mut(&id).ok_or(Error::new(EBADF))?;
        window.mouse_grab = grab;
        Ok(())
    }

    /// Called when the window asks to set mouse relative mode
    pub fn handle_window_mouse_relative(&mut self, id: usize, relative: bool) -> Result<()> {
        let window = self.windows.get_mut(&id).ok_or(Error::new(EBADF))?;
        window.mouse_relative = relative;
        Ok(())
    }

    /// Called when the window asks to be repositioned
    pub fn handle_window_position(
        &mut self,
        id: usize,
        x: Option<i32>,
        y: Option<i32>,
    ) -> Result<()> {
        let window = self.windows.get_mut(&id).ok_or(Error::new(EBADF))?;
        schedule(&mut self.redraws, window.title_rect());
        schedule(&mut self.redraws, window.rect());

        window.x = x.unwrap_or(window.x);
        window.y = y.unwrap_or(window.y);

        schedule(&mut self.redraws, window.title_rect());
        schedule(&mut self.redraws, window.rect());

        Ok(())
    }

    /// Called when the window asks to be resized
    pub fn handle_window_resize(
        &mut self,
        id: usize,
        w: Option<i32>,
        h: Option<i32>,
    ) -> Result<()> {
        let window = self.windows.get_mut(&id).ok_or(Error::new(EBADF))?;
        schedule(&mut self.redraws, window.title_rect());
        schedule(&mut self.redraws, window.rect());

        let w = w.unwrap_or(window.width());
        let h = h.unwrap_or(window.height());

        window.set_size(w, h);

        schedule(&mut self.redraws, window.title_rect());
        schedule(&mut self.redraws, window.rect());

        Ok(())
    }

    /// Called when the window wants to set a flag
    pub fn handle_window_set_flag(&mut self, id: usize, flag: char, value: bool) -> Result<()> {
        let window = self.windows.get_mut(&id).ok_or(Error::new(EBADF))?;

        // Handle maximized flag custom
        if flag == crate::window::ORBITAL_FLAG_MAXIMIZED {
            let toggle_tile = if value {
                window.restore = None;
                true
            } else {
                window.restore.is_some()
            };
            if toggle_tile {
                self.tile_window(Some(&id), TilePosition::FullScreen);
            }
        } else {
            // Setting flag may change visibility, make sure to queue redraws both before and after
            schedule(&mut self.redraws, window.title_rect());
            schedule(&mut self.redraws, window.rect());

            window.set_flag(flag, value);

            // Setting flag may change visibility, make sure to queue redraws both before and after
            schedule(&mut self.redraws, window.title_rect());
            schedule(&mut self.redraws, window.rect());
        }

        Ok(())
    }

    /// Called when the window asks to change title
    pub fn handle_window_title(&mut self, id: usize, title: String) -> Result<()> {
        let window = self.windows.get_mut(&id).ok_or(Error::new(EBADF))?;
        window.title = title;
        window.render_title(&self.font);

        schedule(&mut self.redraws, window.title_rect());

        Ok(())
    }

    /// Called by fevent to clear notified status, assuming you're sending edge-triggered notifications
    /// TODO: Abstract event system away completely.
    pub fn handle_window_clear_notified(&mut self, id: usize) -> Result<()> {
        let window = self.windows.get_mut(&id).ok_or(Error::new(EBADF))?;
        window.notified_read = false;
        Ok(())
    }

    /// Return a reference the window's image that will be mapped in the scheme's fmap function
    pub fn handle_window_map(&mut self, id: usize, create_new: bool) -> Result<&mut [Color]> {
        let window = self.windows.get_mut(&id).ok_or(Error::new(EBADF))?;
        if create_new {
            window.maps += 1;
        }
        Ok(window.map())
    }

    /// Free a reference to the window's image, for use by funmap
    pub fn handle_window_unmap(&mut self, id: usize) -> Result<()> {
        let window = self.windows.get_mut(&id).ok_or(Error::new(EBADF))?;
        if window.maps > 0 {
            window.maps -= 1;
        } else {
            warn!("attempted unmap when there are no mappings");
        }
        Ok(())
    }

    /// Called to get window properties
    pub fn handle_window_properties(&mut self, id: usize) -> Result<Properties> {
        let window = self.windows.get(&id).ok_or(Error::new(EBADF))?;
        Ok(window.properties())
    }

    /// Called to flush a window. It's usually a good idea to redraw here.
    pub fn handle_window_sync(&mut self, id: usize) -> Result<()> {
        let window = self.windows.get(&id).ok_or(Error::new(EBADF))?;
        schedule(&mut self.redraws, window.rect());
        Ok(())
    }

    /// Called when a window should be closed
    pub fn handle_window_close(&mut self, id: usize) {
        // Unfocus current front window
        if let Some(id) = self.order.front() {
            self.focus(*id, false);
        }

        self.order.retain(|&e| e != id);

        if let Some(window) = self.windows.remove(&id) {
            schedule(&mut self.redraws, window.title_rect());
            schedule(&mut self.redraws, window.rect());
        }

        // Focus current front window
        if let Some(id) = self.order.front() {
            self.focus(*id, true);
        }

        // Ensure mouse cursor is correct
        let event = MouseEvent {
            x: self.cursor_x,
            y: self.cursor_y,
        };
        self.mouse_event(event);
    }
    /// Create a clipboard from a window
    pub fn handle_clipboard_new(&mut self, id: usize) -> Result<usize> {
        //TODO: implement better clipboard mechanism
        let window = self.windows.get_mut(&id).ok_or(Error::new(EBADF))?;
        window.clipboard_seek = 0;
        Ok(id)
    }

    /// Read window clipboard
    pub fn handle_clipboard_read(&mut self, id: usize, buf: &mut [u8]) -> Result<usize> {
        //TODO: implement better clipboard mechanism
        let window = self.windows.get_mut(&id).ok_or(Error::new(EBADF))?;
        let mut i = 0;
        while i < buf.len() && window.clipboard_seek < self.clipboard.len() {
            buf[i] = self.clipboard[i];
            i += 1;
            window.clipboard_seek += 1;
        }
        Ok(i)
    }

    /// Write window clipboard
    pub fn handle_clipboard_write(&mut self, id: usize, buf: &[u8]) -> Result<usize> {
        //TODO: implement better clipboard mechanism
        let window = self.windows.get_mut(&id).ok_or(Error::new(EBADF))?;
        let mut i = 0;
        self.clipboard.truncate(window.clipboard_seek);
        while i < buf.len() {
            self.clipboard.push(buf[i]);
            i += 1;
            window.clipboard_seek += 1;
        }
        Ok(i)
    }

    /// Close the window's clipboard access
    pub fn handle_clipboard_close(&mut self, _id: usize) {
        //TODO: implement better clipboard mechanism
    }

    fn redraw(&mut self) {
        self.rezbuffer();

        let cursor_rect = self.cursor_rect();

        // go through the list of rectangles pending a redraw and expand the total redraw rectangle
        // to encompass all of them
        let mut total_redraw_opt: Option<Rect> = None;
        for original_rect in self.redraws.drain(..) {
            if !original_rect.is_empty() {
                total_redraw_opt = match total_redraw_opt {
                    Some(total_redraw) => Some(total_redraw.container(&original_rect)),
                    None => Some(original_rect),
                };
            }

            for display in self.displays.iter_mut() {
                let rect = original_rect.intersection(&display.screen_rect());
                if !rect.is_empty() {
                    display.rect(&rect, self.config.background_color.into());

                    for entry in self.zbuffer.iter().rev() {
                        let id = entry.0;
                        let i = entry.2;
                        if let Some(window) = self.windows.get_mut(&id) {
                            window.draw_title(
                                display,
                                &rect,
                                i == 0,
                                if i == 0 {
                                    &mut self.window_max
                                } else {
                                    &mut self.window_max_unfocused
                                },
                                if i == 0 {
                                    &mut self.window_close
                                } else {
                                    &mut self.window_close_unfocused
                                },
                            );
                            window.draw(display, &rect);
                        }
                    }

                    if !self.hw_cursor {
                        let cursor_intersect = rect.intersection(&cursor_rect);
                        if !cursor_intersect.is_empty() {
                            if let Some(cursor) = self.cursors.get_mut(&self.cursor_i) {
                                display.roi(&cursor_intersect).blend(
                                    &cursor.roi(
                                        &cursor_intersect
                                            .offset(-cursor_rect.left(), -cursor_rect.top()),
                                    ),
                                );
                            }
                        }
                    }
                }
            }
        }

        if self.win_tabbing {
            //TODO: add to total_redraw?
            self.draw_window_list_osd();
        }

        if self.volume_osd {
            //TODO: add to total_redraw?
            self.draw_volume_osd();
        }

        if self.shortcuts_osd {
            //TODO: add to total_redraw?
            self.draw_shortcuts_osd();
        }

        // Add any redraws from OSD's
        for original_rect in self.redraws.drain(..) {
            if !original_rect.is_empty() {
                total_redraw_opt = match total_redraw_opt {
                    Some(total_redraw) => Some(total_redraw.container(&original_rect)),
                    None => Some(original_rect),
                };
            }
        }

        // Sync any parts of displays that changed
        if let Some(total_redraw) = total_redraw_opt {
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

    fn volume(&mut self, volume: Volume) {
        let value = match fs::read_to_string("/scheme/audio/volume") {
            Ok(string) => match string.parse::<i32>() {
                Ok(value) => value,
                Err(err) => {
                    error!("failed to parse volume '{}': {}", string, err);
                    return;
                }
            },
            Err(err) => {
                error!("failed to read volume: {}", err);
                return;
            }
        };

        self.volume_value = match volume {
            Volume::Down => cmp::max(0, value - 5),
            Volume::Up => cmp::min(100, value + 5),
            Volume::Toggle => {
                if value == 0 {
                    self.volume_toggle
                } else {
                    self.volume_toggle = value;
                    0
                }
            }
        };

        match fs::write("/scheme/audio/volume", format!("{}", self.volume_value)) {
            Ok(()) => (),
            Err(err) => {
                error!("failed to write volume: {}", err);
                return;
            }
        }

        self.volume_osd = true;
    }

    // Tab through the list of selectable windows, changing window order and focus to bring
    // the next one to the front and push the previous one to the back.
    // Note that the selectable windows maybe interlaced in the stack with non-selectable windows,
    // the first selectable window may not be the first in the stack and the bottom selectable
    // window may not be the last in the stack
    fn super_tab(&mut self) {
        // Enter win_tabbing mode
        self.win_tabbing = true;

        let mut selectable_window_indexes: Vec<usize> = vec![];
        for (index, id) in self.order.iter().enumerate() {
            if let Some(window) = self.windows.get(id) {
                if !window.title.is_empty() {
                    selectable_window_indexes.push(index);
                }
            }
        }

        if selectable_window_indexes.len() > 1 {
            // Disable dragging
            self.dragging = DragMode::None;

            // remove focus from the first selectable window in the window stack and make it
            // the last selectable window in the stack. Indexes are the indexes of windows
            // in self.order
            let front_index = selectable_window_indexes[0];
            let next_index = selectable_window_indexes[1];
            let last_index = selectable_window_indexes[selectable_window_indexes.len() - 1];
            if let Some(front_id) = self.order.remove(front_index) {
                self.order.insert(last_index, front_id);
                self.focus(front_id, false); // remove focus from it

                // move to the front and give focus to the next selectable window in the stack
                if let Some(next_id) = self.order.get(next_index) {
                    self.focus(*next_id, true); // move focus to next in stack
                }
            }
        }
    }

    // Create a [Rect][orbital-core::rect::Rect] that places a popup in the middle of the display
    fn popup_rect(image: &ImageRef, width: i32, height: i32) -> Rect {
        Rect::new(
            image.width() / 2 - width / 2,
            image.height() / 2 - height / 2,
            width,
            height,
        )
    }

    // Called by redraw() to draw the list of currently open windows in the middle of the screen.
    // Filter out app windows with no title.
    // If there are no windows to select, nothing is drawn.
    fn draw_window_list_osd(&mut self) {
        const SELECT_POPUP_TOP_BOTTOM_MARGIN: u32 = 2;
        const SELECT_POPUP_SIDE_MARGIN: i32 = 4;
        const SELECT_ROW_HEIGHT: u32 = 20;
        const SELECT_ROW_WIDTH: i32 = 400;
        const FONT_HEIGHT: f32 = 16.0;

        //TODO: HiDPI

        let selectable_window_ids: Vec<usize> = self
            .order
            .iter()
            .filter(|id| {
                if let Some(window) = self.windows.get(id) {
                    !window.title.is_empty()
                } else {
                    false
                }
            })
            .copied()
            .collect();

        if selectable_window_ids.len() > 1 {
            // follow the look of the current config - in terms of colors
            let Config {
                bar_color,
                bar_highlight_color,
                text_color,
                text_highlight_color,
                ..
            } = *self.config;

            let list_h = (selectable_window_ids.len() as u32 * SELECT_ROW_HEIGHT
                + (SELECT_POPUP_TOP_BOTTOM_MARGIN * 2)) as i32;
            let list_w = SELECT_ROW_WIDTH;
            let popup_rect = Self::popup_rect(self.image(), list_w, list_h);
            let mut image = Image::from_color(list_w, list_h, bar_color.into());

            for (selectable_index, window_id) in selectable_window_ids.iter().enumerate() {
                if let Some(window) = self.windows.get(window_id) {
                    let vertical_offset = selectable_index as i32 * SELECT_ROW_HEIGHT as i32
                        + SELECT_POPUP_TOP_BOTTOM_MARGIN as i32;
                    let text = self.font.render(&window.title, FONT_HEIGHT);
                    if selectable_index == 0 {
                        image.rect(
                            0,
                            vertical_offset,
                            list_w as u32,
                            SELECT_ROW_HEIGHT,
                            bar_highlight_color.into(),
                        );
                        text.draw(
                            &mut image,
                            SELECT_POPUP_SIDE_MARGIN,
                            vertical_offset + SELECT_POPUP_TOP_BOTTOM_MARGIN as i32,
                            text_highlight_color.into(),
                        );
                    } else {
                        text.draw(
                            &mut image,
                            SELECT_POPUP_SIDE_MARGIN,
                            vertical_offset + SELECT_POPUP_TOP_BOTTOM_MARGIN as i32,
                            text_color.into(),
                        );
                    }
                }
            }
            self.image_mut()
                .roi(&popup_rect)
                .blit(&image.roi(&Rect::new(0, 0, list_w, list_h)));
            self.popup_rect = popup_rect;
            schedule(&mut self.redraws, popup_rect);
        }
    }

    // Draw an on screen display (overlay) for volume control
    fn draw_volume_osd(&mut self) {
        let Config {
            bar_color,
            bar_highlight_color,
            ..
        } = *self.config;

        const BAR_HEIGHT: i32 = 20;
        const BAR_WIDTH: i32 = 100;
        const POPUP_MARGIN: i32 = 2;

        //TODO: HiDPI
        let list_h = BAR_HEIGHT + (2 * POPUP_MARGIN);
        let list_w = BAR_WIDTH + (2 * POPUP_MARGIN);
        let popup_rect = Self::popup_rect(self.image(), list_w, list_h);
        // Color copied over from orbtk's window background
        let mut image = Image::from_color(list_w, list_h, bar_color.into());
        image.rect(
            POPUP_MARGIN,
            POPUP_MARGIN,
            self.volume_value as u32,
            BAR_HEIGHT as u32,
            bar_highlight_color.into(),
        );
        self.image_mut()
            .roi(&popup_rect)
            .blit(&image.roi(&Rect::new(0, 0, list_w, list_h)));
        self.popup_rect = popup_rect;
        schedule(&mut self.redraws, popup_rect);
    }

    const SHORTCUTS_LIST: &'static [&'static str] = &[
        "Super-Q: Quit current window",
        "Super-TAB: Cycle through active windows bringing to the front of the stack",
        "Super-{: Volume down",
        "Super-}: Volume up",
        "Super-\\: Volume toggle (mute / unmute)",
        "Super-Shift-left: Tile window to left",
        "Super-Shift-right: Tile window to right",
        "Super-Shift-up: Tile window to top",
        "Super-Shift-down: Tile window to bottom",
        "Super-left_arrow: Move window left",
        "Super-right_arrow: Move window right",
        "Super-up_arrow: Move window up",
        "Super-down_arrow: Move window down",
        "Super-C: Copy to copy buffer",
        "Super-X: Cut to copy buffer",
        "Super-V: Paste from the copy buffer",
        "Super-M: Toggle window max (maximize or restore)",
        "Super-ENTER: Toggle window max (maximize or restore)",
    ];

    // Draw an on screen display (overlay) of available SUPER keyboard shortcuts
    fn draw_shortcuts_osd(&mut self) {
        const ROW_HEIGHT: u32 = 20;
        const ROW_WIDTH: i32 = 400;
        const POPUP_BORDER: u32 = 2;
        const FONT_HEIGHT: f32 = 16.0;

        // follow the look of the current config - in terms of colors
        let Config {
            bar_color,
            bar_highlight_color,
            text_highlight_color,
            ..
        } = *self.config;

        let list_h = (Self::SHORTCUTS_LIST.len() as u32 * ROW_HEIGHT + (POPUP_BORDER * 2)) as i32;
        let list_w = ROW_WIDTH;
        let popup_rect = Self::popup_rect(self.image(), list_w, list_h);
        let mut image = Image::from_color(list_w, list_h, bar_color.into());

        for (index, shortcut) in Self::SHORTCUTS_LIST.iter().enumerate() {
            let vertical_offset = index as i32 * ROW_HEIGHT as i32 + POPUP_BORDER as i32;
            let text = self.font.render(shortcut, FONT_HEIGHT);
            image.rect(
                0,
                vertical_offset,
                list_w as u32,
                ROW_HEIGHT,
                bar_highlight_color.into(),
            );
            text.draw(
                &mut image,
                POPUP_BORDER as i32,
                vertical_offset + POPUP_BORDER as i32,
                text_highlight_color.into(),
            );
        }

        self.image_mut()
            .roi(&popup_rect)
            .blit(&image.roi(&Rect::new(0, 0, list_w, list_h)));
        self.popup_rect = popup_rect;
        schedule(&mut self.redraws, popup_rect);
    }

    // Keep track of the modifier keys state based on past keydown/keyup events
    fn track_modifier_state(&mut self, scancode: u8, pressed: bool) {
        match (scancode, pressed) {
            (orbclient::K_SUPER, true) => self.modifier_state |= SUPER_MODIFIER,
            (orbclient::K_SUPER, false) => self.modifier_state &= !SUPER_MODIFIER,
            (orbclient::K_LEFT_SHIFT, true) => self.modifier_state |= SHIFT_LEFT_MODIFIER,
            (orbclient::K_LEFT_SHIFT, false) => self.modifier_state &= !SHIFT_LEFT_MODIFIER,
            (orbclient::K_RIGHT_SHIFT, true) => self.modifier_state |= SHIFT_RIGHT_MODIFIER,
            (orbclient::K_RIGHT_SHIFT, false) => self.modifier_state &= !SHIFT_RIGHT_MODIFIER,
            (orbclient::K_CTRL, true) => self.modifier_state |= CONTROL_MODIFIER,
            (orbclient::K_CTRL, false) => self.modifier_state &= !CONTROL_MODIFIER,
            (orbclient::K_ALT, true) => self.modifier_state |= ALT_MODIFIER,
            (orbclient::K_ALT, false) => self.modifier_state &= !ALT_MODIFIER,
            (orbclient::K_ALT_GR, true) => self.modifier_state |= ALT_GR_MODIFIER,
            (orbclient::K_ALT_GR, false) => self.modifier_state &= !ALT_GR_MODIFIER,
            _ => {}
        }

        if self.modifier_state & SHIFT_LEFT_MODIFIER != 0
            || self.modifier_state & SHIFT_RIGHT_MODIFIER != 0
        {
            self.modifier_state |= SHIFT_ANY_MODIFIER;
        } else {
            self.modifier_state &= !SHIFT_ANY_MODIFIER;
        }

        if self.modifier_state & ALT_MODIFIER != 0 || self.modifier_state & ALT_GR_MODIFIER != 0 {
            self.modifier_state |= ALT_ANY_MODIFIER;
        } else {
            self.modifier_state &= !ALT_ANY_MODIFIER;
        }
    }

    // Move the front-most window horizontally and vertically by the number of pixels passed
    fn move_front_window(&mut self, h_movement: i32, v_movement: i32) {
        if let Some(id) = self.order.front() {
            if let Some(window) = self.windows.get_mut(id) {
                schedule(&mut self.redraws, window.title_rect());
                schedule(&mut self.redraws, window.rect());

                // Align location to grid
                window.x -= window.x % GRID_SIZE;
                window.y -= window.y % GRID_SIZE;

                window.x += h_movement;
                window.y += v_movement;

                // Ensure window remains visible
                window.x = cmp::max(
                    -window.width() + GRID_SIZE,
                    cmp::min(self.displays[0].image.width() - GRID_SIZE, window.x),
                );
                window.y = cmp::max(
                    -window.height() + GRID_SIZE,
                    cmp::min(self.displays[0].image.height() - GRID_SIZE, window.y),
                );

                let move_event = MoveEvent {
                    x: window.x,
                    y: window.y,
                }
                .to_event();
                window.event(move_event);

                schedule(&mut self.redraws, window.title_rect());
                schedule(&mut self.redraws, window.rect());
            }
        }
    }

    fn clipboard_event(&mut self, kind: u8) {
        if let Some(id) = self.order.front() {
            if let Some(window) = self.windows.get_mut(id) {
                //TODO: set window's clipboard to primary
                let clipboard_event = ClipboardEvent { kind, size: 0 }.to_event();
                window.event(clipboard_event);
            }
        }
    }

    fn quit_front_window(&mut self) {
        if let Some(id) = self.order.front() {
            if let Some(window) = self.windows.get_mut(id) {
                window.event(QuitEvent.to_event());
            }
        }
    }

    // tile a window to a defined position. If no window id is provided it will use the front window
    fn tile_window(&mut self, window_id: Option<&usize>, position: TilePosition) {
        if let Some(id) = window_id.or(self.order.front()) {
            if let Some(window) = self.windows.get_mut(id) {
                let display_index = Self::get_display_index(&self.displays, &window.rect());
                schedule(&mut self.redraws, window.title_rect());
                schedule(&mut self.redraws, window.rect());

                let (x, y, width, height) = match window.restore.take() {
                    None => {
                        // we are about to maximize window, so store current size for restore later
                        window.restore = Some(window.rect());

                        let top = self.displays[display_index].y + window.title_rect().height();
                        let left = self.displays[display_index].x;
                        let max_height = self.displays[display_index].image.height()
                            - window.title_rect().height();
                        let max_width = self.displays[display_index].image.width();
                        let half_width = (max_width / 2) as u32;
                        let half_height = (max_height / 2) as u32;

                        match position {
                            LeftHalf => (left, top, half_width, max_height as u32),
                            RightHalf => {
                                (left + half_width as i32, top, half_width, max_height as u32)
                            }
                            TopHalf => (left, top, max_width as u32, half_height),
                            BottomHalf => (
                                left,
                                top + half_height as i32,
                                max_width as u32,
                                half_height,
                            ),
                            FullScreen => (left, top, max_width as u32, max_height as u32),
                        }
                    }
                    Some(restore) => (
                        restore.left(),
                        restore.top(),
                        restore.width() as u32,
                        restore.height() as u32,
                    ),
                };

                // TODO understand why this is needed and why handle_window_position isn't enough
                window.x = x;
                window.y = y;
                window.event(MoveEvent { x, y }.to_event());

                window.event(ResizeEvent { width, height }.to_event());
            };
        }
    }

    // undraw any overlay that was being displayed and exit the mode causing it to be displayed
    fn close_overlays(&mut self) {
        // redraw the area that was occupied by the popup
        schedule(&mut self.redraws, self.popup_rect);
        // disable drawing of the win-tab or volume popup or shortcuts overlay on redraw
        self.win_tabbing = false;
        self.volume_osd = false;
        self.shortcuts_osd = false;
    }

    // Process incoming key events
    fn key_event(&mut self, event: KeyEvent) {
        self.track_modifier_state(event.scancode, event.pressed);

        match (event.scancode, event.pressed) {
            (orbclient::K_SUPER, true) => self.shortcuts_osd = true,
            (orbclient::K_SUPER, false) => self.close_overlays(),
            (orbclient::K_VOLUME_TOGGLE, true) => self.volume(Volume::Toggle),
            (orbclient::K_VOLUME_DOWN, true) => self.volume(Volume::Down),
            (orbclient::K_VOLUME_UP, true) => self.volume(Volume::Up),
            (
                orbclient::K_VOLUME_TOGGLE | orbclient::K_VOLUME_DOWN | orbclient::K_VOLUME_UP,
                false,
            ) => self.volume_osd = false,
            _ => {}
        }

        // process SUPER- key combinations
        if self.modifier_state & SUPER_MODIFIER == SUPER_MODIFIER
            && event.pressed
            && event.scancode != orbclient::K_SUPER
        {
            self.close_overlays();

            let shift = self.modifier_state & SHIFT_ANY_MODIFIER != 0;
            match event.scancode {
                orbclient::K_Q => self.quit_front_window(),
                orbclient::K_TAB => self.super_tab(),
                orbclient::K_BRACE_OPEN => self.volume(Volume::Down),
                orbclient::K_BRACE_CLOSE => self.volume(Volume::Up),
                orbclient::K_BACKSLASH => self.volume(Volume::Toggle),
                orbclient::K_M => self.tile_window(None, FullScreen),
                orbclient::K_ENTER => self.tile_window(None, FullScreen),
                orbclient::K_UP if shift => self.tile_window(None, TopHalf),
                orbclient::K_DOWN if shift => self.tile_window(None, BottomHalf),
                orbclient::K_LEFT if shift => self.tile_window(None, LeftHalf),
                orbclient::K_RIGHT if shift => self.tile_window(None, RightHalf),
                orbclient::K_UP => self.move_front_window(0, -GRID_SIZE),
                orbclient::K_DOWN => self.move_front_window(0, GRID_SIZE),
                orbclient::K_LEFT => self.move_front_window(-GRID_SIZE, 0),
                orbclient::K_RIGHT => self.move_front_window(GRID_SIZE, 0),
                orbclient::K_C => self.clipboard_event(orbclient::CLIPBOARD_COPY),
                orbclient::K_X => self.clipboard_event(orbclient::CLIPBOARD_CUT),
                orbclient::K_V => self.clipboard_event(orbclient::CLIPBOARD_PASTE),
                _ => {
                    //TODO: remove hack for sending super events to lowest numbered window
                    // ADM is this related to Launcher or Background or something?
                    if let Some((id, window)) = self.windows.iter_mut().next() {
                        info!("sending super {:?} to {}, {}", event, id, window.title);
                        let mut super_event = event.to_event();
                        super_event.code += 0x1000_0000;
                        window.event(super_event);
                    }
                }
            }
        }

        // send non-Super key events to the front window
        if self.modifier_state & SUPER_MODIFIER == 0 {
            if let Some(id) = self.order.front() {
                if let Some(window) = self.windows.get_mut(id) {
                    if event.pressed && event.character != '\0' {
                        let text_input_event = TextInputEvent {
                            character: event.character,
                        }
                        .to_event();
                        window.event(text_input_event);
                    }
                    window.event(event.to_event());
                }
            }
        }
    }

    fn mouse_event(&mut self, event: MouseEvent) {
        let mut new_cursor = CursorKind::LeftPtr;
        let mut new_hover = None;

        // Check for focus switch, dragging, and forward mouse events to applications
        match self.dragging {
            DragMode::None => {
                for entry in self.zbuffer.iter() {
                    let id = entry.0;
                    if let Some(window) = self.windows.get_mut(&id) {
                        if window.rect().contains(event.x, event.y) {
                            if !window.mouse_cursor {
                                new_cursor = CursorKind::None;
                            }

                            new_hover = Some(id);
                            if new_hover != self.hover {
                                let hover_event = HoverEvent { entered: true }.to_event();
                                window.event(hover_event);
                            }

                            if self.modifier_state & SUPER_MODIFIER == 0 {
                                let mut window_event = event.to_event();
                                window_event.a -= window.x as i64;
                                window_event.b -= window.y as i64;
                                window.event(window_event);
                            }
                            break;
                        } else if window.title_rect().contains(event.x, event.y) {
                            break;
                        } else if window.left_border_rect().contains(event.x, event.y) {
                            new_cursor = CursorKind::LeftSide;
                            break;
                        } else if window.right_border_rect().contains(event.x, event.y) {
                            new_cursor = CursorKind::RightSide;
                            break;
                        } else if window.bottom_border_rect().contains(event.x, event.y) {
                            new_cursor = CursorKind::BottomSide;
                            break;
                        } else if window.bottom_left_border_rect().contains(event.x, event.y) {
                            new_cursor = CursorKind::BottomLeftCorner;
                            break;
                        } else if window.bottom_right_border_rect().contains(event.x, event.y) {
                            new_cursor = CursorKind::BottomRightCorner;
                            break;
                        }
                    }
                }
            }
            DragMode::Title(window_id, drag_x, drag_y) => {
                if let Some(window) = self.windows.get_mut(&window_id) {
                    if drag_x != event.x || drag_y != event.y {
                        schedule(&mut self.redraws, window.title_rect());
                        schedule(&mut self.redraws, window.rect());

                        //TODO: Min and max
                        window.x += event.x - drag_x;
                        window.y += event.y - drag_y;

                        let move_event = MoveEvent {
                            x: window.x,
                            y: window.y,
                        }
                        .to_event();
                        window.event(move_event);

                        self.dragging = DragMode::Title(window_id, event.x, event.y);

                        schedule(&mut self.redraws, window.title_rect());
                        schedule(&mut self.redraws, window.rect());
                    }
                } else {
                    self.dragging = DragMode::None;
                }
            }
            DragMode::LeftBorder(window_id, off_x, right_x) => {
                if let Some(window) = self.windows.get_mut(&window_id) {
                    new_cursor = CursorKind::LeftSide;

                    let x = event.x - off_x;
                    let w = right_x - x;

                    if w > 0 {
                        if x != window.x {
                            schedule(&mut self.redraws, window.title_rect());
                            schedule(&mut self.redraws, window.rect());

                            window.x = x;
                            let move_event = MoveEvent { x, y: window.y }.to_event();
                            window.event(move_event);

                            schedule(&mut self.redraws, window.title_rect());
                            schedule(&mut self.redraws, window.rect());
                        }

                        if w != window.width() {
                            let resize_event = ResizeEvent {
                                width: w as u32,
                                height: window.height() as u32,
                            }
                            .to_event();
                            window.event(resize_event);
                        }
                    }
                } else {
                    self.dragging = DragMode::None;
                }
            }
            DragMode::RightBorder(window_id, off_x) => {
                if let Some(window) = self.windows.get_mut(&window_id) {
                    new_cursor = CursorKind::RightSide;
                    let w = event.x - off_x - window.x;
                    if w > 0 && w != window.width() {
                        let resize_event = ResizeEvent {
                            width: w as u32,
                            height: window.height() as u32,
                        }
                        .to_event();
                        window.event(resize_event);
                    }
                } else {
                    self.dragging = DragMode::None;
                }
            }
            DragMode::BottomBorder(window_id, off_y) => {
                if let Some(window) = self.windows.get_mut(&window_id) {
                    new_cursor = CursorKind::BottomSide;
                    let h = event.y - off_y - window.y;
                    if h > 0 && h != window.height() {
                        let resize_event = ResizeEvent {
                            width: window.width() as u32,
                            height: h as u32,
                        }
                        .to_event();
                        window.event(resize_event);
                    }
                } else {
                    self.dragging = DragMode::None;
                }
            }
            DragMode::BottomLeftBorder(window_id, off_x, off_y, right_x) => {
                if let Some(window) = self.windows.get_mut(&window_id) {
                    new_cursor = CursorKind::BottomLeftCorner;

                    let x = event.x - off_x;
                    let h = event.y - off_y - window.y;
                    let w = right_x - x;

                    if w > 0 && h > 0 {
                        if x != window.x {
                            schedule(&mut self.redraws, window.title_rect());
                            schedule(&mut self.redraws, window.rect());

                            window.x = x;
                            let move_event = MoveEvent { x, y: window.y }.to_event();
                            window.event(move_event);

                            schedule(&mut self.redraws, window.title_rect());
                            schedule(&mut self.redraws, window.rect());
                        }

                        if w != window.width() || h != window.height() {
                            let resize_event = ResizeEvent {
                                width: w as u32,
                                height: h as u32,
                            }
                            .to_event();
                            window.event(resize_event);
                        }
                    }
                } else {
                    self.dragging = DragMode::None;
                }
            }
            DragMode::BottomRightBorder(window_id, off_x, off_y) => {
                if let Some(window) = self.windows.get_mut(&window_id) {
                    new_cursor = CursorKind::BottomRightCorner;
                    let w = event.x - off_x - window.x;
                    let h = event.y - off_y - window.y;
                    if w > 0 && h > 0 && (w != window.width() || h != window.height()) {
                        let resize_event = ResizeEvent {
                            width: w as u32,
                            height: h as u32,
                        }
                        .to_event();
                        window.event(resize_event);
                    }
                } else {
                    self.dragging = DragMode::None;
                }
            }
        }

        if new_hover != self.hover {
            if let Some(id) = self.hover {
                if let Some(window) = self.windows.get_mut(&id) {
                    let hover_event = HoverEvent { entered: false }.to_event();
                    window.event(hover_event);
                }
            }

            self.hover = new_hover;
        }

        if self.hw_cursor {
            self.cursor_x = event.x;
            self.cursor_y = event.y;
            self.update_hw_cursor(new_cursor);
        } else {
            self.update_cursor(event.x, event.y, new_cursor);
        }
    }

    fn mouse_relative_event(&mut self, event: MouseRelativeEvent) {
        let mut relative_cursor_opt = None;
        if let Some(id) = self.order.front() {
            if let Some(window) = self.windows.get_mut(id) {
                //TODO: handle grab?
                if window.mouse_relative {
                    // Send relative event
                    window.event(event.to_event());

                    // Update cursor to center of this window
                    relative_cursor_opt = Some((
                        window.x + window.width() / 2,
                        window.y + window.height() / 2,
                        //TODO: allow cursors on relative windows?
                        CursorKind::None,
                    ));
                }
            }
        }

        // Handle relative window cursor
        if let Some((x, y, kind)) = relative_cursor_opt {
            if self.hw_cursor {
                self.cursor_x = x;
                self.cursor_y = y;
                self.update_hw_cursor(kind);
            } else {
                self.update_cursor(x, y, kind);
            }
            return;
        }

        //TODO: more advanced logic for keeping mouse on screen.
        // This logic assumes horizontal and touching, but not overlapping, screens
        let mut max_x = 0;
        let mut max_y = 0;
        for display in self.displays.iter() {
            let rect = display.screen_rect();
            max_x = cmp::max(max_x, rect.right() - 1);
            max_y = cmp::max(max_y, rect.bottom() - 1);
        }

        let x = cmp::max(0, cmp::min(max_x, self.cursor_x + event.dx));
        let mut y = cmp::max(0, cmp::min(max_y, self.cursor_y + event.dy));
        for display in self.displays.iter() {
            let rect = display.screen_rect();
            if x >= rect.left() && x < rect.right() {
                y = cmp::max(rect.top(), cmp::min(rect.bottom() - 1, y));
            }
        }

        self.mouse_event(MouseEvent { x, y });
    }

    // Find the display that a window (`rect`) most overlaps and return the index of it
    fn get_display_index(displays: &[Display], rect: &Rect) -> usize {
        // Find the index of the Display this window has the most overlap with
        let mut display_index = 0;
        let mut max_intersection_area = 0;
        for (display_i, display) in displays.iter().enumerate() {
            let intersect = display.screen_rect().intersection(rect);
            let area = intersect.area();
            if area > max_intersection_area {
                display_index = display_i;
                max_intersection_area = area;
            }
        }

        display_index
    }

    fn button_event(&mut self, event: ButtonEvent) {
        // Check for focus switch, dragging, and forward mouse events to applications
        match self.dragging {
            DragMode::None => {
                let mut focus = 0;
                for entry in self.zbuffer.iter() {
                    let id = entry.0;
                    let i = entry.2;
                    if let Some(window) = self.windows.get(&id) {
                        if window.rect().contains(self.cursor_x, self.cursor_y) {
                            if self.modifier_state & SUPER_MODIFIER == SUPER_MODIFIER {
                                if event.left && !self.cursor_left {
                                    focus = i;
                                    self.dragging =
                                        DragMode::Title(id, self.cursor_x, self.cursor_y);
                                }
                            } else if let Some(window) = self.windows.get_mut(&id) {
                                window.event(event.to_event());
                                if event.left && !self.cursor_left
                                    || event.middle && !self.cursor_middle
                                    || event.right && !self.cursor_right
                                {
                                    focus = i;
                                }
                            }
                            break;
                        } else if window.title_rect().contains(self.cursor_x, self.cursor_y) {
                            //TODO: Trigger max and exit on release
                            if event.left && !self.cursor_left {
                                focus = i;
                                if (window.max_contains(self.cursor_x, self.cursor_y))
                                    && (window.resizable)
                                {
                                    self.tile_window(Some(&id), FullScreen);
                                } else if (window.close_contains(self.cursor_x, self.cursor_y))
                                    && (!window.unclosable)
                                {
                                    if let Some(window) = self.windows.get_mut(&id) {
                                        window.event(QuitEvent.to_event());
                                    }
                                } else {
                                    self.dragging =
                                        DragMode::Title(id, self.cursor_x, self.cursor_y);
                                }
                            }
                            break;
                        } else if window
                            .left_border_rect()
                            .contains(self.cursor_x, self.cursor_y)
                        {
                            if event.left && !self.cursor_left {
                                focus = i;
                                self.dragging = DragMode::LeftBorder(
                                    id,
                                    self.cursor_x - window.x,
                                    window.x + window.width(),
                                );
                            }
                            break;
                        } else if window
                            .right_border_rect()
                            .contains(self.cursor_x, self.cursor_y)
                        {
                            if event.left && !self.cursor_left {
                                focus = i;
                                self.dragging = DragMode::RightBorder(
                                    id,
                                    self.cursor_x - (window.x + window.width()),
                                );
                            }
                            break;
                        } else if window
                            .bottom_border_rect()
                            .contains(self.cursor_x, self.cursor_y)
                        {
                            if event.left && !self.cursor_left {
                                focus = i;
                                self.dragging = DragMode::BottomBorder(
                                    id,
                                    self.cursor_y - (window.y + window.height()),
                                );
                            }
                            break;
                        } else if window
                            .bottom_left_border_rect()
                            .contains(self.cursor_x, self.cursor_y)
                        {
                            if event.left && !self.cursor_left {
                                focus = i;
                                self.dragging = DragMode::BottomLeftBorder(
                                    id,
                                    self.cursor_x - window.x,
                                    self.cursor_y - (window.y + window.height()),
                                    window.x + window.width(),
                                );
                            }
                            break;
                        } else if window
                            .bottom_right_border_rect()
                            .contains(self.cursor_x, self.cursor_y)
                        {
                            if event.left && !self.cursor_left {
                                focus = i;
                                self.dragging = DragMode::BottomRightBorder(
                                    id,
                                    self.cursor_x - (window.x + window.width()),
                                    self.cursor_y - (window.y + window.height()),
                                );
                            }
                            break;
                        }
                    }
                }

                if focus > 0 {
                    // Redraw old focused window
                    if let Some(id) = self.order.front() {
                        self.focus(*id, false);
                    }

                    // Reorder windows
                    if let Some(id) = self.order.remove(focus) {
                        if let Some(window) = self.windows.get(&id) {
                            match window.zorder {
                                WindowZOrder::Front | WindowZOrder::Normal => {
                                    // Transfer focus if a front or normal window
                                    self.order.push_front(id);
                                }
                                WindowZOrder::Back => {
                                    // Return to original position if a background window
                                    self.order.insert(focus, id);
                                }
                            }
                        }
                    }

                    // Redraw new focused window
                    if let Some(id) = self.order.front() {
                        self.focus(*id, true);
                    }
                }
            }
            _ => {
                if !event.left {
                    self.dragging = DragMode::None;
                }
            }
        }

        self.cursor_left = event.left;
        self.cursor_middle = event.middle;
        self.cursor_right = event.right;
    }

    fn resize_event(&mut self, event: ResizeEvent) {
        self.resize(event.width as i32, event.height as i32);

        let screen_rect = self.screen_rect();
        schedule(&mut self.redraws, screen_rect);

        let screen_event = ScreenEvent {
            width: self.image().width() as u32,
            height: self.image().height() as u32,
        }
        .to_event();
        for (_window_id, window) in self.windows.iter_mut() {
            window.event(screen_event);
        }
    }

    fn event(&mut self, event_union: Event) {
        self.rezbuffer();

        if self.hw_cursor
            && (!self.hw_cursor_initialized
                || self.update_cursor_timer.elapsed().as_millis() > 1000)
        {
            let cursor_kind = self.cursor_i;
            self.cursor_i = CursorKind::None;
            self.update_hw_cursor(cursor_kind);
            self.update_cursor_timer = Instant::now();
        }

        match event_union.to_option() {
            EventOption::Key(event) => self.key_event(event),
            EventOption::Mouse(MouseEvent { x, y }) => {
                // ps2d gives us absolute mouse events with x and y in the range 0..65535.
                // We need to translate this back to screen coordinates. We are using the
                // size of the first display here as the only multi-display system supported
                // by qemu doesn't produce absolute mouse events using vmmouse at all.
                // FIXME once we have usb tablet support add a new event like MouseEvent
                // which indicates the input device from which the event originated to use
                // the correct display for getting the size.
                self.mouse_event(MouseEvent {
                    x: x * self.displays[0].image.width() / 65536,
                    y: y * self.displays[0].image.height() / 65536,
                });
            }
            EventOption::MouseRelative(event) => self.mouse_relative_event(event),
            EventOption::Button(event) => self.button_event(event),
            EventOption::Scroll(_) => {
                if let Some(entry) = self.zbuffer.first() {
                    let id = entry.0;
                    if let Some(window) = self.windows.get_mut(&id) {
                        window.event(event_union);
                    }
                }
            }
            EventOption::Resize(event) => self.resize_event(event),
            event => error!("unexpected event: {:?}", event),
        }
    }

    fn input_event(&mut self, events: &[Event]) -> io::Result<()> {
        for &event in events {
            self.event(event);
        }

        // redrawn by handle_after

        Ok(())
    }

    fn window_new(
        &mut self,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        flags: &str,
        title: String,
    ) -> Result<usize> {
        let id = self.next_id as usize;
        self.next_id += 1;
        if self.next_id < 0 {
            //TODO: should this be an error?
            self.next_id = 1;
        }

        // Unfocus previous top window
        if let Some(id) = self.order.front() {
            self.focus(*id, false);
        }

        let mut window = Window::new(x, y, width, height, self.scale, Rc::clone(&self.config));

        for flag in flags.chars() {
            window.set_flag(flag, true);
        }

        window.title = title;
        window.render_title(&self.font);

        if x < 0 && y < 0 {
            // Automatic placement
            window.x = cmp::max(0, (self.image().width() - width) / 2);
            window.y = cmp::max(
                window.title_rect().height(),
                (self.image().height() - height) / 2,
            );
        }

        // Redraw new window
        schedule(&mut self.redraws, window.title_rect());
        schedule(&mut self.redraws, window.rect());

        // Add to zorder as appropriate
        match window.zorder {
            WindowZOrder::Front | WindowZOrder::Normal => {
                self.order.push_front(id);
            }
            WindowZOrder::Back => {
                self.order.push_back(id);
            }
        }

        self.windows.insert(id, window);

        // Focus new top window
        if let Some(id) = self.order.front() {
            self.focus(*id, true);
        }

        // Ensure mouse cursor is correct
        let event = MouseEvent {
            x: self.cursor_x,
            y: self.cursor_y,
        };
        self.mouse_event(event);

        Ok(id)
    }

    fn update_hw_cursor(&mut self, new_cursor: CursorKind) {
        //header flag that indicates update_cursor or move_cursor
        let mut header: u32 = 0;
        if self.cursor_i != new_cursor || !self.hw_cursor_initialized {
            header = 1;
            self.cursor_i = new_cursor;
            self.hw_cursor_initialized = true;
        }

        //retrieve image data
        let cursor = self.cursors.get_mut(&new_cursor).unwrap();
        let cursor_img: [u32; 4096] = if header != 0 {
            cursor.get_cursor_data()
        } else {
            [0; 4096]
        };

        let w: i32 = cursor.width();
        let h: i32 = cursor.height();

        let x: i32 = self.cursor_x;
        let y: i32 = self.cursor_y;

        let (hot_x, hot_y) = match new_cursor {
            CursorKind::None => (0, 0),
            CursorKind::LeftPtr => (0, 0),
            CursorKind::BottomLeftCorner => (0, h),
            CursorKind::BottomRightCorner => (w, h),
            CursorKind::BottomSide => (w / 2, h),
            CursorKind::LeftSide => (0, h / 2),
            CursorKind::RightSide => (w, h / 2),
        };

        //Construct object to send to the display
        #[repr(C, packed)]
        struct SyncRect {
            header: u32,
            x: i32,
            y: i32,
            hot_x: i32,
            hot_y: i32,
            w: i32,
            h: i32,
            cursor_img: [u32; 4096],
        }

        let sync_rect = SyncRect {
            header,
            x,
            y,
            hot_x,
            hot_y,
            w,
            h,
            cursor_img,
        };

        for (i, display) in self.displays.iter_mut().enumerate() {
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
