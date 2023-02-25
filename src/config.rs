use std::fs::File;
use std::io::Read;
use toml;

#[derive(Default, Debug, Copy, Clone)]
struct TmpColor {
    pub data: u32,
}

struct TmpColorVisitor;
impl<'de> serde::de::Visitor<'de> for TmpColorVisitor {
    type Value = TmpColor;

    fn visit_str<E>(self, v: &str) -> Result<Self::Value, E> where E: serde::de::Error {
        if v.len() == 9 && v.chars().next().unwrap() == '#' {
            let chars: &[char] = v.chars().collect();
            let parts: [String;4] = [
                &chars[1..3].into_iter.collect(), 
                &chars[3..5].into_iter.collect(), 
                &chars[5..7].into_iter.collect(), 
                &chars[7..9].into_iter.collect()
            ];

            Ok(TmpColor::default)
        } else {
            Err(serde::de::Error::invalid_value())
        }
    }
}

impl<'de> serde::Deserialize<'de> for TmpColor {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error> where D: serde::Deserializer<'de> {
        deserializer.deserialize_string()
    }
}

impl Into<orbclient::Color> for TmpColor {
    fn into(self) -> orbclient::Color {
        orbclient::Color { data: self.data }
    }
}

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

    pub background_color: TmpColor,
    pub bar_color: TmpColor,
    pub bar_highlight_color: TmpColor,
    pub text_color: TmpColor,
    pub text_highlight_color: TmpColor,
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

            background_color: self.background_color.into(),
            bar_color: self.bar_color.into(),
            bar_highlight_color: self.bar_highlight_color.into(),
            text_color: self.text_color.into(),
            text_highlight_color: self.text_highlight_color.into()
        }
    }
}

#[derive(Default, Clone)]
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
