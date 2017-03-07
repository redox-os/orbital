use std::fs::File;
use std::io::Read;

pub struct Config {
    pub background: Vec<String>,
    pub background_mode: String,
    pub cursor: String,
    pub bottom_right_corner: String,
    pub bottom_side: String,
    pub right_side: String,
    pub window_close: String,
    pub window_close_unfocused: String,
}

impl Config {
    pub fn from_path(path: &str) -> Config {
        let mut string = String::new();

        match File::open(path) {
            Ok(mut file) => match file.read_to_string(&mut string) {
                Ok(_) => (),
                Err(err) => println!("orbital: failed to read config '{}': {}", path, err)
            },
            Err(err) => println!("orbital: failed to open config '{}': {}", path, err)
        }

        Config::from_str(&string)
    }

    pub fn from_str(string: &str) -> Config {
        let mut config = Config {
            background: Vec::new(),
            background_mode: String::new(),
            cursor: String::new(),
            bottom_right_corner: String::new(),
            bottom_side: String::new(),
            right_side: String::new(),
            window_close: String::new(),
            window_close_unfocused: String::new(),
        };

        for line_original in string.lines() {
            let line = line_original.trim();
            if line.starts_with("background=") {
                config.background.push(line[11..].to_string());
            }
            if line.starts_with("background_mode=") {
                config.background_mode = line[16..].to_string();
            }
            if line.starts_with("cursor=") {
                config.cursor = line[7..].to_string();
            }
            if line.starts_with("bottom_right_corner=") {
                config.bottom_right_corner = line[20..].to_string();
            }
            if line.starts_with("bottom_side=") {
                config.bottom_side = line[12..].to_string();
            }
            if line.starts_with("right_side=") {
                config.right_side = line[11..].to_string();
            }
            if line.starts_with("window_close=") {
                config.window_close = line[13..].to_string();
            }
            if line.starts_with("window_close_unfocused=") {
                config.window_close_unfocused = line[23..].to_string();
            }
        }

        config
    }
}
