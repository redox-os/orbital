extern crate event;
extern crate orbclient;
extern crate rand;

use event::EventQueue;
use orbclient::{Color, EventOption, Renderer, Window, WindowFlag};
use rand::{Rng, rngs::ThreadRng};
use std::{
    collections::BTreeMap,
    fmt,
    os::unix::io::AsRawFd,
};

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
struct Key {
    r: u8,
    g: u8,
    b: u8,
}

impl Key {
    fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    fn color(&self) -> Color {
        Color::rgb(self.r, self.g, self.b)
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }
}

struct Example {
    event_queue: EventQueue<Key>,
    windows: BTreeMap<Key, Window>,
    rng: ThreadRng,
}

impl Example {
    fn new() -> Self {
        Self {
            event_queue: EventQueue::new().unwrap(),
            windows: BTreeMap::new(),
            rng: rand::thread_rng(),
        }
    }

    fn create(&mut self) {
        for _attempt in 0..16 {
            let key = Key::new(
                self.rng.gen_range(64..192),
                self.rng.gen_range(64..192),
                self.rng.gen_range(64..192)
            );

            if self.windows.contains_key(&key) {
                eprintln!("{} already exists", key);
                break;
            }

            let mut window = Window::new_flags(
                -1,
                -1,
                640,
                480,
                &format!("Window {}", key),
                &[
                    WindowFlag::Async,
                    WindowFlag::Resizable,
                ]
            )
            .unwrap();

            window.set(key.color());
            window.sync();

            self.event_queue.add(window.as_raw_fd(), move |_fd_event| {
                Ok(Some(key))
            }).unwrap();

            self.windows.insert(key, window);

            return;
        }
    }

    fn run(&mut self) {
        while ! self.windows.is_empty() {
            let key = self.event_queue.run().unwrap();

            let mut create = false;
            let mut remove = false;
            if let Some(window) = self.windows.get_mut(&key) {
                for event in window.events() {
                    let event_option = event.to_option();
                    println!("{}: {:?}", key, event_option);
                    match event_option {
                        EventOption::Key(key_event) => if key_event.pressed {
                            match key_event.character {
                                'n' => create = true,
                                'q' => remove = true,
                                _ => (),
                            }
                        },
                        EventOption::Quit(_quit_event) => remove = true,
                        EventOption::Resize(_resize_event) => {
                            window.set(key.color());
                            window.sync();
                        },
                        _ => (),
                    }
                }
            } else {
                println!("{}: failed to find window", key);
            }

            if create {
                self.create();
            }

            if remove {
                self.windows.remove(&key);
            }
        }
    }
}

fn main() {
    let mut example = Example::new();
    example.create();
    example.run();
}
