use orbclient::{Renderer, image::Image};
use orbfont::Font;

use crate::config::Config;

pub struct ShortcutsWidget {
    pub enabled: bool,
    cached: Option<Image>,
    cached_scale: u32,
}

const SHORTCUTS_LIST: &[&str] = &[
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
    "Super-Numpad-0: Enable mouse accessibility keys using numpad",
    "Super-F10: Enable damage borders on screen",
    "Super-F12: Enable FPS counter on screen",
];

impl ShortcutsWidget {
    pub fn new() -> Self {
        Self {
            enabled: false,
            cached: None,
            cached_scale: 0,
        }
    }
    pub fn draw_osd<'a>(
        &'a mut self,
        scale: u32,
        config: &Config,
        font: &Font,
    ) -> Option<&'a Image> {
        if !self.enabled {
            return None;
        }
        if self.cached_scale == scale {
            return self.cached.as_ref();
        }
        self.cached.replace(Self::generate_osd(scale, config, font));
        self.cached_scale = scale;
        self.cached.as_ref()
    }

    fn generate_osd(scale: u32, config: &Config, font: &Font) -> Image {
        let row_height: u32 = 20 * scale;
        let row_width: u32 = 400 * scale;
        let popup_border: u32 = 2 * scale;
        let font_height: f32 = 16.0 * scale as f32;

        // follow the look of the current config - in terms of colors
        let Config {
            bar_color,
            bar_highlight_color,
            text_highlight_color,
            ..
        } = *config;

        let list_h = SHORTCUTS_LIST.len() as u32 * row_height + (popup_border * 2);
        let list_w = row_width;
        let mut image = Image::from_color(list_w, list_h, bar_color.into());

        for (index, shortcut) in SHORTCUTS_LIST.iter().enumerate() {
            let vertical_offset = index as i32 * row_height as i32 + popup_border as i32;
            let text = font.render(shortcut, font_height);
            image.rect(
                0,
                vertical_offset,
                list_w as u32,
                row_height,
                bar_highlight_color.into(),
            );
            text.draw(
                &mut image,
                popup_border as i32,
                vertical_offset + popup_border as i32,
                text_highlight_color.into(),
            );
        }

        image
    }
}
