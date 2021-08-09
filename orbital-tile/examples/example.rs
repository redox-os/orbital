use log::{debug, info, error};
use orbclient::{Color, Renderer};
use orbital_tile::{
    Direction,
    Fork,
    Id,
    Orientation,
    Position,
    Size,
};
use rand::Rng;
use std::collections::{BTreeMap, VecDeque};

pub struct Window {
    pub parent: Option<Id>,
    pub pos: Position,
    pub size: Size,
    pub color: Color,
}

impl Window {
    pub fn new(parent: Option<Id>, pos: Position, size: Size, color: Color) -> Self {
        Self {
            parent,
            pos,
            size,
            color
        }
    }

    pub fn left(&self) -> i64 {
        self.pos.x
    }

    pub fn right(&self) -> i64 {
        self.pos.x + self.size.w
    }

    pub fn top(&self) -> i64 {
        self.pos.y
    }

    pub fn bottom(&self) -> i64 {
        self.pos.y + self.size.h
    }
}

pub struct Example {
    w: u32,
    h: u32,
    orbclient_window: orbclient::Window,
    running: bool,
    windows: BTreeMap<Id, Window>,
    order: Vec<Id>,
    forks: BTreeMap<Id, Fork>,
    next_id: u64,
    rng: rand::rngs::ThreadRng,
}

impl Example {
    pub fn new(w: u32, h: u32) -> Option<Self> {
        // Add topmost fork
        let mut forks = BTreeMap::new();
        forks.insert(Id(0), Fork::new(
            None,
            Position::new(0, 0),
            Size::new(w as i64, h as i64),
            Orientation::Horizontal,
            None,
            None
        ));
        Some(Self {
            w, h,
            orbclient_window: orbclient::Window::new(-1, -1, w, h, "orbital-tile")?,
            running: true,
            windows: BTreeMap::new(),
            order: Vec::new(),
            forks,
            next_id: 1,
            rng: rand::thread_rng(),
        })
    }

    pub fn draw(&mut self) {
        self.orbclient_window.set(Color::rgb(0, 0, 0));
        for id in self.order.iter() {
            if let Some(window) = self.windows.get(id) {
                self.orbclient_window.rect(
                    window.pos.x as i32, window.pos.y as i32,
                    window.size.w as u32, window.size.h as u32,
                    window.color
                );
            }
        }
        if let Some(id) = self.order.last() {
            if let Some(window) = self.windows.get(id) {
                let (x1, y1) = (
                    window.pos.x as i32,
                    window.pos.y as i32,
                );
                let (x2, y2) = (
                    x1 + (window.size.w as i32 - 1),
                    y1 + (window.size.h as i32 - 1),
                );
                let color = Color::rgb(255, 255, 255);
                self.orbclient_window.line(x1, y1, x2, y1, color);
                self.orbclient_window.line(x1, y1, x1, y2, color);
                self.orbclient_window.line(x1, y2, x2, y2, color);
                self.orbclient_window.line(x2, y1, x2, y2, color);
            }
        }
        self.orbclient_window.sync();
    }

    pub fn close_focused(&mut self) {
        let id = match self.order.pop() {
            Some(some) => some,
            None => return,
        };

        let window = match self.windows.remove(&id) {
            Some(some) => some,
            None => {
                error!("failed to remove window {:?}", id);
                return;
            }
        };

        debug!("close window {:?} {:?} {:?}", id, window.pos, window.size);

        let fork_id = match window.parent {
            Some(some) => some,
            None => return,
        };

        let mut resizes = VecDeque::new();
        let mut replace_opt = None;
        let remove = if let Some(fork) = self.forks.get_mut(&fork_id) {
            if let Some(other_id) = if fork.a == Some(id) {
                fork.a = None;
                fork.b.take()
            } else if fork.b == Some(id) {
                fork.b = None;
                fork.a.take()
            } else {
                error!("failed to find {:?} in fork {:?}", id, fork_id);
                return;
            } {
                if let Some(parent_id) = fork.parent {
                    replace_opt = Some((parent_id, other_id));
                    resizes.push_back((other_id, fork.pos, fork.size));
                    true
                } else {
                    fork.a = Some(other_id);
                    resizes.push_back((other_id, fork.pos, fork.size));
                    false
                }
            } else {
                // Do not remove top fork even if empty
                //TODO: can we work without special casing the top fork?
                fork_id != Id(0)
            }
        } else {
            error!("failed to find fork {:?}", fork_id);
            return;
        };

        if let Some((parent_id, other_id)) = replace_opt {
            debug!("replace {:?} with {:?} in {:?}", fork_id, other_id, parent_id);

            if let Some(fork) = self.forks.get_mut(&parent_id) {
                if fork.a == Some(fork_id) {
                    fork.a = Some(other_id);
                } else if fork.b == Some(fork_id) {
                    fork.b = Some(other_id);
                } else {
                    error!("failed to find {:?} in fork {:?}", fork_id, parent_id);
                }
            }

            if let Some(window) = self.windows.get_mut(&other_id) {
                window.parent = Some(parent_id);
            } else if let Some(fork) = self.forks.get_mut(&other_id) {
                fork.parent = Some(parent_id);
            } else {
                error!("failed to find window or fork {:?}", other_id);
            }
        }

        if remove {
            debug!("removing fork {:?}", fork_id);
            if self.forks.remove(&fork_id).is_none() {
                error!("failed to remove fork {:?}", fork_id);
                return;
            }
        }

        while let Some((other_id, pos, size)) = resizes.pop_front() {
            debug!("resizing {:?} to {:?} {:?}", other_id, pos, size);

            if let Some(window) = self.windows.get_mut(&other_id) {
                window.pos = pos;
                window.size = size;
            } else if let Some(fork) = self.forks.get_mut(&other_id) {
                fork.pos = pos;
                fork.size = size;

                if let Some(a) = fork.a {
                    let a_pos = pos;
                    let mut a_size = size;
                    if let Some(b) = fork.b {
                        let mut b_pos = pos;
                        let mut b_size = size;
                        //TODO: don't just forget relative sizes!
                        match fork.orient {
                            Orientation::Horizontal => {
                                a_size.w /= 2;
                                b_pos.x += a_size.w;
                                b_size.w = a_size.w;
                            },
                            Orientation::Vertical => {
                                a_size.h /= 2;
                                b_pos.y += a_size.h;
                                b_size.h = a_size.h;
                            },
                        }
                        resizes.push_back((b, b_pos, b_size));
                    }
                    resizes.push_back((a, a_pos, a_size));
                }

            } else {
                error!("failed to find window or fork {:?}", other_id);
            }
        }
    }

    pub fn create_focused(&mut self) {
        let id = Id(self.next_id);
        self.next_id += 1;

        let w = self.rng.gen_range(1..32) * 32;
        let h = self.rng.gen_range(1..32) * 32;
        let mut size = Size::new(w, h);

        let x = self.rng.gen_range(0..self.w as i64 - w);
        let y = self.rng.gen_range(0..self.h as i64 - h);
        let mut pos = Position::new(x, y);

        // Tile with most recent also tiled window
        let mut parent = None;
        for other_id in self.order.iter().rev() {
            let other = match self.windows.get_mut(&other_id) {
                Some(some) => some,
                None => continue,
            };

            let mut fork_id = match other.parent {
                Some(some) => some,
                None => continue,
            };

            let new_fork_opt = match self.forks.get_mut(&fork_id) {
                Some(fork) => if fork.b.is_some() {
                    // Create new fork if needed
                    let original_fork_id = fork_id;
                    fork_id = Id(self.next_id);
                    self.next_id += 1;

                    let orient = match fork.orient {
                        Orientation::Horizontal => Orientation::Vertical,
                        Orientation::Vertical => Orientation::Horizontal,
                    };

                    debug!("create fork {:?} {:?}", fork_id, orient);

                    other.parent = Some(fork_id);
                    if fork.a == Some(*other_id) {
                        fork.a = Some(fork_id);
                    } else if fork.b == Some(*other_id) {
                        fork.b = Some(fork_id);
                    } else {
                        error!("failed to find {:?} in fork {:?}", other_id, original_fork_id);
                    }

                    Some(Fork::new(
                        Some(original_fork_id),
                        other.pos,
                        other.size,
                        orient,
                        Some(*other_id),
                        None,
                    ))
                } else {
                    None
                },
                None => continue,
            };

            if let Some(new_fork) = new_fork_opt {
                self.forks.insert(fork_id, new_fork);
            }

            let fork = match self.forks.get_mut(&fork_id) {
                Some(some) => some,
                None => continue,
            };

            fork.b = Some(id);

            pos = other.pos;
            match fork.orient {
                Orientation::Horizontal => {
                    other.size.w /= 2;
                    pos.x += other.size.w;
                },
                Orientation::Vertical => {
                    other.size.h /= 2;
                    pos.y += other.size.h;
                }
            }
            size = other.size;

            parent = Some(fork_id);
            break;
        }

        if parent.is_none() {
            // Check topmost fork for room
            let fork_id = Id(0);
            if let Some(fork) = self.forks.get_mut(&fork_id) {
                if fork.a.is_none() {
                    fork.a = Some(id);

                    pos = fork.pos;
                    size = fork.size;

                    parent = Some(fork_id);
                } else {
                    info!("found no place for window, floating");
                }
            }
        }

        debug!("create window {:?} {:?} {:?} {:?}", id, parent, pos, size);

        self.windows.insert(id, Window::new(
            parent,
            pos,
            size,
            Color::rgb(
                self.rng.gen_range(64..192),
                self.rng.gen_range(64..192),
                self.rng.gen_range(64..192)
            )
        ));
        self.order.push(id);
    }

    //TODO: resolve without recursion
    pub fn dump_tree(&self, id: Id, level: usize) {
        if let Some(window) = self.windows.get(&id) {
            println!("{:indent$}window {:?} {:?} {:?}", "", id, window.pos, window.size, indent=4 * level);
        } else if let Some(fork) = self.forks.get(&id) {
            println!("{:indent$}fork {:?} {:?} {:?}", "", id, fork.pos, fork.size, indent=4 * level);
            if let Some(a) = fork.a {
                self.dump_tree(a, level + 1);
            }
            if let Some(b) = fork.b {
                self.dump_tree(b, level + 1);
            }
        } else {
            println!("{:indent$}{:?} not found", "", id, indent=4 * level);
        }
    }

    pub fn focus_direction(&mut self, direction: Direction) {
        debug!("focus {:?}", direction);

        let mut closest_dist = 0;
        let mut closest_id = None;
        if let Some(current_id) = self.order.last() {
            if let Some(current) = self.windows.get(&current_id) {
                let (current_left, current_right, current_top, current_bottom) = (
                    current.left(), current.right(), current.top(), current.bottom()
                );
                for id in self.order.iter() {
                    if id == current_id {
                        continue;
                    }
                    if let Some(window) = self.windows.get(&id) {
                        // The distance must be that of the shortest straight line that can be
                        // drawn from the current window, in the specified direction, to the window
                        // we are evaluating.
                        let (window_left, window_right, window_top, window_bottom) = (
                            window.left(), window.right(), window.top(), window.bottom()
                        );
                        // Window is not intersecting vertically
                        let out_of_bounds_vertical = || {
                            window_top >= current_bottom || window_bottom <= current_top
                        };
                        // Window is not intersecting horizontally
                        let out_of_bounds_horizontal = || {
                            window_left >= current_right || window_right <= current_left
                        };
                        let dist = match direction {
                            Direction::Left => {
                                if out_of_bounds_vertical() { continue; }
                                if window_right <= current_left {
                                    // To the left, with space
                                    current_left - window_right
                                } else if window_left <= current_left {
                                    // To the left, overlapping
                                    0
                                } else {
                                    // Not to the left, skipping
                                    continue;
                                }
                            },
                            Direction::Right => {
                                if out_of_bounds_vertical() { continue; }
                                if window_left >= current_right {
                                    // To the right, with space
                                    window_left - current_right
                                } else if window_right >= current_right {
                                    // To the right, overlapping
                                    0
                                } else {
                                    // Not to the right, skipping
                                    continue;
                                }
                            },
                            Direction::Up => {
                                if out_of_bounds_horizontal() { continue; }
                                if window_bottom <= current_top {
                                    // To the top, with space
                                    current_top - window_bottom
                                } else if window_top <= current_top {
                                    // To the top, overlapping
                                    0
                                } else {
                                    // Not to the top, skipping
                                    continue;
                                }
                            },
                            Direction::Down => {
                                if out_of_bounds_horizontal() { continue; }
                                if window_top >= current_bottom {
                                    // To the bottom, with space
                                    window_top - current_bottom
                                } else if window_bottom >= current_bottom {
                                    // To the bottom, overlapping
                                    0
                                } else {
                                    // Not to the bottom, skipping
                                    continue;
                                }
                            },
                        };
                        // Distance in wrong direction, skip
                        if dist < 0 { continue; }
                        if dist <= closest_dist || closest_id.is_none() {
                            closest_dist = dist;
                            closest_id = Some(*id);
                        }
                    }
                }
            }
        }

        if let Some(id) = closest_id {
            debug!("focusing {:?}", id);
            self.order.retain(|x| *x != id);
            self.order.push(id);
        }
    }

    pub fn events(&mut self) {
        for event in self.orbclient_window.events() {
            match event.to_option() {
                orbclient::EventOption::Key(key_event) => if key_event.pressed {
                    match key_event.character {
                        'h' => self.focus_direction(Direction::Left),
                        'j' => self.focus_direction(Direction::Down),
                        'k' => self.focus_direction(Direction::Up),
                        'l' => self.focus_direction(Direction::Right),
                        'n' => self.create_focused(),
                        'q' => self.close_focused(),
                        't' => self.dump_tree(Id(0), 0),
                        _ => (),
                    }
                },
                orbclient::EventOption::Quit(_) => {
                    self.running = false;
                },
                _ => (),
            }
        }
    }

    pub fn run(&mut self) {
        while self.running {
            self.draw();
            self.events();
        }
    }
}


fn main() {
    env_logger::builder()
        .filter_level(log::LevelFilter::Debug)
        .parse_default_env()
        .init();
    Example::new(1920, 1080).unwrap().run();
}
