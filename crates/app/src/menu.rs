//! The native macOS menu bar (via `muda`). Every item routes to a
//! [`Cmd`](crate::keys::Cmd), the same as the keyboard.
//!
//! Keys that a text field also uses (Space, ⌘-arrows) get no menu
//! accelerator: AppKit offers key equivalents to the menu before the view, so
//! the palette's field would lose ⌘← and friends. Those items show without a
//! key, and the keys stay with egui. Edit's items are AppKit's own; with no
//! responder for them in a winit view they disable themselves and let ⌘C/⌘V
//! through to egui.

use crate::keys::Cmd;

#[cfg(target_os = "macos")]
pub use imp::MenuBar;

#[cfg(not(target_os = "macos"))]
pub struct MenuBar;

#[cfg(not(target_os = "macos"))]
impl MenuBar {
    pub fn install(_post: impl Fn(Cmd) + Send + Sync + 'static) -> Option<Self> {
        None
    }
    pub fn sync(&mut self, _queue: bool, _bit_perfect: bool, _theme: u8) {}
}

/// Menu item id ↔ command.
fn id_of(cmd: Cmd) -> String {
    format!("{cmd:?}")
}

fn cmd_of(id: &str) -> Option<Cmd> {
    const ALL: &[Cmd] = &[
        Cmd::PlayPause,
        Cmd::Next,
        Cmd::Previous,
        Cmd::VolumeUp,
        Cmd::VolumeDown,
        Cmd::SeekForward,
        Cmd::SeekBack,
        Cmd::Love,
        Cmd::Reveal,
        Cmd::Search,
        Cmd::Back,
        Cmd::Forward,
        Cmd::ToggleQueue,
        Cmd::Lyrics,
        Cmd::TogglePlayer,
        Cmd::Full,
        Cmd::Player,
        Cmd::Mini,
        Cmd::ToggleMini,
        Cmd::Settings,
        Cmd::Shortcuts,
        Cmd::NewPlaylist,
        Cmd::ToggleBitPerfect,
        Cmd::Place(0),
        Cmd::Place(1),
        Cmd::Place(2),
        Cmd::Place(3),
        Cmd::Place(4),
        Cmd::Place(5),
        Cmd::Appearance(0),
        Cmd::Appearance(1),
        Cmd::Appearance(2),
    ];
    ALL.iter().copied().find(|c| id_of(*c) == id)
}

#[cfg(target_os = "macos")]
mod imp {
    use super::{cmd_of, id_of};
    use crate::keys::{Cmd, keys_for};
    use muda::accelerator::Accelerator;
    use muda::{AboutMetadata, CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};

    pub struct MenuBar {
        _menu: Menu,
        queue: CheckMenuItem,
        bit_perfect: CheckMenuItem,
        themes: [CheckMenuItem; 3],
        shown: Option<(bool, bool, u8)>,
    }

    /// The accelerator for `cmd`, from the key map ("⇧⌘L" → "Shift+Cmd+L").
    /// Keys a text field needs (no ⌘, or ⌘ with an arrow) get none.
    fn accel(cmd: Cmd) -> Option<Accelerator> {
        let keys = keys_for(cmd)?;
        let mut parts = Vec::new();
        let mut key = String::new();
        for ch in keys.chars() {
            match ch {
                '⇧' => parts.push("Shift"),
                '⌥' => parts.push("Alt"),
                '⌘' => parts.push("Cmd"),
                '→' | '←' | '↑' | '↓' => return None,
                c => key.push(c),
            }
        }
        if !parts.contains(&"Cmd") || key.is_empty() {
            return None;
        }
        let key = match key.as_str() {
            "," => "Comma".to_owned(),
            "/" => "Slash".to_owned(),
            "[" => "BracketLeft".to_owned(),
            "]" => "BracketRight".to_owned(),
            k => k.to_owned(),
        };
        format!("{}+{key}", parts.join("+")).parse().ok()
    }

    fn item(text: &str, cmd: Cmd) -> MenuItem {
        MenuItem::with_id(id_of(cmd), text, true, accel(cmd))
    }

    fn check(text: &str, cmd: Cmd) -> CheckMenuItem {
        CheckMenuItem::with_id(id_of(cmd), text, true, false, accel(cmd))
    }

    impl MenuBar {
        /// Builds the menu bar and hands it to NSApp. Call on the main thread
        /// once the app is running (from the app's constructor).
        pub fn install(post: impl Fn(Cmd) + Send + Sync + 'static) -> Option<Self> {
            let sep = PredefinedMenuItem::separator;
            let about = AboutMetadata {
                name: Some("Listener".into()),
                version: Some(env!("CARGO_PKG_VERSION").into()),
                comments: Some("A native player for Navidrome.".into()),
                ..Default::default()
            };
            let app = Submenu::with_items(
                "Listener",
                true,
                &[
                    &PredefinedMenuItem::about(Some("About Listener"), Some(about)),
                    &sep(),
                    &item("Settings…", Cmd::Settings),
                    &sep(),
                    &PredefinedMenuItem::services(None),
                    &sep(),
                    &PredefinedMenuItem::hide(None),
                    &PredefinedMenuItem::hide_others(None),
                    &PredefinedMenuItem::show_all(None),
                    &sep(),
                    &PredefinedMenuItem::quit(Some("Quit Listener")),
                ],
            )
            .ok()?;
            let file = Submenu::with_items("File", true, &[&item("New Playlist…", Cmd::NewPlaylist)]).ok()?;
            let edit = Submenu::with_items(
                "Edit",
                true,
                &[
                    &PredefinedMenuItem::undo(None),
                    &PredefinedMenuItem::redo(None),
                    &sep(),
                    &PredefinedMenuItem::cut(None),
                    &PredefinedMenuItem::copy(None),
                    &PredefinedMenuItem::paste(None),
                    &PredefinedMenuItem::select_all(None),
                    &sep(),
                    &item("Search…", Cmd::Search),
                ],
            )
            .ok()?;
            let bit_perfect = check("Bit-Perfect", Cmd::ToggleBitPerfect);
            let controls = Submenu::with_items(
                "Controls",
                true,
                &[
                    &item("Play / Pause", Cmd::PlayPause),
                    &item("Next", Cmd::Next),
                    &item("Previous", Cmd::Previous),
                    &item("Skip Ahead 10 Seconds", Cmd::SeekForward),
                    &item("Skip Back 10 Seconds", Cmd::SeekBack),
                    &sep(),
                    &item("Volume Up", Cmd::VolumeUp),
                    &item("Volume Down", Cmd::VolumeDown),
                    &sep(),
                    &item("Love", Cmd::Love),
                    &item("Show Playing Track", Cmd::Reveal),
                    &sep(),
                    &bit_perfect,
                ],
            )
            .ok()?;
            let queue = check("Show Queue", Cmd::ToggleQueue);
            let themes = [check("System", Cmd::Appearance(0)), check("Light", Cmd::Appearance(1)), check("Dark", Cmd::Appearance(2))];
            let appearance = Submenu::with_items("Appearance", true, &[&themes[0], &themes[1], &themes[2]]).ok()?;
            let go = Submenu::with_items(
                "Go",
                true,
                &[
                    &item("Back", Cmd::Back),
                    &item("Forward", Cmd::Forward),
                    &sep(),
                    &item("Recently Added", Cmd::Place(0)),
                    &item("Artists", Cmd::Place(1)),
                    &item("Albums", Cmd::Place(2)),
                    &item("Songs", Cmd::Place(3)),
                    &item("Favorites", Cmd::Place(4)),
                    &item("Offline", Cmd::Place(5)),
                ],
            )
            .ok()?;
            let view = Submenu::with_items(
                "View",
                true,
                &[
                    &item("Lyrics", Cmd::Lyrics),
                    &queue,
                    &sep(),
                    &item("Full Window", Cmd::Full),
                    &item("Player", Cmd::Player),
                    &item("Mini Player", Cmd::Mini),
                    &sep(),
                    &appearance,
                    &sep(),
                    &PredefinedMenuItem::fullscreen(None),
                ],
            )
            .ok()?;
            let window = Submenu::with_items(
                "Window",
                true,
                &[&PredefinedMenuItem::minimize(None), &PredefinedMenuItem::maximize(Some("Zoom")), &sep(), &PredefinedMenuItem::bring_all_to_front(None)],
            )
            .ok()?;
            let help = Submenu::with_items("Help", true, &[&item("Keyboard Shortcuts", Cmd::Shortcuts)]).ok()?;
            let menu = Menu::with_items(&[&app, &file, &edit, &controls, &go, &view, &window, &help]).ok()?;
            menu.init_for_nsapp();
            window.set_as_windows_menu_for_nsapp();
            help.set_as_help_menu_for_nsapp();
            MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
                if let Some(cmd) = cmd_of(e.id().as_ref()) {
                    post(cmd);
                }
            }));
            Some(Self { _menu: menu, queue, bit_perfect, themes, shown: None })
        }

        /// Keeps the check marks in step with the app (cheap when nothing changed).
        pub fn sync(&mut self, queue: bool, bit_perfect: bool, theme: u8) {
            if self.shown == Some((queue, bit_perfect, theme)) {
                return;
            }
            self.shown = Some((queue, bit_perfect, theme));
            self.queue.set_checked(queue);
            self.bit_perfect.set_checked(bit_perfect);
            for (i, t) in self.themes.iter().enumerate() {
                t.set_checked(i == usize::from(theme));
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn accelerators_come_from_the_key_map() {
            assert!(accel(Cmd::Search).is_some());
            assert!(accel(Cmd::Love).is_some());
            assert!(accel(Cmd::Settings).is_some());
            assert!(accel(Cmd::Back).is_some());
            // Keys a text field needs stay with the field.
            assert!(accel(Cmd::Next).is_none());
            assert!(accel(Cmd::VolumeUp).is_none());
            assert!(accel(Cmd::PlayPause).is_none());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip() {
        for cmd in [Cmd::Next, Cmd::Place(3), Cmd::Appearance(2), Cmd::ToggleBitPerfect] {
            assert_eq!(cmd_of(&id_of(cmd)), Some(cmd));
        }
        assert_eq!(cmd_of("nonsense"), None);
    }
}
