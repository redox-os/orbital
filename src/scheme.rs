use std::num::NonZero;
use std::rc::Rc;
use std::str::FromStr;
use std::sync::Arc;
use std::{cmp, collections::BTreeMap, fs, str};

use log::{error, info, warn};
use orbclient::image::Image;
use orbclient::rect::{Rect, RectEdge};
use orbclient::*;
use syscall::error::{EBADF, Error, Result};

use crate::compositor::{Compositor, SCALE_BASELINE};
use crate::config::Config;
use crate::core::Properties;
use crate::widget::fps::FpsWidget;
use crate::widget::shortcuts::ShortcutsWidget;
use crate::window::{Window, WindowId};
use crate::window_order::{WindowOrder, WindowZOrder};

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

#[derive(Debug, Clone)]
enum DragMode {
    None,
    Title(WindowId, i32, i32),
    TopBorder(WindowId, i32, i32),
    LeftBorder(WindowId, i32, i32),
    RightBorder(WindowId, i32),
    BottomBorder(WindowId, i32),
    BottomLeftBorder(WindowId, i32, i32, i32),
    BottomRightBorder(WindowId, i32, i32),
}

#[derive(Debug, Clone)]
enum Volume {
    Down,
    Up,
    Toggle,
}

#[derive(Clone, Copy, Debug)]
pub enum TilePosition {
    LeftHalf,
    TopHalf,
    RightHalf,
    BottomHalf,
    Maximized,
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
    compositor: Compositor,

    window_max: Image,
    window_max_unfocused: Image,
    window_close: Image,
    window_close_unfocused: Image,
    cursors: BTreeMap<CursorKind, Arc<Image>>,
    cursor_x: i32,
    cursor_y: i32,
    cursor_left: bool,
    cursor_middle: bool,
    cursor_right: bool,
    cursor_simulate_enabled: bool,
    cursor_simulate_speed: i32,
    dragging: DragMode,
    dragging_window_initiated: bool,
    modifier_state: u8,
    volume_value: i32,
    volume_toggle: i32,
    next_id: isize,
    hover: Option<WindowId>,
    order: WindowOrder,
    windows: BTreeMap<WindowId, Window>,
    font: orbfont::Font,
    clipboard: Vec<u8>,
    config: Rc<Config>,
    // Is the user currently switching windows with win-tab
    // Set true when win-tab is pressed, set false when win is released.
    // While it is true, redraw() calls draw_window_list()
    win_tabbing: bool,
    volume_osd: bool,
    last_popup_rect: Option<Rect>,
    shortcuts_widget: ShortcutsWidget,
    fps_widget: FpsWidget,
}

impl OrbitalScheme {
    pub(crate) fn new(compositor: Compositor, config: Rc<Config>) -> Result<OrbitalScheme, String> {
        let scale = NonZero::new(compositor.scale()).unwrap_or(NonZero::new(1).unwrap());
        let load_image = |path| {
            Image::from_path(path)
                .unwrap_or(Image::new(0, 0))
                .resize_exact(scale)
        };
        let load_cursor = |path| Arc::new(load_image(path));

        let mut cursors = BTreeMap::new();
        cursors.insert(CursorKind::None, Arc::new(Image::new(0, 0)));
        cursors.insert(CursorKind::LeftPtr, load_cursor(&config.cursor));
        cursors.insert(
            CursorKind::BottomLeftCorner,
            load_cursor(&config.bottom_left_corner),
        );
        cursors.insert(
            CursorKind::BottomRightCorner,
            load_cursor(&config.bottom_right_corner),
        );
        cursors.insert(CursorKind::BottomSide, load_cursor(&config.bottom_side));
        cursors.insert(CursorKind::LeftSide, load_cursor(&config.left_side));
        cursors.insert(CursorKind::RightSide, load_cursor(&config.right_side));

        let font = orbfont::Font::find(Some("Sans"), None, None)?;

        let mut orbital_scheme = OrbitalScheme {
            compositor,

            window_max: load_image(&config.window_max),
            window_max_unfocused: load_image(&config.window_max_unfocused),
            window_close: load_image(&config.window_close),
            window_close_unfocused: load_image(&config.window_close_unfocused),
            cursors,
            cursor_x: 0,
            cursor_y: 0,
            cursor_left: false,
            cursor_middle: false,
            cursor_right: false,
            cursor_simulate_speed: 32,
            cursor_simulate_enabled: false,
            dragging: DragMode::None,
            dragging_window_initiated: false,
            modifier_state: 0,
            volume_value: 0,
            volume_toggle: 0,
            next_id: 1,
            hover: None,
            order: WindowOrder::new(),
            windows: BTreeMap::new(),
            font,
            clipboard: Vec::new(),
            config: Rc::clone(&config),
            win_tabbing: false,
            volume_osd: false,
            last_popup_rect: None,
            shortcuts_widget: ShortcutsWidget::new(),
            fps_widget: FpsWidget::new(),
        };

        orbital_scheme.update_cursor(0, 0, CursorKind::LeftPtr);

        Ok(orbital_scheme)
    }

    pub(crate) fn display_count(&self) -> usize {
        self.compositor.displays().len()
    }

    pub(crate) fn display_size(&self, display: usize) -> (u32, u32, u32) {
        let rect = self.compositor.displays()[display].screen_rect();
        (rect.width(), rect.height(), self.compositor.scale())
    }

    fn update_window(
        compositor: &mut Compositor,
        window: &mut Window,
        f: impl FnOnce(&Compositor, &mut Window),
    ) {
        compositor.schedule(window.title_rect());
        compositor.schedule(window.rect());

        f(compositor, window);

        compositor.schedule(window.title_rect());
        compositor.schedule(window.rect());
    }

    fn focus(&mut self, id: WindowId, focused: bool) {
        if let Some(window) = self.windows.get_mut(&id) {
            Self::update_window(&mut self.compositor, window, |_compositor, window| {
                window.event(FocusEvent { focused }.to_event());
            });
        }
    }

    //TODO: update cursor in more places to ensure consistency:
    // - Window resizes
    // - Window sets cursor on/off
    // - Window moves
    fn update_cursor(&mut self, x: i32, y: i32, kind: CursorKind) {
        self.cursor_x = x;
        self.cursor_y = y;

        let cursor = self.cursors.get(&kind).unwrap();

        let w = cursor.width() as i32;
        let h = cursor.height() as i32;

        let (hot_x, hot_y) = match kind {
            CursorKind::None => (0, 0),
            CursorKind::LeftPtr => (0, 0),
            CursorKind::BottomLeftCorner => (0, h),
            CursorKind::BottomRightCorner => (w, h),
            CursorKind::BottomSide => (w / 2, h),
            CursorKind::LeftSide => (0, h / 2),
            CursorKind::RightSide => (w, h / 2),
        };

        self.compositor
            .update_cursor(self.cursor_x, self.cursor_y, hot_x, hot_y, cursor);
    }
}

impl OrbitalScheme {
    /// Return true if a packet should be delayed until a display event
    pub fn should_delay(&self, id: WindowId) -> bool {
        self.windows
            .get(&id)
            .map(|window| !window.asynchronous)
            .unwrap_or(true)
    }

    /// Callback to handle events over the input handle
    pub fn handle_input(&mut self, events: &[Event]) {
        for &event in events {
            self.event(event);
        }
    }

    pub fn get_window_mut(&mut self, id: WindowId) -> Option<&mut Window> {
        self.windows.get_mut(&id)
    }

    /// Called when a new window is requested by the scheme.
    /// Return a window ID that will be used to identify it later.
    #[allow(clippy::too_many_arguments)]
    pub fn handle_window_new(
        &mut self,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        parts: &str,
        title: String,
    ) -> Result<WindowId> {
        self.window_new(x, y, width, height, parts, title)
    }

    /// Called when the scheme is read for events
    pub fn handle_window_read(&mut self, id: WindowId, buf: &mut [Event]) -> Result<usize> {
        let window = self.windows.get_mut(&id).ok_or(Error::new(EBADF))?;
        Ok(window.read(buf))
    }

    /// Called when the window asks to set async
    pub fn handle_window_async(&mut self, id: WindowId, is_async: bool) -> Result<()> {
        let window = self.windows.get_mut(&id).ok_or(Error::new(EBADF))?;
        window.asynchronous = is_async;
        Ok(())
    }

    /// Called when the window asks to be dragged
    pub fn handle_window_drag(&mut self, id: WindowId, mode: WindowDragKind) -> Result<()> {
        if self.hover != Some(id) {
            return Ok(());
        }
        let window = self.windows.get_mut(&id).ok_or(Error::new(EBADF))?;

        self.dragging = match mode {
            WindowDragKind::Move => DragMode::Title(id, self.cursor_x, self.cursor_y),
            WindowDragKind::ResizeTop => {
                DragMode::TopBorder(id, self.cursor_y - window.y, window.y + window.iheight())
            }
            WindowDragKind::ResizeLeft => {
                DragMode::LeftBorder(id, self.cursor_x - window.x, window.x + window.iwidth())
            }
            WindowDragKind::ResizeRight => {
                DragMode::RightBorder(id, self.cursor_x - (window.x + window.iwidth()))
            }
            WindowDragKind::ResizeBottom => {
                DragMode::BottomBorder(id, self.cursor_y - (window.y + window.iheight()))
            }
            WindowDragKind::None => DragMode::None,
        };
        self.dragging_window_initiated = true;

        Ok(())
    }

    /// Called when the window asks to set mouse cursor visibility
    pub fn handle_window_mouse_cursor(&mut self, id: WindowId, visible: bool) -> Result<()> {
        let window = self.windows.get_mut(&id).ok_or(Error::new(EBADF))?;
        window.mouse_cursor = visible;
        Ok(())
    }

    /// Called when the window asks to set mouse grabbing
    pub fn handle_window_mouse_grab(&mut self, id: WindowId, grab: bool) -> Result<()> {
        let window = self.windows.get_mut(&id).ok_or(Error::new(EBADF))?;
        window.mouse_grab = grab;
        Ok(())
    }

    /// Called when the window asks to set mouse relative mode
    pub fn handle_window_mouse_relative(&mut self, id: WindowId, relative: bool) -> Result<()> {
        let window = self.windows.get_mut(&id).ok_or(Error::new(EBADF))?;
        window.mouse_relative = relative;
        Ok(())
    }

    /// Called when the window asks to be repositioned
    pub fn handle_window_position(
        &mut self,
        id: WindowId,
        x: Option<i32>,
        y: Option<i32>,
    ) -> Result<()> {
        let window = self.windows.get_mut(&id).ok_or(Error::new(EBADF))?;
        Self::update_window(&mut self.compositor, window, |_compositor, window| {
            window.x = x.unwrap_or(window.x);
            window.y = y.unwrap_or(window.y);
        });

        Ok(())
    }

    /// Called when the window asks to be resized
    pub fn handle_window_resize(
        &mut self,
        id: WindowId,
        w: Option<u32>,
        h: Option<u32>,
    ) -> Result<()> {
        let window = self.windows.get_mut(&id).ok_or(Error::new(EBADF))?;
        let mut resize_event = None;
        Self::update_window(&mut self.compositor, window, |_compositor, window| {
            let w = w.unwrap_or(window.image_width());
            let h = h.unwrap_or(window.image_height());
            if w == window.image_width() && h == window.image_height() {
                return;
            }
            window.set_size(w, h);

            if window.resizing {
                // when user initiated resizing, application must not ask their own size until it
                // follows what user is requesting, only then the application can request again.
                // So far, there's no GUI app that ask any other value when ResizeEvent is received.
                if w != window.width() || h != window.height() {
                    // the last resize event is too late, send again
                    resize_event = Some(ResizeEvent {
                        height: window.height(),
                        width: window.width(),
                    })
                } else {
                    window.resizing = false;
                }
            }
        });

        if let Some(resize_event) = resize_event {
            window.event(resize_event.to_event());
        }

        Ok(())
    }

    /// Called when the window wants to set a flag
    pub fn handle_window_set_flag(
        &mut self,
        id: WindowId,
        flag: WindowFlag,
        value: bool,
    ) -> Result<()> {
        let window = self.windows.get_mut(&id).ok_or(Error::new(EBADF))?;
        // Handle maximized flag custom
        if matches!(flag, WindowFlag::Maximized | WindowFlag::Fullscreen) {
            let toggle_tile = if value {
                window.restore = None;
                true
            } else {
                window.restore.is_some()
            };
            if toggle_tile {
                Self::tile_window(
                    &mut self.compositor,
                    &mut self.windows,
                    id,
                    if flag == WindowFlag::Fullscreen {
                        TilePosition::FullScreen
                    } else {
                        TilePosition::Maximized
                    },
                );
            }
        } else {
            // Setting flag may change visibility, make sure to queue redraws both before and after
            Self::update_window(&mut self.compositor, window, |_compositor, window| {
                window.set_flag(flag, value);
            });
            // Send scale event to the window, not part of queue redraw
            if flag == WindowFlag::Scalable && value {
                let scale_event = ScaleEvent {
                    scale: self.compositor.factored_scale() as i32,
                    baseline: SCALE_BASELINE as i32,
                };
                window.event(scale_event.to_event());
            }
        }

        Ok(())
    }

    /// Called when the window asks to change title
    pub fn handle_window_title(&mut self, id: WindowId, title: String) -> Result<()> {
        let window = self.windows.get_mut(&id).ok_or(Error::new(EBADF))?;
        window.title = title;
        window.render_title(&self.font);

        self.compositor.schedule(window.title_rect());

        Ok(())
    }

    /// Called by fevent to clear notified status, assuming you're sending edge-triggered notifications
    /// TODO: Abstract event system away completely.
    pub fn handle_window_clear_notified(&mut self, id: WindowId) -> Result<()> {
        let window = self.windows.get_mut(&id).ok_or(Error::new(EBADF))?;
        window.notified_read = false;
        Ok(())
    }

    /// Return a reference the window's image that will be mapped in the scheme's fmap function
    pub fn handle_window_map(&mut self, id: WindowId, create_new: bool) -> Result<&mut [Color]> {
        let window = self.windows.get_mut(&id).ok_or(Error::new(EBADF))?;
        if create_new {
            window.maps += 1;
        }
        Ok(window.map())
    }

    /// Free a reference to the window's image, for use by funmap
    pub fn handle_window_unmap(&mut self, id: WindowId) -> Result<()> {
        let window = self.windows.get_mut(&id).ok_or(Error::new(EBADF))?;
        if window.maps > 0 {
            window.maps -= 1;
        } else {
            warn!("attempted unmap when there are no mappings");
        }
        Ok(())
    }

    /// Called to get window properties
    pub fn handle_window_properties(&mut self, id: WindowId) -> Result<Properties<'_>> {
        let window = self.windows.get(&id).ok_or(Error::new(EBADF))?;
        Ok(window.properties())
    }

    /// Called to flush a window. It's usually a good idea to redraw here.
    pub fn handle_window_sync(&mut self, id: WindowId, damages: Option<Vec<Rect>>) -> Result<()> {
        let window = self.windows.get_mut(&id).ok_or(Error::new(EBADF))?;
        window.handle_window_sync(&damages);
        let rect = window.rect();
        if let Some(damages) = damages {
            for damage in damages {
                let dmgr = rect.intersection(&damage.translate(rect.left(), rect.top()));
                self.compositor.schedule(dmgr);
            }
        } else {
            self.compositor.schedule(rect);
        }
        Ok(())
    }

    /// Called when a window should be closed
    pub fn handle_window_close(&mut self, id: WindowId) {
        // Unfocus current front window
        if let Some(id) = self.order.focused() {
            self.focus(id, false);
        }

        self.order.remove_window(id);

        if let Some(window) = self.windows.remove(&id) {
            self.compositor.schedule(window.title_rect());
            self.compositor.schedule(window.rect());
        }

        // Focus current front window
        if let Some(id) = self.order.focused() {
            self.focus(id, true);
        }

        // Ensure mouse cursor is correct
        let event = MouseEvent {
            x: self.cursor_x,
            y: self.cursor_y,
        };
        self.mouse_event(event);
    }

    /// Read window clipboard
    pub fn handle_clipboard_read(
        &mut self,
        _id: WindowId,
        offset: u64,
        buf: &mut [u8],
    ) -> Result<usize> {
        //TODO: implement better clipboard mechanism
        let mut i = 0;
        let mut offset = offset as usize;
        while i < buf.len() && offset < self.clipboard.len() {
            buf[i] = self.clipboard[i];
            i += 1;
            offset += 1;
        }
        Ok(i)
    }

    /// Write window clipboard
    pub fn handle_clipboard_write(
        &mut self,
        _id: WindowId,
        offset: u64,
        buf: &[u8],
    ) -> Result<usize> {
        //TODO: implement better clipboard mechanism
        let mut i = 0;
        self.clipboard.truncate(offset as usize);
        while i < buf.len() {
            self.clipboard.push(buf[i]);
            i += 1;
        }
        Ok(i)
    }

    pub(crate) fn redraw(&mut self) {
        self.resize_if_necessary();

        self.fps_widget.start_measure();
        self.order
            .rezbuffer(&|id| self.windows.get(&id).unwrap().zorder);

        let popup_owned;
        let popup_lazy;
        let popup = if self.shortcuts_widget.enabled {
            popup_lazy = true; // shortcuts_widget never need to update
            self.shortcuts_widget
                .draw_osd(self.compositor.scale(), &self.config, &self.font)
        } else if self.volume_osd {
            popup_lazy = false; // TODO: make it lazy like fps widget
            popup_owned = Some(self.draw_volume_osd());
            popup_owned.as_ref()
        } else if self.win_tabbing {
            popup_lazy = false; // TODO: make it lazy like fps widget
            popup_owned = self.draw_window_list_osd();
            popup_owned.as_ref()
        } else {
            popup_lazy = false;
            None
        };
        if popup.is_none() {
            if let Some(last_popup_rect) = self.last_popup_rect.take() {
                self.compositor.schedule(last_popup_rect);
            }
        }
        let popup_rect = if let Some(popup) = &popup {
            let rect = Rect::new(
                self.compositor.screen_rect().iwidth() / 2 - popup.width() as i32 / 2,
                self.compositor.screen_rect().iheight() / 2 - popup.height() as i32 / 2,
                popup.width(),
                popup.height(),
            );
            if !popup_lazy
                || self
                    .last_popup_rect
                    .is_none_or(|s| s.width() != rect.width() || s.height() != rect.height())
            {
                self.last_popup_rect = Some(rect);
                self.compositor.schedule(rect);
            }
            Some(rect)
        } else {
            None
        };

        {
            let popup = self
                .fps_widget
                .draw_osd(self.compositor.scale(), &self.config, &self.font);
            if let Some(popup) = popup {
                let rect = Rect::new(
                    (self.compositor.screen_rect().iwidth() - popup.width() as i32) / 2,
                    self.compositor.screen_rect().iheight() * 9 / 10 - popup.height() as i32,
                    popup.width(),
                    popup.height(),
                );
                self.fps_widget.set_osd_position(rect);
                if self.fps_widget.need_redraw() {
                    self.compositor.schedule(rect);
                }
            }
        }

        self.compositor.redraw(|display, rect| {
            display.rect(&rect, self.config.background_color.into());

            for (id, focused) in self.order.iter_back_to_front() {
                if let Some(window) = self.windows.get(&id) {
                    window.draw_title(
                        display,
                        &rect,
                        focused,
                        if focused {
                            &self.window_max
                        } else {
                            &self.window_max_unfocused
                        },
                        if focused {
                            &self.window_close
                        } else {
                            &self.window_close_unfocused
                        },
                    );
                    window.draw(display, &rect);
                }
            }

            if let Some(popup) = &popup {
                display
                    .roi_mut(popup_rect.as_ref().unwrap())
                    .blend(&popup.roi(&Rect::new(0, 0, popup.width(), popup.height())));
            }

            if let Some((image, rect)) = self.fps_widget.get_rendered_osd() {
                display.roi_mut(rect).blend(&image.roi(&Rect::new(
                    0,
                    0,
                    image.width(),
                    image.height(),
                )));
            }
        });

        self.fps_widget.end_measure();
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

        let mut selectable_windows: Vec<WindowId> = vec![];
        for id in self.order.focus_order() {
            if let Some(window) = self.windows.get(&id) {
                if !window.title.is_empty() {
                    selectable_windows.push(id);
                }
            }
        }

        if selectable_windows.len() > 1 {
            // Disable dragging
            self.dragging = DragMode::None;

            // remove focus from the first selectable window in the window stack and make it
            // the last selectable window in the stack. Indexes are the indexes of windows
            // in self.order
            if let Some(id) = self.order.focused() {
                self.focus(id, false);
            }

            self.order
                .move_focused_after(selectable_windows[selectable_windows.len() - 1]);

            if let Some(id) = self.order.focused() {
                self.focus(id, true);
            }
        }
    }

    // Called by redraw() to draw the list of currently open windows in the middle of the screen.
    // Filter out app windows with no title.
    // If there are no windows to select, nothing is drawn.
    fn draw_window_list_osd(&mut self) -> Option<Image> {
        const SELECT_POPUP_TOP_BOTTOM_MARGIN: u32 = 2;
        const SELECT_POPUP_SIDE_MARGIN: i32 = 4;
        const SELECT_ROW_HEIGHT: u32 = 20;
        const SELECT_ROW_WIDTH: u32 = 400;
        const FONT_HEIGHT: f32 = 16.0;

        //TODO: HiDPI

        let selectable_window_ids: Vec<WindowId> = self
            .order
            .focus_order()
            .filter(|id| {
                if let Some(window) = self.windows.get(id) {
                    !window.title.is_empty()
                } else {
                    false
                }
            })
            .collect();

        if selectable_window_ids.len() <= 1 {
            return None;
        }

        // follow the look of the current config - in terms of colors
        let Config {
            bar_color,
            bar_highlight_color,
            text_color,
            text_highlight_color,
            ..
        } = *self.config;

        let list_h = selectable_window_ids.len() as u32 * SELECT_ROW_HEIGHT
            + (SELECT_POPUP_TOP_BOTTOM_MARGIN * 2);
        let list_w = SELECT_ROW_WIDTH;
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

        Some(image)
    }

    // Draw an on screen display (overlay) for volume control
    fn draw_volume_osd(&mut self) -> Image {
        let Config {
            bar_color,
            bar_highlight_color,
            ..
        } = *self.config;

        const BAR_HEIGHT: u32 = 20;
        const BAR_WIDTH: u32 = 100;
        const POPUP_MARGIN: u32 = 2;

        //TODO: HiDPI
        let list_h = BAR_HEIGHT + (2 * POPUP_MARGIN);
        let list_w = BAR_WIDTH + (2 * POPUP_MARGIN);
        // Color copied over from orbtk's window background
        let mut image = Image::from_color(list_w, list_h, bar_color.into());
        image.rect(
            2,
            2,
            self.volume_value as u32,
            BAR_HEIGHT as u32,
            bar_highlight_color.into(),
        );

        image
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
        if let Some(id) = self.order.focused() {
            if let Some(window) = self.windows.get_mut(&id) {
                let display_width = self.compositor.screen_rect().iwidth();
                let display_height = self.compositor.screen_rect().iheight();
                Self::update_window(&mut self.compositor, window, |_compositor, window| {
                    // Align location to grid
                    window.x -= window.x % GRID_SIZE;
                    window.y -= window.y % GRID_SIZE;

                    window.x += h_movement;
                    window.y += v_movement;

                    // Ensure window remains visible
                    window.x = cmp::max(
                        -window.iwidth() + GRID_SIZE,
                        cmp::min(display_width - GRID_SIZE, window.x),
                    );
                    window.y = cmp::max(
                        -window.iheight() + GRID_SIZE,
                        cmp::min(display_height - GRID_SIZE, window.y),
                    );

                    let move_event = MoveEvent {
                        x: window.x,
                        y: window.y,
                    }
                    .to_event();
                    window.event(move_event);
                });
            }
        }
    }

    fn clipboard_event(&mut self, kind: ClipboardAction) {
        if let Some(id) = self.order.focused() {
            if let Some(window) = self.windows.get_mut(&id) {
                let size = if matches!(kind, ClipboardAction::Paste) {
                    self.clipboard.len().saturating_sub(1)
                } else {
                    0
                };
                //TODO: set window's clipboard to primary
                let clipboard_event = ClipboardEvent { kind, size }.to_event();
                window.event(clipboard_event);
            }
        }
    }

    fn quit_front_window(&mut self) {
        if let Some(id) = self.order.focused() {
            if let Some(window) = self.windows.get_mut(&id) {
                window.event(QuitEvent.to_event());
            }
        }
    }

    /// Tile the focused window to a defined position.
    fn tile_focused_window(&mut self, position: TilePosition) {
        if let Some(id) = self.order.focused() {
            Self::tile_window(&mut self.compositor, &mut self.windows, id, position);
        }
    }

    fn tile_window(
        compositor: &mut Compositor,
        windows: &mut BTreeMap<WindowId, Window>,
        window_id: WindowId,
        position: TilePosition,
    ) {
        if let Some(window) = windows.get_mut(&window_id) {
            Self::update_window(compositor, window, |compositor, window| {
                let (x, y, width, height) = match window.restore.take() {
                    None => {
                        // we are about to maximize window, so store current size for restore later
                        window.restore = Some((window.rect(), position));

                        let screen_rect = compositor.get_screen_rect_for_window(&window.rect());
                        let window_rect = if matches!(position, TilePosition::FullScreen) {
                            screen_rect
                        } else {
                            compositor.get_window_rect_from_screen_rect(&screen_rect)
                        };
                        let top = window_rect.top() + window.title_rect().iheight();
                        let left = window_rect.left();
                        let max_height = window_rect.height() - window.title_rect().height();
                        let max_width = window_rect.width();
                        let half_width = (max_width / 2) as u32;
                        let half_height = (max_height / 2) as u32;

                        match position {
                            TilePosition::LeftHalf => (left, top, half_width, max_height as u32),
                            TilePosition::RightHalf => {
                                (left + half_width as i32, top, half_width, max_height as u32)
                            }
                            TilePosition::TopHalf => (left, top, max_width as u32, half_height),
                            TilePosition::BottomHalf => (
                                left,
                                top + half_height as i32,
                                max_width as u32,
                                half_height,
                            ),
                            TilePosition::Maximized | TilePosition::FullScreen => {
                                (left, top, max_width as u32, max_height as u32)
                            }
                        }
                    }
                    Some((restore, _)) => (
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
                window.send_resize_event(width, height);
            });
        }
    }

    // undraw any overlay that was being displayed and exit the mode causing it to be displayed
    fn close_overlays(&mut self) {
        // disable drawing of the win-tab or volume popup or shortcuts overlay on redraw
        self.win_tabbing = false;
        self.volume_osd = false;
        self.shortcuts_widget.enabled = false;
    }

    // Process incoming key events
    fn key_event(&mut self, event: KeyEvent) {
        self.track_modifier_state(event.scancode, event.pressed);

        match (event.scancode, event.pressed) {
            (orbclient::K_SUPER, true) => self.shortcuts_widget.enabled = true,
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
                orbclient::K_NUM_0 => self.cursor_simulate_enabled = !self.cursor_simulate_enabled,
                orbclient::K_BRACE_OPEN => self.volume(Volume::Down),
                orbclient::K_BRACE_CLOSE => self.volume(Volume::Up),
                orbclient::K_BACKSLASH => self.volume(Volume::Toggle),
                orbclient::K_M => self.tile_focused_window(TilePosition::Maximized),
                orbclient::K_ENTER => self.tile_focused_window(TilePosition::Maximized),
                orbclient::K_UP if shift => self.tile_focused_window(TilePosition::TopHalf),
                orbclient::K_DOWN if shift => self.tile_focused_window(TilePosition::BottomHalf),
                orbclient::K_LEFT if shift => self.tile_focused_window(TilePosition::LeftHalf),
                orbclient::K_RIGHT if shift => self.tile_focused_window(TilePosition::RightHalf),
                orbclient::K_UP => self.move_front_window(0, -GRID_SIZE),
                orbclient::K_DOWN => self.move_front_window(0, GRID_SIZE),
                orbclient::K_LEFT => self.move_front_window(-GRID_SIZE, 0),
                orbclient::K_RIGHT => self.move_front_window(GRID_SIZE, 0),
                orbclient::K_C => self.clipboard_event(ClipboardAction::Copy),
                orbclient::K_X => self.clipboard_event(ClipboardAction::Cut),
                orbclient::K_V => self.clipboard_event(ClipboardAction::Paste),
                orbclient::K_F10 => {
                    self.compositor.toggle_damage_border();
                }
                orbclient::K_F12 => {
                    if let Some(damage) = self.fps_widget.toggle_enabled() {
                        self.compositor.schedule(damage);
                    }
                }
                _ => {
                    //TODO: send all modifier instead by repurposing unused 'character' then remove this hack
                    if let Some((id, window)) = self.windows.iter_mut().next() {
                        info!("sending super {:?} to {}, {}", event, id.0, window.title);
                        let mut super_event = event.to_event();
                        super_event.code += 0x1000_0000;
                        window.event(super_event);
                    }
                }
            }
        }

        if self.cursor_simulate_enabled && self.simulate_mouse_event(&event) {
            return;
        }

        // send non-Super key events to the front window
        if self.modifier_state & SUPER_MODIFIER == 0 {
            if let Some(id) = self.order.focused() {
                if let Some(window) = self.windows.get_mut(&id) {
                    // TODO: ALT GR mapping is not handled
                    if event.pressed
                        && event.character != '\0'
                        && self.modifier_state & (CONTROL_MODIFIER | ALT_MODIFIER) == 0
                    {
                        let text_input_event = TextInputEvent {
                            character: event.character,
                        }
                        .to_event();
                        window.event(text_input_event);
                    }
                    // TODO: Remove event.character or repurpose it to send all modifiers
                    window.event(
                        KeyEvent {
                            character: '\0',
                            pressed: event.pressed,
                            scancode: event.scancode,
                        }
                        .to_event(),
                    );
                }
            }
        }
    }

    fn simulate_mouse_event(&mut self, event: &KeyEvent) -> bool {
        match (event.scancode, event.pressed) {
            (orbclient::K_NUM_4, true) => self.mouse_event(MouseEvent {
                x: self.cursor_x - self.cursor_simulate_speed,
                y: self.cursor_y,
            }),
            (orbclient::K_NUM_2, true) => self.mouse_event(MouseEvent {
                x: self.cursor_x,
                y: self.cursor_y + self.cursor_simulate_speed,
            }),
            (orbclient::K_NUM_8, true) => self.mouse_event(MouseEvent {
                x: self.cursor_x,
                y: self.cursor_y - self.cursor_simulate_speed,
            }),
            (orbclient::K_NUM_6, true) => self.mouse_event(MouseEvent {
                x: self.cursor_x + self.cursor_simulate_speed,
                y: self.cursor_y,
            }),
            (orbclient::K_NUM_3, true) => {
                if self.cursor_simulate_speed > 2 {
                    self.cursor_simulate_speed /= 2;
                }
            }
            (orbclient::K_NUM_9, true) => {
                if self.cursor_simulate_speed <= 128 {
                    self.cursor_simulate_speed *= 2;
                }
            }
            (orbclient::K_NUM_5, _) => self.button_event(ButtonEvent {
                left: event.pressed,
                middle: false,
                right: false,
            }),
            (orbclient::K_NUM_7, _) => self.button_event(ButtonEvent {
                left: false,
                middle: event.pressed,
                right: false,
            }),
            (orbclient::K_NUM_1, _) => self.button_event(ButtonEvent {
                left: false,
                middle: false,
                right: event.pressed,
            }),
            _ => return false,
        }
        true
    }

    fn mouse_event(&mut self, event: MouseEvent) {
        let mut new_cursor = CursorKind::LeftPtr;
        let mut new_hover = None;

        // Check for focus switch, dragging, and forward mouse events to applications
        match self.dragging {
            DragMode::None => {
                for id in self.order.iter_front_to_back() {
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
                                if window.scalable {
                                    window_event.a = window_event.a * (SCALE_BASELINE as i64)
                                        / window.factored_scale as i64;
                                    window_event.b = window_event.b * (SCALE_BASELINE as i64)
                                        / window.factored_scale as i64;
                                }
                                window.event(window_event);
                            }
                            break;
                        } else if window.title_rect().contains(event.x, event.y) {
                            break;
                        } else {
                            let cursor_mode = match (
                                window
                                    .border_rect(RectEdge::Left)
                                    .contains(self.cursor_x, self.cursor_y),
                                window
                                    .border_rect(RectEdge::Right)
                                    .contains(self.cursor_x, self.cursor_y),
                                window
                                    .border_rect(RectEdge::Bottom)
                                    .contains(self.cursor_x, self.cursor_y),
                            ) {
                                (true, false, false) => Some(CursorKind::LeftSide),
                                (false, true, false) => Some(CursorKind::RightSide),
                                (false, false, true) => Some(CursorKind::BottomSide),
                                (true, false, true) => Some(CursorKind::BottomLeftCorner),
                                (false, true, true) => Some(CursorKind::BottomRightCorner),
                                (_, _, _) => None,
                            };
                            if let Some(cusor_mode) = cursor_mode {
                                new_cursor = cusor_mode;
                                break;
                            }
                        }
                    }
                }
            }
            DragMode::Title(window_id, drag_x, drag_y) => {
                if let Some(window) = self.windows.get_mut(&window_id) {
                    if drag_x != event.x || drag_y != event.y {
                        Self::update_window(&mut self.compositor, window, |_compositor, window| {
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
                        });
                    }
                } else {
                    self.dragging = DragMode::None;
                }
            }
            DragMode::TopBorder(window_id, off_y, bottom_y) => {
                if let Some(window) = self.windows.get_mut(&window_id) {
                    new_cursor = CursorKind::BottomSide; // TODO

                    let y = event.y - off_y;
                    let h = bottom_y - y;

                    if h > 0 {
                        if y != window.y {
                            Self::update_window(
                                &mut self.compositor,
                                window,
                                |_compositor, window| {
                                    window.y = y;
                                    window.event(MoveEvent { x: window.x, y }.to_event());
                                },
                            );
                        }

                        if h != window.iheight() {
                            Self::update_window(
                                &mut self.compositor,
                                window,
                                |_compositor, window| {
                                    window.send_resize_event(window.width(), h as u32);
                                },
                            );
                        }
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
                            Self::update_window(
                                &mut self.compositor,
                                window,
                                |_compositor, window| {
                                    window.x = x;
                                    window.event(MoveEvent { x, y: window.y }.to_event());
                                },
                            );
                        }

                        if w != window.iwidth() {
                            Self::update_window(
                                &mut self.compositor,
                                window,
                                |_compositor, window| {
                                    window.send_resize_event(w as u32, window.height());
                                },
                            );
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
                    if w > 0 && w != window.iwidth() {
                        Self::update_window(&mut self.compositor, window, |_compositor, window| {
                            window.send_resize_event(w as u32, window.height());
                        });
                    }
                } else {
                    self.dragging = DragMode::None;
                }
            }
            DragMode::BottomBorder(window_id, off_y) => {
                if let Some(window) = self.windows.get_mut(&window_id) {
                    new_cursor = CursorKind::BottomSide;
                    let h = event.y - off_y - window.y;
                    if h > 0 && h != window.iheight() {
                        Self::update_window(&mut self.compositor, window, |_compositor, window| {
                            window.send_resize_event(window.width(), h as u32);
                        });
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
                            Self::update_window(
                                &mut self.compositor,
                                window,
                                |_compositor, window| {
                                    window.x = x;
                                    window.event(MoveEvent { x, y: window.y }.to_event());
                                },
                            );
                        }

                        if w != window.iwidth() || h != window.iheight() {
                            Self::update_window(
                                &mut self.compositor,
                                window,
                                |_compositor, window| {
                                    window.send_resize_event(w as u32, h as u32);
                                },
                            );
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
                    if w > 0 && h > 0 && (w != window.iwidth() || h != window.iheight()) {
                        Self::update_window(&mut self.compositor, window, |_compositor, window| {
                            window.send_resize_event(w as u32, h as u32);
                        });
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

        self.update_cursor(event.x, event.y, new_cursor);
    }

    fn mouse_relative_event(&mut self, event: MouseRelativeEvent) {
        let mut relative_cursor_opt = None;
        if let Some(id) = self.order.focused() {
            if let Some(window) = self.windows.get_mut(&id) {
                //TODO: handle grab?
                if window.mouse_relative {
                    // Send relative event
                    window.event(event.to_event());

                    // Update cursor to center of this window
                    relative_cursor_opt = Some((
                        window.x + window.iwidth() / 2,
                        window.y + window.iheight() / 2,
                        //TODO: allow cursors on relative windows?
                        CursorKind::None,
                    ));
                }
            }
        }

        // Handle relative window cursor
        if let Some((x, y, kind)) = relative_cursor_opt {
            self.update_cursor(x, y, kind);
            return;
        }

        //TODO: more advanced logic for keeping mouse on screen.
        // This logic assumes horizontal and touching, but not overlapping, screens
        let mut max_x = 0;
        let mut max_y = 0;
        for display in self.compositor.displays() {
            let rect = display.screen_rect();
            max_x = cmp::max(max_x, rect.right() - 1);
            max_y = cmp::max(max_y, rect.bottom() - 1);
        }

        let x = cmp::max(0, cmp::min(max_x, self.cursor_x + event.dx));
        let mut y = cmp::max(0, cmp::min(max_y, self.cursor_y + event.dy));
        for display in self.compositor.displays() {
            let rect = display.screen_rect();
            if x >= rect.left() && x < rect.right() {
                y = cmp::max(rect.top(), cmp::min(rect.bottom() - 1, y));
            }
        }

        self.mouse_event(MouseEvent { x, y });
    }

    fn button_event(&mut self, event: ButtonEvent) {
        // Check for focus switch, dragging, and forward mouse events to applications
        let focus = match self.dragging {
            DragMode::None => self.button_event_initiate(event),
            _ if self.dragging_window_initiated => {
                // If drag request is initiated from window, keep sending button events
                let focus = self.button_event_initiate(event);
                if !event.left {
                    self.dragging_window_initiated = false;
                    self.dragging = DragMode::None;
                }
                focus
            }
            _ => {
                if !event.left {
                    self.dragging = DragMode::None;
                }
                None
            }
        };

        if let Some(focus) = focus
            && self.order.focused() != Some(focus)
        {
            // Redraw old focused window
            if let Some(id) = self.order.focused() {
                self.focus(id, false);
            }

            // Reorder windows
            if self.windows.get(&focus).unwrap().zorder != WindowZOrder::Back {
                // Transfer focus if a front or normal window
                self.order.make_focused(focus);
            }

            // Redraw new focused window
            if let Some(id) = self.order.focused() {
                self.focus(id, true);
            }
        }

        self.cursor_left = event.left;
        self.cursor_middle = event.middle;
        self.cursor_right = event.right;
    }

    /// Handles first button events from user, returns WindowId that we think it should focus on.
    /// May mutate self.dragging
    fn button_event_initiate(&mut self, event: ButtonEvent) -> Option<WindowId> {
        for id in self.order.iter_front_to_back() {
            let Some(window) = self.windows.get(&id) else {
                continue;
            };
            if window.rect().contains(self.cursor_x, self.cursor_y) {
                if self.modifier_state & SUPER_MODIFIER == SUPER_MODIFIER {
                    if event.left && !self.cursor_left {
                        self.dragging = DragMode::Title(id, self.cursor_x, self.cursor_y);
                        return Some(id);
                    }
                } else if let Some(window) = self.windows.get_mut(&id) {
                    window.event(event.to_event());
                    if event.left && !self.cursor_left
                        || event.middle && !self.cursor_middle
                        || event.right && !self.cursor_right
                    {
                        return Some(id);
                    }
                }
                break;
            } else if window.title_rect().contains(self.cursor_x, self.cursor_y) {
                let on_max_btn =
                    window.resizable && window.max_contains(self.cursor_x, self.cursor_y);
                let on_close_btn =
                    !window.unclosable && window.close_contains(self.cursor_x, self.cursor_y);
                // pressed down
                if event.left && !self.cursor_left {
                    if !on_max_btn && !on_close_btn {
                        self.dragging = DragMode::Title(id, self.cursor_x, self.cursor_y);
                    }
                }
                // releasing up
                if !event.left && self.cursor_left {
                    if on_max_btn {
                        Self::tile_window(
                            &mut self.compositor,
                            &mut self.windows,
                            id,
                            TilePosition::Maximized,
                        );
                    } else if on_close_btn {
                        if let Some(window) = self.windows.get_mut(&id) {
                            window.event(QuitEvent.to_event());
                        }
                    }
                }
                return Some(id);
            } else {
                let dragging = match (
                    window
                        .border_rect(RectEdge::Left)
                        .contains(self.cursor_x, self.cursor_y),
                    window
                        .border_rect(RectEdge::Right)
                        .contains(self.cursor_x, self.cursor_y),
                    window
                        .border_rect(RectEdge::Bottom)
                        .contains(self.cursor_x, self.cursor_y),
                ) {
                    (true, false, false) => Some(DragMode::LeftBorder(
                        id,
                        self.cursor_x - window.x,
                        window.x + window.iwidth(),
                    )),
                    (false, true, false) => Some(DragMode::RightBorder(
                        id,
                        self.cursor_x - (window.x + window.iwidth()),
                    )),
                    (false, false, true) => Some(DragMode::BottomBorder(
                        id,
                        self.cursor_y - (window.y + window.iheight()),
                    )),
                    (true, false, true) => Some(DragMode::BottomLeftBorder(
                        id,
                        self.cursor_x - window.x,
                        self.cursor_y - (window.y + window.iheight()),
                        window.x + window.iwidth(),
                    )),
                    (false, true, true) => Some(DragMode::BottomRightBorder(
                        id,
                        self.cursor_x - (window.x + window.iwidth()),
                        self.cursor_y - (window.y + window.iheight()),
                    )),
                    (_, _, _) => None,
                };
                if let Some(dragging) = dragging {
                    if event.left && !self.cursor_left {
                        self.dragging = dragging;
                        return Some(id);
                    }
                    break;
                }
            }
        }
        None
    }

    fn resize_if_necessary(&mut self) {
        let old_scale = self.compositor.factored_scale();

        if !self.compositor.resize_if_necessary() {
            return;
        }

        let screen_event = ScreenEvent {
            width: self.compositor.screen_rect().width() as u32,
            height: self.compositor.screen_rect().height() as u32,
        }
        .to_event();
        for (_window_id, window) in self.windows.iter_mut() {
            window.event(screen_event);
        }

        if old_scale != self.compositor.factored_scale() {
            let scale_event = ScaleEvent {
                scale: self.compositor.factored_scale() as i32,
                baseline: SCALE_BASELINE as i32,
            }
            .to_event();
            for (_window_id, window) in self.windows.iter_mut() {
                if window.scalable {
                    window.scale = self.compositor.scale();
                    window.factored_scale = self.compositor.factored_scale();
                    window.event(scale_event);
                }
            }
        }
    }

    fn event(&mut self, event_union: Event) {
        self.order
            .rezbuffer(&|id| self.windows.get(&id).unwrap().zorder);

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
                    x: x * self.compositor.screen_rect().iwidth() / 65536,
                    y: y * self.compositor.screen_rect().iheight() / 65536,
                });
            }
            EventOption::MouseRelative(event) => self.mouse_relative_event(event),
            EventOption::Button(event) => self.button_event(event),
            EventOption::Scroll(_)
            | EventOption::ControllerAxis(_)
            | EventOption::ControllerButton(_) => {
                if let Some(id) = self.order.iter_front_to_back().next() {
                    if let Some(window) = self.windows.get_mut(&id) {
                        window.event(event_union);
                    }
                }
            }
            event => error!("unexpected event: {:?}", event),
        }
    }

    fn window_new(
        &mut self,
        x: i32,
        y: i32,
        mut width: u32,
        mut height: u32,
        flags: &str,
        title: String,
    ) -> Result<WindowId> {
        let id = WindowId(self.next_id as usize);
        self.next_id += 1;
        if self.next_id < 0 {
            //TODO: should this be an error?
            self.next_id = 1;
        }

        // Unfocus previous top window
        if let Some(id) = self.order.focused() {
            self.focus(id, false);
        }

        // Resize to fit allowed screen area
        let screen_rect = self.compositor.screen_rect();
        let allow_rect = self
            .compositor
            .get_window_rect_from_screen_rect(&screen_rect);

        let flags = WindowFlags::from_str(flags).unwrap_or_else(|e| {
            warn!("unknown window flags: {e:?} from {flags:?}");
            // attempt to recover working flags
            let flags = flags.as_bytes();
            let Some(Ok(flags)) = flags
                .get(..flags.len().saturating_sub(e.len()))
                .map(core::str::from_utf8)
            else {
                return WindowFlags::default();
            };
            WindowFlags::from_str(flags).unwrap_or_default()
        });
        if flags.contains(WindowFlag::Resizable) {
            width = width.min(allow_rect.width());
            height = height.min(allow_rect.height());
        }

        let mut window = Window::new(
            x,
            y,
            width,
            height,
            self.compositor.scale(),
            Rc::clone(&self.config),
        );

        for flag in flags {
            window.set_flag(flag, true);
        }

        window.title = title;
        window.render_title(&self.font);
        let scalable = flags.contains(WindowFlag::Scalable) && self.compositor.scale() > 1;

        // Automatic placement
        if x < 0 && y < 0 {
            // Center by default in allowed area
            let mut scale = 1;
            if scalable {
                scale = self.compositor.scale();
            }
            let center_x = cmp::max(0, (allow_rect.iwidth() - (width * scale) as i32) / 2);
            let center_y = cmp::max(
                window.title_rect().iheight(),
                (allow_rect.iheight() - (height * scale) as i32) / 2,
            ) as i32;
            window.x = center_x;
            window.y = center_y;

            // Process overlaps
            let mut overlap = true;
            let mut attempts = 0;
            while overlap {
                overlap = false;
                let cascade_rect = window.cascade_rect();
                for other_id in self.order.focus_order() {
                    let Some(other) = self.windows.get(&other_id) else {
                        continue;
                    };

                    // Ignore windows not shown on the same level
                    if other.hidden || other.zorder != window.zorder {
                        continue;
                    }

                    // Ignore windows not colliding in cascade region
                    if cascade_rect.intersection(&other.cascade_rect()).is_empty() {
                        continue;
                    }

                    // Adjust position by cascading region size
                    overlap = true;
                    window.x += cascade_rect.iwidth();
                    window.y += cascade_rect.iheight();

                    // Reset X or Y if beyond the screen size
                    if window.x + window.iwidth() > screen_rect.iwidth() {
                        window.x = 0;
                    }
                    if window.y + window.iheight() > screen_rect.iheight() {
                        window.y = window.title_rect().iheight();
                    }

                    // Give up if we ran out of places to try
                    attempts += 1;
                    if attempts > 1000 {
                        window.x = center_x;
                        window.y = center_y;
                        overlap = false;
                    }
                    break;
                }
            }
        }

        // Redraw new window
        self.compositor.schedule(window.title_rect());
        self.compositor.schedule(window.rect());

        // Add to zorder as appropriate
        self.order.add_window(id, window.zorder);

        if scalable {
            window.factored_scale = self.compositor.factored_scale();
            window.event(
                ScaleEvent {
                    scale: self.compositor.factored_scale() as _,
                    baseline: SCALE_BASELINE as _,
                }
                .to_event(),
            );
        } else {
            // needed by winit to send the first redraw event
            window.event(ResizeEvent { height, width }.to_event());
        }

        self.windows.insert(id, window);

        // Focus new top window
        if let Some(id) = self.order.focused() {
            self.focus(id, true);
        }

        // Ensure mouse cursor is correct
        self.mouse_event(MouseEvent {
            x: self.cursor_x,
            y: self.cursor_y,
        });

        Ok(id)
    }
}
