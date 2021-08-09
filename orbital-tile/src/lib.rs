#[derive(Clone, Copy, Debug)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Debug)]
pub struct Fork {
    pub parent: Option<Id>,
    pub pos: Position,
    pub size: Size,
    pub orient: Orientation,
    pub a: Option<Id>,
    pub b: Option<Id>,
}

impl Fork {
    pub fn new(parent: Option<Id>, pos: Position, size: Size, orient: Orientation, a: Option<Id>, b: Option<Id>) -> Self {
        Self {
            parent,
            pos,
            size,
            orient,
            a,
            b
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Id(pub u64);

#[derive(Clone, Copy, Debug)]
pub enum Orientation {
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy, Debug)]
pub struct Position {
    pub x: i64,
    pub y: i64,
}

impl Position {
    pub fn new(x: i64, y: i64) -> Self {
        Self { x, y }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Size {
    pub w: i64,
    pub h: i64,
}

impl Size {
    pub fn new(w: i64, h: i64) -> Self {
        Self { w, h }
    }
}
