use std::time::Instant;

use orbclient::Renderer;
use orbfont::Font;

use crate::{
    config::Config,
    core::{image::Image, rect::Rect},
};

pub struct FpsWidget {
    pub enabled: bool,
    fps_counted: u64,
    fps_cputime: u64,
    fps_measured: String,
    fps_instant: Instant,
    fps_popup_image: Option<Image>,
    fps_popup_rect: Option<Rect>,
    fps_measure_instant: Option<Instant>,
}

impl FpsWidget {
    pub fn new() -> Self {
        Self {
            enabled: false,
            fps_measured: "-".to_string(),
            fps_counted: 0,
            fps_cputime: 0,
            fps_instant: Instant::now(),
            fps_popup_image: None,
            fps_popup_rect: None,
            fps_measure_instant: None,
        }
    }

    pub fn start_measure(&mut self) {
        if self.enabled {
            self.fps_measure_instant = Some(Instant::now())
        }
    }

    pub fn end_measure(&mut self) {
        if self.enabled {
            if let Some(time) = self.fps_measure_instant.take() {
                self.fps_cputime += time.elapsed().as_micros() as u64;
            }
        }
    }

    pub fn draw_fps_osd(&mut self, scale: i32, config: &Config, font: &Font) -> Option<&Image> {
        if !self.enabled {
            return None;
        }

        let fps_totaltime = self.fps_instant.elapsed().as_micros() as u64;
        self.fps_counted += 1;
        // update atleast every 330ms
        if self.fps_cputime > 0 && fps_totaltime > 330_000 {
            self.fps_measured = format!(
                // uncomment to debug (adjust row_width as well)
                // "{} F for {}ms of {}ms = {} FPS {}% CPU",
                // self.fps_counted,
                // self.fps_cputime / 1000,
                // fps_totaltime / 1000,
                "{} FPS {}% CPU",
                self.fps_counted * 1_000_000 / fps_totaltime,
                self.fps_cputime * 100 / fps_totaltime,
            );
            self.fps_counted = 0;
            self.fps_cputime = 0;
            self.fps_instant = std::time::Instant::now();
        } else {
            return None;
        }

        let Config {
            bar_color,
            bar_highlight_color,
            text_highlight_color,
            ..
        } = &config;

        let row_width: i32 = 120 * scale;
        let popup_border: u32 = 5 * scale as u32;
        let font_height: f32 = (18 * scale) as f32;
        let row_height: i32 = 18 * scale + 8;

        let mut image = Image::from_color(row_width, row_height, (*bar_color).into());
        let text = font.render(&self.fps_measured, font_height);
        image.rect(
            0,
            popup_border as i32,
            row_width as u32,
            row_height as u32,
            (*bar_highlight_color).into(),
        );
        text.draw(
            &mut image,
            popup_border as i32,
            popup_border as i32,
            (*text_highlight_color).into(),
        );

        self.fps_popup_image = Some(image);
        self.fps_popup_image.as_ref()
    }

    pub fn set_osd_position(&mut self, rect: Rect) {
        self.fps_popup_rect = Some(rect);
    }

    pub fn get_rendered_osd(&self) -> Option<(&Image, &Rect)> {
        if !self.enabled {
            return None;
        }

        match (&self.fps_popup_image, &self.fps_popup_rect) {
            (Some(img), Some(rect)) => Some((img, rect)),
            _ => None,
        }
    }

    pub fn toggle_enabled(&mut self) -> Option<Rect> {
        self.enabled = !self.enabled;

        self.fps_counted = 0;
        self.fps_cputime = 0;
        self.fps_instant = std::time::Instant::now();
        self.fps_popup_image = None;
        self.fps_popup_rect.take()
    }
}
