//! The UI never shows emoji or decorative symbols: icons are painted (see
//! `icons.rs`), so a stray "★" or "▶" in a string is a mistake. This test reads
//! every string and char literal in the workspace's sources and fails on one.

use std::path::Path;

/// Symbols that are fine in text: keyboard glyphs in shortcut hints.
const KEYBOARD: [char; 6] = ['⌘', '⌥', '⌃', '⏎', '⌫', '⌦'];

/// Emoji, pictographs and the symbol blocks macOS may draw as color emoji
/// (stars, notes, checks, crosses, play/pause shapes, bullets).
fn banned(c: char) -> bool {
    let o = u32::from(c);
    matches!(o, 0x1F000..=0x1FAFF | 0x2600..=0x27BF | 0x2B00..=0x2BFF | 0x25A0..=0x25FF | 0xFE0F | 0x200D | 0x2022)
        || ((0x2300..=0x23FF).contains(&o) && !KEYBOARD.contains(&c))
}

/// The string and char literals in Rust source, with the line each starts on.
/// Comments are skipped, so diagrams in docs may use arrows and shapes.
fn literals(src: &str) -> Vec<(usize, String)> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let (mut i, mut line) = (0, 1);
    let at = |i: usize| chars.get(i).copied().unwrap_or('\0');
    while i < chars.len() {
        let c = chars[i];
        if c == '\n' {
            line += 1;
            i += 1;
        } else if c == '/' && at(i + 1) == '/' {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && at(i + 1) == '*' {
            i += 2;
            while i < chars.len() && !(chars[i] == '*' && at(i + 1) == '/') {
                line += usize::from(chars[i] == '\n');
                i += 1;
            }
            i += 2;
        } else if c == 'r' && (at(i + 1) == '"' || (at(i + 1) == '#' && !at(i.wrapping_sub(1)).is_alphanumeric())) {
            // Raw string: r"…", r#"…"#.
            let mut j = i + 1;
            let mut hashes = 0;
            while at(j) == '#' {
                hashes += 1;
                j += 1;
            }
            if at(j) != '"' {
                i += 1;
                continue;
            }
            j += 1;
            let start = (line, j);
            let close: String = std::iter::once('"').chain(std::iter::repeat_n('#', hashes)).collect();
            let mut text = String::new();
            while j < chars.len() && !chars[j..].iter().take(close.len()).copied().eq(close.chars()) {
                line += usize::from(chars[j] == '\n');
                text.push(chars[j]);
                j += 1;
            }
            out.push((start.0, text));
            i = j + close.len();
        } else if c == '"' {
            let start = line;
            let mut j = i + 1;
            let mut text = String::new();
            while j < chars.len() && chars[j] != '"' {
                if chars[j] == '\\' {
                    text.push(chars[j]);
                    j += 1;
                }
                line += usize::from(at(j) == '\n');
                text.push(at(j));
                j += 1;
            }
            out.push((start, text));
            i = j + 1;
        } else if c == '\'' {
            // A char literal ('x', '\n', '\u{..}'), or else a lifetime.
            if at(i + 1) == '\\' {
                let mut j = i + 2;
                while j < chars.len() && chars[j] != '\'' {
                    j += 1;
                }
                i = j + 1;
            } else if at(i + 2) == '\'' {
                out.push((line, at(i + 1).to_string()));
                i += 3;
            } else {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    out
}

fn rust_files(dir: &Path, into: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = entry.path();
        if p.is_dir() {
            rust_files(&p, into);
        } else if p.extension().is_some_and(|e| e == "rs") {
            into.push(p);
        }
    }
}

#[test]
fn no_emoji_or_symbol_glyphs_in_any_string() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("crates dir");
    let mut files = Vec::new();
    rust_files(crates, &mut files);
    assert!(files.len() > 20, "found the sources");
    let mut found = Vec::new();
    // This file names the banned symbols to test the scanner.
    for f in files.into_iter().filter(|f| !f.ends_with("no_emoji.rs")) {
        let src = std::fs::read_to_string(&f).unwrap();
        for (line, text) in literals(&src) {
            if let Some(c) = text.chars().find(|&c| banned(c)) {
                found.push(format!("{}:{line}: {c:?} (U+{:04X}) in {text:?}", f.display(), u32::from(c)));
            }
        }
    }
    assert!(found.is_empty(), "draw these with icons instead:\n{}", found.join("\n"));
}

#[test]
fn the_scanner_reads_literals_and_skips_comments() {
    let src = "// ★ in a comment\nlet a = \"play ▶\"; /* ✓ */ let b = '♪'; let c = r#\"x ★ \"y\"\"#; fn f<'a>(_: &'a str) {}";
    let lits: Vec<String> = literals(src).into_iter().map(|(_, t)| t).collect();
    assert_eq!(lits, vec!["play ▶".to_owned(), "♪".to_owned(), "x ★ \"y\"".to_owned()]);
    assert!(banned('★') && banned('▶') && banned('✓') && banned('⏸') && banned('•'));
    assert!(!banned('⌘') && !banned('⇧') && !banned('⏎') && !banned('·') && !banned('→') && !banned('…'));
}
