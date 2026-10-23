//! The keyboard map. One table both handles the app-wide keys and draws the
//! ⌘/ overlay, so what's listed is what works. The menu bar routes through the
//! same [`Cmd`]s.
//!
//! Avoided on purpose: ⇧⌘3/4/5 (screenshots), ⌘H/⌘M/⌘Q/⌘W (the system's),
//! ⌘C/⌘V/⌘X/⌘A/⌘Z (text editing), ⌘Space (Spotlight).

use egui::{Key, Modifiers};

/// Something the keyboard or the menu bar asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmd {
    PlayPause,
    Next,
    Previous,
    VolumeUp,
    VolumeDown,
    SeekForward,
    SeekBack,
    /// Love (star) the playing track.
    Love,
    /// Go to the playing track's album and select it.
    Reveal,
    Search,
    Back,
    Forward,
    /// A sidebar place, by position (0 = Recently Added).
    Place(u8),
    ToggleQueue,
    Full,
    Compact,
    Mini,
    ToggleMini,
    Settings,
    Shortcuts,
    NewPlaylist,
    ToggleBitPerfect,
    /// System / Light / Dark (menu only).
    Appearance(u8),
}

/// A key and what it does. `idle` keys only fire when no text field has focus
/// (Space, ?, plain ⌘-arrows that a field uses for the cursor).
pub struct Binding {
    pub group: &'static str,
    pub label: &'static str,
    /// As shown: ⌘ ⌥ ⇧ then the key.
    pub keys: &'static str,
    /// `None` for keys a view handles itself (listed for reference).
    pub key: Option<(Modifiers, Key)>,
    pub idle: bool,
    pub cmd: Option<Cmd>,
}

const fn b(group: &'static str, label: &'static str, keys: &'static str, mods: Modifiers, key: Key, idle: bool, cmd: Cmd) -> Binding {
    Binding { group, label, keys, key: Some((mods, key)), idle, cmd: Some(cmd) }
}

/// Listed in the overlay; handled where they apply (lists, the queue, the palette).
const fn view(group: &'static str, label: &'static str, keys: &'static str) -> Binding {
    Binding { group, label, keys, key: None, idle: false, cmd: None }
}

const CMD: Modifiers = Modifiers::COMMAND;
const CMD_SHIFT: Modifiers = Modifiers { shift: true, ..Modifiers::COMMAND };
const CMD_ALT: Modifiers = Modifiers { alt: true, ..Modifiers::COMMAND };
const NONE: Modifiers = Modifiers::NONE;
const SHIFT: Modifiers = Modifiers::SHIFT;

pub const GROUPS: [&str; 4] = ["Playback", "Go", "Lists & queue", "Window"];

pub static BINDINGS: &[Binding] = &[
    b("Playback", "Play / pause", "Space", NONE, Key::Space, true, Cmd::PlayPause),
    b("Playback", "Next track", "⌘→", CMD, Key::ArrowRight, true, Cmd::Next),
    b("Playback", "Previous track", "⌘←", CMD, Key::ArrowLeft, true, Cmd::Previous),
    b("Playback", "Skip ahead 10 s", "⌥⌘→", CMD_ALT, Key::ArrowRight, true, Cmd::SeekForward),
    b("Playback", "Skip back 10 s", "⌥⌘←", CMD_ALT, Key::ArrowLeft, true, Cmd::SeekBack),
    b("Playback", "Volume up", "⌘↑", CMD, Key::ArrowUp, true, Cmd::VolumeUp),
    b("Playback", "Volume down", "⌘↓", CMD, Key::ArrowDown, true, Cmd::VolumeDown),
    b("Playback", "Love playing track", "⇧⌘L", CMD_SHIFT, Key::L, false, Cmd::Love),
    b("Playback", "Bit-perfect on / off", "⇧⌘B", CMD_SHIFT, Key::B, false, Cmd::ToggleBitPerfect),
    b("Go", "Search", "⌘K", CMD, Key::K, false, Cmd::Search),
    b("Go", "Search", "⌘F", CMD, Key::F, false, Cmd::Search),
    view("Go", "Search", "/"),
    b("Go", "Show playing track", "⌘L", CMD, Key::L, false, Cmd::Reveal),
    b("Go", "Back", "⌘[", CMD, Key::OpenBracket, false, Cmd::Back),
    b("Go", "Forward", "⌘]", CMD, Key::CloseBracket, false, Cmd::Forward),
    b("Go", "Recently Added", "⌘1", CMD, Key::Num1, false, Cmd::Place(0)),
    b("Go", "Artists", "⌘2", CMD, Key::Num2, false, Cmd::Place(1)),
    b("Go", "Albums", "⌘3", CMD, Key::Num3, false, Cmd::Place(2)),
    b("Go", "Songs", "⌘4", CMD, Key::Num4, false, Cmd::Place(3)),
    b("Go", "Favorites", "⌘5", CMD, Key::Num5, false, Cmd::Place(4)),
    b("Go", "Offline", "⌘6", CMD, Key::Num6, false, Cmd::Place(5)),
    view("Lists & queue", "Select", "↑ ↓"),
    view("Lists & queue", "Play from here", "⏎"),
    view("Lists & queue", "Play next", "⌥⏎"),
    view("Lists & queue", "Add to queue", "⇧⏎"),
    view("Lists & queue", "Move in queue or playlist", "⌥↑ ⌥↓"),
    view("Lists & queue", "Remove from queue or playlist", "⌫"),
    b("Lists & queue", "New playlist", "⌘N", CMD, Key::N, false, Cmd::NewPlaylist),
    b("Window", "Show / hide queue", "⌥⌘U", CMD_ALT, Key::U, false, Cmd::ToggleQueue),
    b("Window", "Full window", "⌥⌘1", CMD_ALT, Key::Num1, false, Cmd::Full),
    b("Window", "Compact", "⌥⌘2", CMD_ALT, Key::Num2, false, Cmd::Compact),
    b("Window", "Mini player", "⌥⌘3", CMD_ALT, Key::Num3, false, Cmd::Mini),
    b("Window", "Mini player on / off", "⇧⌘M", CMD_SHIFT, Key::M, false, Cmd::ToggleMini),
    b("Window", "Settings", "⌘,", CMD, Key::Comma, false, Cmd::Settings),
    b("Window", "Keyboard shortcuts", "⌘/", CMD, Key::Slash, false, Cmd::Shortcuts),
    b("Window", "Keyboard shortcuts", "?", SHIFT, Key::Questionmark, true, Cmd::Shortcuts),
];

/// The display string for `cmd`'s first binding (for tooltips and menus).
pub fn keys_for(cmd: Cmd) -> Option<&'static str> {
    BINDINGS.iter().find(|b| b.cmd == Some(cmd)).map(|b| b.keys)
}

/// Consumes every bound key pressed this frame and returns their commands.
/// `typing`: a text field has focus, so `idle` keys belong to it.
pub fn take(input: &mut egui::InputState, typing: bool) -> Vec<Cmd> {
    let mut out = Vec::new();
    for b in BINDINGS {
        let (Some((mods, key)), Some(cmd)) = (b.key, b.cmd) else { continue };
        if b.idle && typing {
            continue;
        }
        // Exact modifiers: ⌘→ must not also fire for ⌥⌘→. `?` needs Shift on
        // some layouts and not others, so only ⌘ matters for it.
        let fits = |m: &Modifiers| if key == Key::Questionmark { !m.command } else { m.matches_exact(mods) };
        let hit = input.events.iter().any(|e| matches!(e, egui::Event::Key { key: k, pressed: true, modifiers, .. } if *k == key && fits(modifiers)));
        if hit {
            input.events.retain(|e| !matches!(e, egui::Event::Key { key: k, pressed: true, modifiers, .. } if *k == key && fits(modifiers)));
            out.push(cmd);
        }
    }
    // A `?` typed with Shift arrives as text too; keep it out of any field.
    if out.contains(&Cmd::Shortcuts) {
        input.events.retain(|e| !matches!(e, egui::Event::Text(t) if t == "?" || t == "/"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn no_key_is_bound_twice() {
        let mut seen = HashSet::new();
        for b in BINDINGS {
            if let Some((m, k)) = b.key {
                assert!(seen.insert((m.command, m.alt, m.shift, m.ctrl, k)), "{} bound twice", b.keys);
            }
            assert!(GROUPS.contains(&b.group), "{} has an unknown group", b.label);
        }
    }

    fn press(key: Key, modifiers: Modifiers, typing: bool) -> Vec<Cmd> {
        let ev = egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers };
        let raw = egui::RawInput { events: vec![ev], ..Default::default() };
        let ctx = egui::Context::default();
        let mut out = Vec::new();
        let mut full = ctx.run_ui(raw, |ui| out = ui.ctx().input_mut(|i| take(i, typing)));
        full.textures_delta.clear();
        out
    }

    #[test]
    fn exact_modifiers_pick_the_right_command() {
        assert_eq!(press(Key::ArrowRight, CMD, false), [Cmd::Next]);
        assert_eq!(press(Key::ArrowRight, CMD_ALT, false), [Cmd::SeekForward]);
        assert_eq!(press(Key::L, CMD, false), [Cmd::Reveal]);
        assert_eq!(press(Key::L, CMD_SHIFT, false), [Cmd::Love]);
        assert_eq!(press(Key::Questionmark, SHIFT, false), [Cmd::Shortcuts]);
        // Space and ⌘-arrows belong to a focused text field.
        assert!(press(Key::Space, NONE, true).is_empty());
        assert!(press(Key::ArrowRight, CMD, true).is_empty());
        assert_eq!(press(Key::K, CMD, true), [Cmd::Search]);
    }
}
