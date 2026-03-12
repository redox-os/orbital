use std::cmp::{max, min};

#[derive(Copy, Clone, Debug, Default)]
pub struct Rect {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

impl Rect {
    pub fn new(x: i32, y: i32, w: i32, h: i32) -> Rect {
        assert!(w >= 0);
        assert!(h >= 0);

        Rect { x, y, w, h }
    }

    pub fn area(&self) -> i32 {
        self.w * self.h
    }

    pub fn left(&self) -> i32 {
        self.x
    }

    pub fn right(&self) -> i32 {
        self.x + self.w
    }

    pub fn top(&self) -> i32 {
        self.y
    }

    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }

    pub fn width(&self) -> i32 {
        self.w
    }

    pub fn height(&self) -> i32 {
        self.h
    }

    pub fn container(&self, other: &Rect) -> Rect {
        let left = self.left().min(other.left());
        let right = self.right().max(other.right());
        let top = self.top().min(other.top());
        let bottom = self.bottom().max(other.bottom());

        assert!(left <= right);
        assert!(top <= bottom);

        Rect::new(left, top, right - left, bottom - top)
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        self.left() <= x && self.right() >= x && self.top() <= y && self.bottom() >= y
    }

    pub fn is_empty(&self) -> bool {
        self.w == 0 || self.h == 0
    }

    pub fn intersection(&self, other: &Rect) -> Rect {
        let left = self.left().max(other.left());
        let right = self.right().min(other.right());
        let top = self.top().max(other.top());
        let bottom = self.bottom().min(other.bottom());

        if right < left || bottom < top {
            return Rect::new(0, 0, 0, 0);
        }

        Rect::new(left, top, right - left, bottom - top)
    }

    pub fn offset(&self, x: i32, y: i32) -> Rect {
        Rect::new(self.x + x, self.y + y, self.w, self.h)
    }
}
