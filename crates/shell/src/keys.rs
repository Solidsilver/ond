//! Key combos as each platform writes them. The apps write their shortcuts
//! with the Mac glyphs (⌘ ⌥ ⇧ ⏎ ⌫); elsewhere [`shown`] spells them out.

/// Text with key combos, as this platform writes them: the Mac glyphs on
/// macOS, "Ctrl+Shift+L" style elsewhere. Works on a bare combo ("⇧⌘L") and
/// on combos inside a sentence ("Back (⌘[)").
pub fn shown(text: &str) -> std::borrow::Cow<'_, str> {
    if cfg!(target_os = "macos") || !text.contains(['⌘', '⌥', '⇧', '⏎', '⌫']) { text.into() } else { pc_style(text).into() }
}

/// "⇧⌘L" → "Ctrl+Shift+L", "Back (⌘[)" → "Back (Ctrl+[)", "⌥⏎" → "Alt+Enter".
fn pc_style(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if !matches!(c, '⌘' | '⌥' | '⇧' | '⏎' | '⌫') {
            out.push(c);
            continue;
        }
        // A run of modifiers, then the key they go with.
        let (mut ctrl, mut alt, mut shift, mut key) = (false, false, false, Some(c));
        while let Some(k) = key {
            match k {
                '⌘' => ctrl = true,
                '⌥' => alt = true,
                '⇧' => shift = true,
                _ => break,
            }
            key = chars.next();
        }
        for (on, name) in [(ctrl, "Ctrl+"), (alt, "Alt+"), (shift, "Shift+")] {
            if on {
                out.push_str(name);
            }
        }
        match key {
            Some('⏎') => out.push_str("Enter"),
            Some('⌫') => out.push_str("Backspace"),
            Some(k) => out.push(k),
            None => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn other_platforms_spell_out_the_modifiers() {
        assert_eq!(pc_style("⇧⌘L"), "Ctrl+Shift+L");
        assert_eq!(pc_style("⌥⌘→"), "Ctrl+Alt+→");
        assert_eq!(pc_style("⌥↑ ⌥↓"), "Alt+↑ Alt+↓");
        assert_eq!(pc_style("⇧⏎"), "Shift+Enter");
        assert_eq!(pc_style("Space"), "Space");
        assert_eq!(pc_style("⌫"), "Backspace");
        assert_eq!(pc_style("Back (⌘[)"), "Back (Ctrl+[)");
        assert_eq!(pc_style("⏎ open    ⌘⏎ play"), "Enter open    Ctrl+Enter play");
        assert_eq!(pc_style("press ⇧⌘D on its page."), "press Ctrl+Shift+D on its page.");
    }
}
