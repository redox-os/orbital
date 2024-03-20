use std::fs::File;
use std::io::Read;
use log::{debug, error};
use serde_derive::Deserialize;
use orbclient::Color;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize)]
pub struct ConfigColor { data: u32 }

impl From<ConfigColor> for Color {
    fn from(value: ConfigColor) -> Self {
        Self { data: value.data }
    }
}
impl From<Color> for ConfigColor {
    fn from(value: Color) -> Self {
        Self { data: value.data }
    }
}

#[derive(Deserialize, Clone)]
pub struct Config {
    pub cursor: String,
    pub bottom_left_corner: String,
    pub bottom_right_corner: String,
    pub bottom_side: String,
    pub left_side: String,
    pub right_side: String,
    pub window_max: String,
    pub window_max_unfocused: String,
    pub window_close: String,
    pub window_close_unfocused: String,

    #[serde(default = "background_color_default")]
    pub background_color: ConfigColor,
    #[serde(default = "bar_color_default")]
    pub bar_color: ConfigColor,
    #[serde(default = "bar_highlight_color_default")]
    pub bar_highlight_color: ConfigColor,
    #[serde(default = "text_color_default")]
    pub text_color: ConfigColor,
    #[serde(default = "text_highlight_color_default")]
    pub text_highlight_color: ConfigColor,
}

fn background_color_default() -> ConfigColor { Color::rgb(0, 0, 0).into() }
fn bar_color_default() -> ConfigColor { Color::rgba(47, 52, 63, 224).into() }
fn bar_highlight_color_default() -> ConfigColor { Color::rgba(80, 86, 102, 224).into() }
fn text_color_default() -> ConfigColor { Color::rgb(204, 210, 224).into() }
fn text_highlight_color_default() -> ConfigColor { Color::rgb(204, 210, 224).into() }

/// Create a sane default Orbital [Config] in case none is supplied or it is unreadable
impl Default for Config {
    fn default() -> Self {
        // Cannot use "..Default::default() for all these fields as that is recursive, so they
        // all have to be "defaulted" manually.
        Config {
            // TODO: What would be good or better defaults for these config values?
            cursor: String::default(),
            bottom_left_corner: String::default(),
            bottom_right_corner: String::default(),
            bottom_side: String::default(),
            left_side: String::default(),
            right_side: String::default(),
            window_max: String::default(),
            window_max_unfocused: String::default(),
            window_close: String::default(),
            window_close_unfocused: String::default(),

            // These are the default colors for Orbital that have been defined
            background_color: background_color_default(),
            bar_color: bar_color_default(),
            bar_highlight_color: bar_highlight_color_default(),
            text_color: text_color_default(),
            text_highlight_color: text_highlight_color_default(),
        }
    }
}

/// [Config] holds configuration information for Orbital, such as colors, cursors etc.
impl Config {
    // returns the default config if the string passed is not a valid config
    fn config_from_string(config: &str) -> Config {
        match toml::from_str(config) {
            Ok(config) => config,
            Err(err) => {
                error!("failed to parse config '{}'", err);
                Config::default()
            }
        }
    }

    /// Read an Orbital configuration from a toml file at `path`
    pub fn from_path(path: &str) -> Config {
        let mut string = String::new();

        match File::open(path) {
            Ok(mut file) => match file.read_to_string(&mut string) {
                Ok(_) => debug!("reading config from path: '{}'", path),
                Err(err) => error!("failed to read config '{}': {}", path, err),
            },
            Err(err) => error!("failed to open config '{}': {}", path, err),
        }

        Self::config_from_string(&string)
    }
}

#[cfg(test)]
mod test {
    use crate::config::{background_color_default, Config, text_highlight_color_default};

    #[test]
    fn non_existent_config_file() {
        let config = Config::from_path("no-such-file.toml");
        assert_eq!(config.cursor, "");
        assert_eq!(config.text_highlight_color, text_highlight_color_default());
    }

    #[test]
    fn partial_config() {
        let config_str = r##"
            background_color = "#FFFFFFFF"
        "##;
        let config = Config::config_from_string(config_str);
        assert_eq!(config.background_color, background_color_default());
    }

    #[test]
    fn valid_partial_config() {
        let config_str = r##"cursor = "/ui/left_ptr.png"
bottom_left_corner = "/ui/bottom_left_corner.png"
bottom_right_corner = "/ui/bottom_right_corner.png"
bottom_side = "/ui/bottom_side.png"
left_side = "/ui/left_side.png"
right_side = "/ui/right_side.png"
window_max = "/ui/window_max.png"
window_max_unfocused = "/ui/window_max_unfocused.png"
window_close = "/ui/window_close.png"
window_close_unfocused = "/ui/window_close_unfocused.png""##;
        let config = Config::config_from_string(config_str);
        assert_eq!(config.background_color, background_color_default());
        assert_eq!(config.bottom_left_corner, "/ui/bottom_left_corner.png");
    }
}
