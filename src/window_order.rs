use std::collections::VecDeque;

use crate::window::WindowId;

#[derive(Copy, Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum WindowZOrder {
    Back,
    Normal,
    Front,
}

pub(crate) struct WindowOrder {
    focus_order: VecDeque<WindowId>,
    zbuffer: Vec<(WindowId, WindowZOrder, bool)>,
}

impl WindowOrder {
    pub(crate) fn new() -> WindowOrder {
        WindowOrder {
            focus_order: VecDeque::new(),
            zbuffer: Vec::new(),
        }
    }

    pub(crate) fn add_window(&mut self, id: WindowId, zorder: WindowZOrder) {
        match zorder {
            WindowZOrder::Front | WindowZOrder::Normal => {
                self.focus_order.push_front(id);
            }
            WindowZOrder::Back => {
                self.focus_order.push_back(id);
            }
        }
    }

    pub(crate) fn remove_window(&mut self, id: WindowId) {
        self.focus_order.retain(|&e| e != id);
    }

    pub(crate) fn make_focused(&mut self, id: WindowId) {
        let index = self.focus_order.iter().position(|&e| e == id).unwrap();
        self.focus_order.remove(index).unwrap();
        self.focus_order.push_front(id);
    }

    pub(crate) fn move_focused_after(&mut self, id: WindowId) {
        let after_index = self.focus_order.iter().position(|&e| e == id).unwrap();
        let front_id = self.focus_order.pop_front().unwrap();
        self.focus_order.insert(after_index, front_id);
    }

    pub(crate) fn rezbuffer(&mut self, get_zorder: &dyn Fn(WindowId) -> WindowZOrder) {
        self.zbuffer.clear();

        for (i, &id) in self.focus_order.iter().enumerate() {
            self.zbuffer.push((id, get_zorder(id), i == 0));
        }

        self.zbuffer.sort_by(|a, b| b.1.cmp(&a.1));
    }

    pub(crate) fn focused(&self) -> Option<WindowId> {
        self.focus_order.front().copied()
    }

    pub(crate) fn focus_order(&self) -> impl Iterator<Item = WindowId> {
        self.focus_order.iter().copied()
    }

    pub(crate) fn iter_front_to_back(&self) -> impl Iterator<Item = WindowId> {
        self.zbuffer.iter().map(|&(id, _, _)| id)
    }

    pub(crate) fn iter_back_to_front(&self) -> impl Iterator<Item = (WindowId, bool)> {
        self.zbuffer
            .iter()
            .map(|&(id, _, focused)| (id, focused))
            .rev()
    }
}
