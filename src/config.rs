use std::fs::File;
use std::io::Read;
use toml;

#[derive(Default, Deserialize, Clone)]
#[serde(default)]
struct TmpConfig {
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

    pub background_color: String,
    pub bar_color: String,
    pub bar_highlight_color: String,
    pub text_color: String,
    pub text_highlight_color: String,
}

impl Into<Config> for TmpConfig {
    fn into(self) -> Config {
        Config {
            cursor: self.cursor.clone(),
            bottom_left_corner: self.bottom_left_corner.clone(),
            bottom_right_corner: self.bottom_right_corner.clone(),

            left_side: self.left_side.clone(),
            right_side: self.right_side.clone(),
            bottom_side: self.bottom_side.clone(),

            window_max: self.window_max.clone(),
            window_max_unfocused: self.window_max_unfocused.clone(),
            window_close: self.window_close.clone(),
            window_close_unfocused: self.window_close_unfocused.clone(),

            background_color: parse_colour(&self.background_color).unwrap_or(orbclient::Color::rgb(0, 0, 0)),
            bar_color: parse_colour(&self.bar_color).unwrap_or(orbclient::Color::rgba(47, 52, 63, 224)),
            bar_highlight_color: parse_colour(&self.bar_highlight_color).unwrap_or(orbclient::Color::rgba(80, 86, 102, 224)),
            text_color: parse_colour(&self.text_color).unwrap_or(orbclient::Color::rgb(204, 210, 224)),
            text_highlight_color: parse_colour(&self.text_highlight_color).unwrap_or(orbclient::Color::rgb(204, 210, 224)),
        }
    }
}

#[derive(Clone)]
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

    pub background_color: orbclient::Color,
    pub bar_color: orbclient::Color,
    pub bar_highlight_color: orbclient::Color,
    pub text_color: orbclient::Color,
    pub text_highlight_color: orbclient::Color,
}

impl TmpConfig {
    fn from_path(path: &str) -> TmpConfig {
        let mut string = String::new();

        match File::open(path) {
            Ok(mut file) => match file.read_to_string(&mut string) {
                Ok(_) => (),
                Err(err) => println!("orbital: failed to read config '{}': {}", path, err),
            },
            Err(err) => println!("orbital: failed to open config '{}': {}", path, err),
        }

        match toml::from_str(&string) {
            Ok(config) => config,
            Err(err) => {
                println!("orbital: failed to parse config '{}': {}", path, err);
                TmpConfig::default()
            }
        }
    }
}

impl Config {
    pub fn from_path(path: &str) -> Config {
        TmpConfig::from_path(path).into()
    }
}

/// Parse ARGB colours from TOML file
fn parse_colour(colour: &str) -> Result<orbclient::Color, String> {
    let chars: Vec<char> = colour.chars().collect();
    
    if chars.len() == 9 && chars[0] == '#' {
        let channels: &[String; 4] = &[
            chars[1..3].iter().collect(),
            chars[3..5].iter().collect(),
            chars[5..7].iter().collect(),
            chars[7..9].iter().collect(),
        ];
        let channels: &[u8;4] = &[
            u8::from_str_radix(&channels[0], 16).map_err(|err| err.to_string())?,
            u8::from_str_radix(&channels[1], 16).map_err(|err| err.to_string())?,
            u8::from_str_radix(&channels[2], 16).map_err(|err| err.to_string())?,
            u8::from_str_radix(&channels[3], 16).map_err(|err| err.to_string())?,
        ];

        Ok(orbclient::Color::rgba(
            channels[1],
            channels[2],
            channels[3],
            channels[0],
        ))
    } else {
        Err(format!("{} is not a valid colour", colour))
    }
}
