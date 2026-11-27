//! The keyboard: every action and the key that does it, editable in the Keys window and kept between runs.
//! A binding is a key with Shift and / or Ctrl, or none. For moves, Shift and Ctrl held with an unmodified
//! binding change the step (as before); for the other actions the modifiers must match exactly, so that e.g.
//! Shift+F3 (laser on) and F3 (laser off) are told apart.

use eframe::egui::{self, Key};
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Act {
    Left,
    Right,
    Up,
    Down,
    ZUp,
    ZDown,
    ZUp2,
    ZDown2,
    Stop,
    Light,
    LaserOn,
    LaserOff,
    Focus,
    Snap,
    Follow,
    Fit,
    Explore,
    Select,
    Lattice,
    Cancel,
    Done,
}

pub const ALL: [Act; 21] = [
    Act::Left, Act::Right, Act::Up, Act::Down, Act::ZUp, Act::ZDown, Act::ZUp2, Act::ZDown2, Act::Stop, Act::Light, Act::LaserOn,
    Act::LaserOff, Act::Focus, Act::Snap, Act::Follow, Act::Fit, Act::Explore, Act::Select, Act::Lattice, Act::Cancel, Act::Done,
];

impl Act {
    pub fn id(self) -> &'static str {
        match self {
            Act::Left => "left",
            Act::Right => "right",
            Act::Up => "up",
            Act::Down => "down",
            Act::ZUp => "z_up",
            Act::ZDown => "z_down",
            Act::ZUp2 => "z_up_2",
            Act::ZDown2 => "z_down_2",
            Act::Stop => "stop",
            Act::Light => "light",
            Act::LaserOn => "laser_on",
            Act::LaserOff => "laser_off",
            Act::Focus => "focus",
            Act::Snap => "snap",
            Act::Follow => "follow",
            Act::Fit => "fit",
            Act::Explore => "explore",
            Act::Select => "select",
            Act::Lattice => "lattice",
            Act::Cancel => "cancel",
            Act::Done => "done",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Act::Left => "Stage: left (Shift: 1/10, Ctrl: a whole field)",
            Act::Right => "Stage: right",
            Act::Up => "Stage: up",
            Act::Down => "Stage: down",
            Act::ZUp => "Z up one step (Shift: five)",
            Act::ZDown => "Z down one step (Shift: five)",
            Act::ZUp2 => "Z up, second key",
            Act::ZDown2 => "Z down, second key",
            Act::Stop => "Stop (stage, scan, laser off)",
            Act::Light => "Light on / off",
            Act::LaserOn => "Laser on",
            Act::LaserOff => "Laser off",
            Act::Focus => "Find focus here",
            Act::Snap => "Snap a frame",
            Act::Follow => "Follow the stage",
            Act::Fit => "Show the whole sample",
            Act::Explore => "Tool: Explore",
            Act::Select => "Tool: Select",
            Act::Lattice => "Tool: Lattice",
            Act::Cancel => "Cancel (a selection being drawn, Move map)",
            Act::Done => "Done (Move map)",
        }
    }

    /// Shift / Ctrl change the step of these instead of being part of the key.
    fn step_mods(self) -> bool {
        matches!(self, Act::Left | Act::Right | Act::Up | Act::Down | Act::ZUp | Act::ZDown | Act::ZUp2 | Act::ZDown2)
    }

    fn default_bind(self) -> Option<Bind> {
        let k = |key: Key| Some(Bind { key, shift: false, ctrl: false });
        match self {
            Act::Left => k(Key::ArrowLeft),
            Act::Right => k(Key::ArrowRight),
            Act::Up => k(Key::ArrowUp),
            Act::Down => k(Key::ArrowDown),
            Act::ZUp => k(Key::Period),
            Act::ZDown => k(Key::Comma),
            Act::ZUp2 => k(Key::PageUp),
            Act::ZDown2 => k(Key::PageDown),
            Act::Stop => k(Key::Space),
            Act::Light => k(Key::F2),
            Act::LaserOn => Some(Bind { key: Key::F3, shift: true, ctrl: false }),
            Act::LaserOff => k(Key::F3),
            Act::Focus => None,
            Act::Snap => None,
            Act::Follow => k(Key::F),
            Act::Fit => k(Key::Home),
            Act::Explore => k(Key::E),
            Act::Select => k(Key::S),
            Act::Lattice => k(Key::L),
            Act::Cancel => k(Key::Escape),
            Act::Done => k(Key::Enter),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Bind {
    pub key: Key,
    pub shift: bool,
    pub ctrl: bool,
}

impl Bind {
    pub fn text(&self) -> String {
        format!("{}{}{}", if self.ctrl { "Ctrl+" } else { "" }, if self.shift { "Shift+" } else { "" }, self.key.name())
    }

    fn parse(s: &str) -> Option<Bind> {
        let (mut shift, mut ctrl, mut rest) = (false, false, s);
        loop {
            if let Some(r) = rest.strip_prefix("Ctrl+") {
                ctrl = true;
                rest = r;
            } else if let Some(r) = rest.strip_prefix("Shift+") {
                shift = true;
                rest = r;
            } else {
                break;
            }
        }
        Key::from_name(rest).map(|key| Bind { key, shift, ctrl })
    }
}

pub struct Keys {
    map: HashMap<Act, Option<Bind>>,
}

impl Keys {
    pub fn defaults() -> Keys {
        Keys { map: ALL.iter().map(|a| (*a, a.default_bind())).collect() }
    }

    /// From what was saved: {"laser_on": "Shift+F3", "snap": "", ...}; missing actions keep their defaults.
    pub fn load(s: Option<String>) -> Keys {
        let mut k = Keys::defaults();
        if let Some(m) = s.and_then(|s| serde_json::from_str::<HashMap<String, String>>(&s).ok()) {
            for a in ALL {
                if let Some(v) = m.get(a.id()) {
                    k.map.insert(a, if v.is_empty() { None } else { Bind::parse(v).or(a.default_bind()) });
                }
            }
        }
        k
    }

    pub fn save(&self) -> String {
        let m: HashMap<&str, String> = ALL.iter().map(|a| (a.id(), self.get(*a).map(|b| b.text()).unwrap_or_default())).collect();
        serde_json::to_string(&m).unwrap_or_default()
    }

    pub fn get(&self, a: Act) -> Option<Bind> {
        self.map.get(&a).copied().flatten()
    }

    pub fn set(&mut self, a: Act, b: Option<Bind>) {
        self.map.insert(a, b);
    }

    pub fn reset(&mut self, a: Act) {
        self.map.insert(a, a.default_bind());
    }

    /// The key of an action, for buttons: "S", "Shift+F3", or "" when it has none.
    pub fn text(&self, a: Act) -> String {
        self.get(a).map(|b| b.text()).unwrap_or_default()
    }

    /// Was the action's key pressed in this frame?
    pub fn pressed(&self, ui: &egui::Ui, a: Act) -> bool {
        let Some(b) = self.get(a) else { return false };
        ui.input(|i| {
            if !i.key_pressed(b.key) {
                return false;
            }
            let (shift, ctrl) = (i.modifiers.shift, i.modifiers.command);
            if (b.shift && !shift) || (b.ctrl && !ctrl) {
                return false;
            }
            a.step_mods() || ((shift == b.shift) && (ctrl == b.ctrl))
        })
    }

    /// Actions sharing a binding with another one.
    pub fn clashes(&self) -> Vec<Act> {
        ALL.iter()
            .filter(|a| self.get(**a).is_some_and(|b| ALL.iter().any(|o| o != *a && self.get(*o) == Some(b))))
            .copied()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bindings_round_trip() {
        let mut k = Keys::defaults();
        k.set(Act::Snap, Some(Bind { key: Key::N, shift: false, ctrl: true }));
        k.set(Act::Follow, None);
        let k2 = Keys::load(Some(k.save()));
        assert_eq!(k2.text(Act::Snap), "Ctrl+N");
        assert_eq!(k2.text(Act::Follow), "");
        assert_eq!(k2.text(Act::LaserOn), "Shift+F3");
        assert!(k2.clashes().is_empty());
    }
}
