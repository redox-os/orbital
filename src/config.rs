use std::fs::File;
use std::io::Read;
use toml;

#[derive(Default, Deserialize)]
#[serde(default)]
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
    pub window_close_unfocused: String
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

        match toml::from_str(&string) {
            Ok(config) => config,
            Err(err) => {
                println!("orbital: failed to parse config '{}': {}", path, err);
                Config::default()
            }
        }
    }
}
