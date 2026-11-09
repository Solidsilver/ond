//! A description as prose: the HTML Audiobookshelf stores, turned into blocks
//! egui can lay out.
//!
//! A description is a paragraph of marketing copy, not a document: bold runs
//! in a blurb, `<p>` paragraphs, a list of praise, the odd `<b>`-wrapped
//! heading, an entity or two and sometimes a link. [`parse`] reads that markup
//! with a tokenizer and a state machine, not a DOM, so unknown tags keep their
//! text and cost nothing, malformed markup can't panic, and nothing is ever
//! fetched or run. [`draw`] paints the blocks at the reading size, with bold,
//! italic, underlined and struck-through runs, hanging list markers, indented
//! quotes, and links the system browser opens.
//!
//! Links are only as good as their scheme: `http` and `https` open, everything
//! else (`javascript:`, `data:`, a relative path) stays plain words, because a
//! server chose the markup and a click must not be able to run it.

use crate::theme::{Tokens, semibold};
use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, CursorIcon, FontFamily, FontId, Galley, OpenUrl, Response, Sense, Stroke, Ui, Vec2, pos2, vec2};
use std::borrow::Cow;
use std::ops::Range;

/// The size description text is read at, and the leading between its rows.
const BODY_SIZE: f32 = 13.0;
const LINE_HEIGHT: f32 = 20.0;
/// A list item's indent per level of nesting, and the width its marker takes
/// (numbers are right-aligned in it, as a chapter's number is).
const LIST_INDENT: f32 = 18.0;
const MARKER_W: f32 = 16.0;
/// A quoted paragraph's indent, per level of quotation.
const QUOTE_INDENT: f32 = 14.0;
/// The space between one paragraph and the next, and less between the items of
/// a list so it reads as one.
const PARA_GAP: f32 = 9.0;
const ITEM_GAP: f32 = 4.0;

/// The inks and fonts a description is drawn in: the theme's, at the size a
/// book's page reads at.
pub struct Style {
    /// The body text's font.
    pub font: FontId,
    /// The leading between rows of body text.
    pub line_height: f32,
    /// What bold runs are drawn in: the theme's semibold weight.
    pub bold: FontFamily,
    pub text: Color32,
    pub muted: Color32,
    pub link: Color32,
}

impl Style {
    /// The theme's inks, at the size a book's page is set in.
    pub fn reading(ui: &Ui) -> Self {
        let t = Tokens::of(ui);
        Self { font: FontId::proportional(BODY_SIZE), line_height: LINE_HEIGHT, bold: semibold(), text: t.text, muted: t.muted, link: t.accent }
    }
}

/// Where a block sits on the page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Paragraph,
    /// A heading, with its level: `h1` is 1.
    Heading(u8),
    /// Quoted text, indented and quiet; the depth counts nested quotes.
    Quote(u8),
    /// A list item: the marker drawn before it, and how deep its list nests.
    Item {
        marker: Marker,
        depth: u8,
    },
}

/// What is drawn before a list item's text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Marker {
    /// A `ul`'s dot. Painted rather than typed: the family draws its glyphs
    /// rather than set symbols in a string (Ond's `no_emoji` test says so).
    Dot,
    /// An `ol`'s item number, in its list's order.
    Number(u32),
}

/// A stretch of a block's text with one style.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Run {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    /// The URL this run opens, when it is a link whose scheme may be opened.
    pub link: Option<String>,
}

/// One paragraph, heading, quoted paragraph or list item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub kind: Kind,
    pub runs: Vec<Run>,
}

impl Block {
    /// The block as a layout job, with every run's style set.
    pub fn paragraph(&self, style: &Style) -> Paragraph {
        let size = self.size(style);
        let mut job = LayoutJob::default();
        job.text.reserve(self.runs.iter().map(|r| r.text.len()).sum());
        let mut links: Vec<(Range<usize>, String)> = Vec::new();
        for run in &self.runs {
            let start = job.text.len();
            let ink = match self.kind {
                Kind::Quote(_) => style.muted,
                _ => style.text,
            };
            let mut format = TextFormat {
                font_id: FontId::new(size, if run.bold { style.bold.clone() } else { style.font.family.clone() }),
                color: ink,
                italics: run.italic,
                underline: if run.underline { Stroke::new(1.0, ink) } else { Stroke::NONE },
                strikethrough: if run.strike { Stroke::new(1.0, ink) } else { Stroke::NONE },
                line_height: Some(self.line_height(style)),
                ..Default::default()
            };
            if let Some(url) = &run.link {
                // A link reads as one whatever the surrounding markup says.
                format.color = style.link;
                format.underline = Stroke::new(1.0, style.link);
                links.push((start..start + run.text.len(), url.clone()));
            }
            job.append(&run.text, 0.0, format);
        }
        Paragraph { job, links }
    }

    fn size(&self, style: &Style) -> f32 {
        match self.kind {
            // A heading is a touch larger than the prose: enough to break the
            // page into parts, not enough to shout on a book's page.
            Kind::Heading(level) => style.font.size + f32::from(5_u8.saturating_sub(level.min(4))),
            _ => style.font.size,
        }
    }

    fn line_height(&self, style: &Style) -> f32 {
        style.line_height.max(self.size(style) * 1.45)
    }
}

/// A block laid out: the job to paint, and the byte ranges of its text that
/// open a URL.
pub struct Paragraph {
    pub job: LayoutJob,
    pub links: Vec<(Range<usize>, String)>,
}

impl Paragraph {
    /// The URL under a point in the laid-out paragraph, if a link is there.
    pub fn link_at(&self, galley: &Galley, at: Vec2) -> Option<&str> {
        link_at(&self.links, galley, at)
    }

    /// The URL at a byte offset in the laid-out text, if a link is there.
    pub fn link_at_byte(&self, byte: usize) -> Option<&str> {
        link_at_byte(&self.links, byte)
    }
}

/// The URL under a point, from the byte ranges a paragraph's links cover.
fn link_at<'a>(links: &'a [(Range<usize>, String)], galley: &Galley, at: Vec2) -> Option<&'a str> {
    if links.is_empty() {
        return None;
    }
    let cursor = galley.cursor_from_pos(at);
    let byte = galley.text().char_indices().nth(cursor.index.0).map_or(galley.text().len(), |(byte, _)| byte);
    link_at_byte(links, byte)
}

fn link_at_byte(links: &[(Range<usize>, String)], byte: usize) -> Option<&str> {
    links.iter().find(|(range, _)| range.contains(&byte)).map(|(_, url)| url.as_str())
}

/// Paints `blocks` at the style's reading size, filling `ui`'s width, and
/// makes the links among them open in the system browser.
pub fn draw(ui: &mut Ui, blocks: &[Block], style: &Style) {
    for (i, block) in blocks.iter().enumerate() {
        if i > 0 {
            ui.add_space(gap(&blocks[i - 1].kind, &block.kind));
        }
        match &block.kind {
            Kind::Item { marker, depth } => item(ui, block, *marker, *depth, style),
            Kind::Quote(depth) => {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    ui.add_space(QUOTE_INDENT * f32::from(*depth));
                    paragraph(ui, block, style);
                });
            }
            _ => {
                paragraph(ui, block, style);
            }
        }
    }
}

/// The space before a block, from what came before it.
fn gap(previous: &Kind, next: &Kind) -> f32 {
    match (previous, next) {
        (Kind::Item { .. }, Kind::Item { .. }) => ITEM_GAP,
        _ => PARA_GAP,
    }
}

/// A list item: its marker in a fixed slot, then text that hangs from it.
fn item(ui: &mut Ui, block: &Block, marker: Marker, depth: u8, style: &Style) {
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.add_space(LIST_INDENT * f32::from(depth.saturating_sub(1)));
        let (slot, _) = ui.allocate_exact_size(vec2(MARKER_W, style.line_height), Sense::hover());
        if ui.is_rect_visible(slot) {
            let middle = slot.top() + style.line_height / 2.0;
            match marker {
                Marker::Dot => {
                    ui.painter().circle_filled(pos2(slot.right() - 8.0, middle), 1.5, style.muted);
                }
                Marker::Number(number) => {
                    let galley = ui.painter().layout_no_wrap(format!("{number}."), style.font.clone(), style.muted);
                    let at = pos2(slot.right() - galley.size().x - 3.0, middle - galley.size().y / 2.0);
                    ui.painter().galley(at, galley, style.muted);
                }
            }
        }
        paragraph(ui, block, style);
    });
}

/// One block of prose: laid out to fill what's left of the row, painted, and
/// its links clickable.
fn paragraph(ui: &mut Ui, block: &Block, style: &Style) -> Response {
    let Paragraph { job, links } = block.paragraph(style);
    let label = egui::Label::new(job).sense(Sense::click());
    let (at, galley, response) = label.layout_in_ui(ui);
    let text = galley.text().to_owned();
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, ui.is_enabled(), &text));
    if ui.is_rect_visible(response.rect) {
        // Painted through the label's selection state, so the prose can still
        // be selected and copied as it could before it was markup.
        egui::text_selection::LabelSelectionState::label_text_selection(ui, &response, at, galley.clone(), style.text, Stroke::NONE);
        if response.hovered()
            && let Some(pointer) = ui.ctx().pointer_hover_pos()
            && let Some(url) = link_at(&links, &galley, pointer - at)
        {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
            if response.clicked() {
                ui.ctx().open_url(OpenUrl::new_tab(url));
            }
        }
    }
    response
}

/// Reads a description's markup into blocks, in the order it reads.
pub fn parse(html: &str) -> Vec<Block> {
    let tokens = tokens(html);
    // Prose the server never marked up keeps its own lines: a newline is the
    // paragraph break it looks like. Markup, where there is any, gets HTML's
    // rule instead, in which a newline is only a space.
    let plain = tokens.iter().all(|t| matches!(t, Token::Text(_)));
    let mut builder = Builder { blocks: Vec::new(), cur: None, stack: Vec::new(), plain };
    for token in tokens {
        match token {
            Token::Text(text) => builder.text(&text),
            Token::Start { tag, attrs } => builder.start(tag, attrs),
            Token::End { tag } => builder.end(tag),
        }
    }
    builder.finish()
}

/// One piece of markup: text, an opening tag with its attributes, or a
/// closing tag.
enum Token<'a> {
    Text(Cow<'a, str>),
    Start { tag: Tag, attrs: &'a str },
    End { tag: Tag },
}

/// The tags this reader knows. Everything else is [`Tag::Other`]: dropped,
/// with its text kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tag {
    P,
    Div,
    Br,
    B,
    Strong,
    I,
    Em,
    U,
    S,
    Del,
    H(u8),
    Ul,
    Ol,
    Li,
    Blockquote,
    A,
    Script,
    Style,
    Other,
}

impl Tag {
    fn of(name: &str) -> Tag {
        match name.trim().to_ascii_lowercase().as_str() {
            "p" => Tag::P,
            "div" => Tag::Div,
            "br" => Tag::Br,
            "b" => Tag::B,
            "strong" => Tag::Strong,
            "i" => Tag::I,
            "em" => Tag::Em,
            "u" => Tag::U,
            "s" => Tag::S,
            "del" => Tag::Del,
            "h1" => Tag::H(1),
            "h2" => Tag::H(2),
            "h3" => Tag::H(3),
            "h4" => Tag::H(4),
            "h5" => Tag::H(5),
            "h6" => Tag::H(6),
            "ul" => Tag::Ul,
            "ol" => Tag::Ol,
            "li" => Tag::Li,
            "blockquote" => Tag::Blockquote,
            "a" => Tag::A,
            "script" => Tag::Script,
            "style" => Tag::Style,
            _ => Tag::Other,
        }
    }
}

/// Splits `html` into text (entities decoded) and tags, reading the markup
/// tolerantly: a `<` that starts no tag is a character like any other, and a
/// tag that never closes is text.
fn tokens(html: &str) -> Vec<Token<'_>> {
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(lt) = rest.find('<') {
        let (text, tail) = rest.split_at(lt);
        if !text.is_empty() {
            out.push(Token::Text(decoded(text)));
        }
        match markup(tail) {
            Some((token, used)) => {
                if let Some(token) = token {
                    out.push(token);
                }
                rest = &tail[used..];
            }
            None => {
                out.push(Token::Text(Cow::Borrowed("<")));
                rest = &tail[1..];
            }
        }
    }
    if !rest.is_empty() {
        out.push(Token::Text(decoded(rest)));
    }
    out
}

/// Reads the markup starting at `s`'s `<`: the token it makes (`None` for a
/// comment or a declaration) and how many bytes it took. `None` when `s`
/// starts no tag at all.
fn markup(s: &str) -> Option<(Option<Token<'_>>, usize)> {
    if s.starts_with("<!--") {
        // A comment runs to its end, or, unterminated, takes the rest with it.
        return Some((None, s.find("-->").map_or(s.len(), |i| i + 3)));
    }
    if s.starts_with("<!") || s.starts_with("<?") {
        return s.find('>').map(|i| (None, i + 1));
    }
    let end = tag_end(s)?;
    let inner = &s[1..end];
    let (name, attrs) = split_name(inner);
    let token = match name.strip_prefix('/') {
        Some(name) => Token::End { tag: Tag::of(name) },
        None if name.is_empty() => return None,
        None => Token::Start { tag: Tag::of(name), attrs },
    };
    Some((Some(token), end + 1))
}

/// The byte index of the `>` that ends the tag `s` starts, skipping the ones
/// inside quoted attributes; `None` when there is no end.
fn tag_end(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    match bytes.get(1) {
        Some(c) if c.is_ascii_alphabetic() || *c == b'/' => {}
        _ => return None,
    }
    let mut quote = None;
    for (i, &c) in bytes.iter().enumerate().skip(1) {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None => match c {
                b'"' | b'\'' => quote = Some(c),
                b'>' => return Some(i),
                _ => {}
            },
        }
    }
    None
}

/// A tag's name and the rest of its inside, with a self-closing slash taken
/// off the name.
fn split_name(inner: &str) -> (&str, &str) {
    let end = inner.find(|c: char| c.is_ascii_whitespace()).unwrap_or(inner.len());
    let (name, attrs) = inner.split_at(end);
    (name.trim_end_matches('/'), attrs)
}

/// An attribute's value, however it was written: quoted with either mark or
/// bare. `None` when the tag hasn't got it.
fn attribute<'a>(attrs: &'a str, wanted: &str) -> Option<&'a str> {
    let mut rest = attrs;
    loop {
        rest = rest.trim_start();
        if rest.is_empty() {
            return None;
        }
        let end = rest.find(|c: char| c.is_whitespace() || c == '=').unwrap_or(rest.len());
        let (name, tail) = rest.split_at(end);
        let tail = tail.trim_start();
        let (value, after) = match tail.strip_prefix('=') {
            Some(tail) => {
                let tail = tail.trim_start();
                match tail.chars().next() {
                    Some(open @ ('"' | '\'')) => {
                        let body = &tail[1..];
                        match body.find(open) {
                            Some(i) => (&body[..i], &body[i + 1..]),
                            None => (body, ""),
                        }
                    }
                    _ => {
                        let end = tail.find(|c: char| c.is_whitespace() || c == '>').unwrap_or(tail.len());
                        tail.split_at(end)
                    }
                }
            }
            None => ("", tail),
        };
        if name.eq_ignore_ascii_case(wanted) {
            return Some(value);
        }
        rest = after;
    }
}

/// The URL a link opens, when its scheme is one the system browser may have.
/// A description arrives from a server, so `javascript:`, `data:` and the rest
/// stay as plain words.
fn openable(url: &str) -> Option<String> {
    let url = url.trim();
    let colon = url.find(':')?;
    if !matches!(url[..colon].to_ascii_lowercase().as_str(), "http" | "https") {
        return None;
    }
    let rest = &url[colon + 1..];
    let host = rest.trim_start_matches('/');
    let clean = !url.chars().any(|c| c.is_control() || c.is_whitespace());
    (clean && !host.is_empty()).then(|| url.to_owned())
}

/// Text with its character entities decoded, and any other `&` left as it
/// was written.
fn decoded(text: &str) -> Cow<'_, str> {
    if !text.contains('&') {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let tail = &rest[amp + 1..];
        // No entity's name is longer than this; a `;` further off is a
        // semicolon of the prose's, not of an entity.
        if let Some(i) = tail.find(';').filter(|&i| i <= 32)
            && let Some(c) = entity(&tail[..i])
        {
            out.push(c);
            rest = &tail[i + 1..];
        } else {
            out.push('&');
            rest = tail;
        }
    }
    out.push_str(rest);
    Cow::Owned(out)
}

/// The character a named or numeric entity stands for.
fn entity(name: &str) -> Option<char> {
    if let Some(number) = name.strip_prefix('#') {
        let code = match number.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => number.parse().ok()?,
        };
        return char::from_u32(code).filter(|c| !matches!(c, '\u{0}' | '\u{feff}'));
    }
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        // Kept as a non-breaking space: HTML never wraps there.
        "nbsp" => '\u{a0}',
        "mdash" => '—',
        "ndash" => '–',
        "hellip" => '…',
        "rsquo" => '’',
        "lsquo" => '‘',
        "ldquo" => '“',
        "rdquo" => '”',
        "bull" => '\u{2022}',
        "middot" => '·',
        "times" => '×',
        "divide" => '÷',
        "plusmn" => '±',
        "copy" => '©',
        "reg" => '®',
        "trade" => '™',
        "deg" => '°',
        "sect" => '§',
        "para" => '¶',
        "pound" => '£',
        "euro" => '€',
        "cent" => '¢',
        "laquo" => '«',
        "raquo" => '»',
        "frac12" => '½',
        "frac14" => '¼',
        "frac34" => '¾',
        "aacute" => 'á',
        "agrave" => 'à',
        "eacute" => 'é',
        "egrave" => 'è',
        "iacute" => 'í',
        "oacute" => 'ó',
        "uacute" => 'ú',
        "ntilde" => 'ñ',
        "ccedil" => 'ç',
        "auml" => 'ä',
        "ouml" => 'ö',
        "uuml" => 'ü',
        "szlig" => 'ß',
        "thinsp" => '\u{2009}',
        _ => return None,
    })
}

/// Whether a character is one HTML collapses: a run of these is one space.
fn is_html_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{c}')
}

/// A run of HTML whitespace as a single space, at either end included so that
/// the next text can decide what to do with it.
fn collapsed(text: &str) -> Cow<'_, str> {
    if !text.contains(is_html_space) {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut space = false;
    for c in text.chars() {
        if is_html_space(c) {
            space = true;
        } else {
            if space {
                out.push(' ');
            }
            space = false;
            out.push(c);
        }
    }
    if space {
        out.push(' ');
    }
    Cow::Owned(out)
}

/// A tag that is open, and what the text inside it needs from it.
struct Open {
    tag: Tag,
    /// For `a`: the URL it opens, when its scheme may be.
    href: Option<String>,
    /// For `ol`: the number the next item takes.
    next: u32,
    /// For `li`: what is drawn before its text.
    marker: Marker,
}

impl Open {
    fn new(tag: Tag) -> Self {
        Self { tag, href: None, next: 1, marker: Marker::Dot }
    }
}

/// The styles in force at one point in a description.
#[derive(Debug, Clone, Copy, Default)]
struct Flags {
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
}

/// Writes the blocks, following the tags as they open and close. The open
/// tags are the whole state: what a piece of text looks like and where it sits
/// is read off them, so markup that forgets to close a tag can't leave a style
/// in force after [`Builder::close`] takes it off the stack.
struct Builder {
    blocks: Vec<Block>,
    cur: Option<Block>,
    stack: Vec<Open>,
    /// The description has no markup, so a newline is a paragraph break.
    plain: bool,
}

impl Builder {
    fn finish(mut self) -> Vec<Block> {
        self.end_block();
        self.blocks
    }

    fn start(&mut self, tag: Tag, attrs: &str) {
        match tag {
            // Their contents are not prose; nothing to draw and no break to
            // add, in case they sit mid-sentence.
            Tag::Script | Tag::Style => self.stack.push(Open::new(tag)),
            Tag::P | Tag::Div => self.end_block(),
            Tag::Br => self.break_line(),
            Tag::B | Tag::Strong | Tag::I | Tag::Em | Tag::U | Tag::S | Tag::Del => self.stack.push(Open::new(tag)),
            Tag::A => {
                // The address is markup's too: an `&amp;` in a query string is
                // an ampersand to the browser.
                let href = attribute(attrs, "href").and_then(|href| openable(&decoded(href)));
                self.stack.push(Open { href, ..Open::new(tag) });
            }
            Tag::H(_) => {
                self.end_block();
                self.stack.push(Open::new(tag));
            }
            Tag::Ul | Tag::Ol => {
                self.end_block();
                self.stack.push(Open::new(tag));
            }
            Tag::Li => {
                self.end_block();
                let marker = match self.stack.iter_mut().rev().find(|o| matches!(o.tag, Tag::Ul | Tag::Ol)) {
                    Some(list) if list.tag == Tag::Ol => {
                        let n = list.next;
                        list.next = list.next.saturating_add(1);
                        Marker::Number(n)
                    }
                    _ => Marker::Dot,
                };
                self.stack.push(Open { marker, ..Open::new(tag) });
            }
            Tag::Blockquote => {
                self.end_block();
                self.stack.push(Open::new(tag));
            }
            Tag::Other => {}
        }
    }

    fn end(&mut self, tag: Tag) {
        match tag {
            Tag::P | Tag::Div | Tag::H(_) | Tag::Ul | Tag::Ol | Tag::Li | Tag::Blockquote => {
                self.end_block();
                self.close(tag);
            }
            Tag::Script | Tag::Style | Tag::B | Tag::Strong | Tag::I | Tag::Em | Tag::U | Tag::S | Tag::Del | Tag::A => self.close(tag),
            Tag::Br | Tag::Other => {}
        }
    }

    /// Takes `tag`, and anything left open inside it, off the stack.
    fn close(&mut self, tag: Tag) {
        if let Some(i) = self.stack.iter().rposition(|o| o.tag == tag) {
            self.stack.truncate(i);
        }
    }

    fn text(&mut self, text: &str) {
        if self.skipping() || text.is_empty() {
            return;
        }
        if !self.plain {
            self.append(&collapsed(text));
            return;
        }
        let text = if text.contains('\r') { Cow::Owned(text.replace("\r\n", "\n").replace('\r', "\n")) } else { Cow::Borrowed(text) };
        for (i, line) in text.split('\n').enumerate() {
            if i > 0 {
                self.end_block();
            }
            self.append(line);
        }
    }

    fn append(&mut self, text: &str) {
        let skip = match self.last_char() {
            // Where a paragraph starts, or after a space or a break, a space
            // the markup left is not one the prose shows.
            None | Some(' ' | '\n') => text.len() - text.trim_start_matches(is_html_space).len(),
            Some(_) => 0,
        };
        let text = &text[skip..];
        if text.is_empty() {
            return;
        }
        let (flags, link) = self.flags();
        self.push(text, flags, link);
    }

    /// A `<br>`: a row's end, or a paragraph's when it follows another.
    fn break_line(&mut self) {
        match self.last_char() {
            None => {}
            Some('\n') => self.end_block(),
            Some(_) => {
                let (flags, link) = self.flags();
                self.push("\n", flags, link);
            }
        }
    }

    /// Adds text to the block in hand, folding it into the run before it when
    /// that run has the same style.
    fn push(&mut self, text: &str, flags: Flags, link: Option<String>) {
        let block = self.block();
        match block.runs.last_mut() {
            Some(run)
                if run.bold == flags.bold
                    && run.italic == flags.italic
                    && run.underline == flags.underline
                    && run.strike == flags.strike
                    && run.link == link =>
            {
                run.text.push_str(text);
            }
            _ => block.runs.push(Run { text: text.to_owned(), bold: flags.bold, italic: flags.italic, underline: flags.underline, strike: flags.strike, link }),
        }
    }

    /// The last character of the block in hand, whitespace included.
    fn last_char(&self) -> Option<char> {
        self.cur.as_ref()?.runs.iter().rev().find(|r| !r.text.is_empty())?.text.chars().last()
    }

    /// Ends the paragraph in hand, keeping it when it has words.
    fn end_block(&mut self) {
        let Some(mut block) = self.cur.take() else { return };
        if let Some(last) = block.runs.iter_mut().rev().find(|r| !r.text.is_empty()) {
            last.text.truncate(last.text.trim_end_matches(is_html_space).len());
        }
        block.runs.retain(|run| !run.text.is_empty());
        if !block.runs.is_empty() {
            self.blocks.push(block);
        }
    }

    /// The block being written, started with the kind where it sits gives it.
    fn block(&mut self) -> &mut Block {
        let kind = self.kind();
        self.cur.get_or_insert_with(|| Block { kind, runs: Vec::new() })
    }

    fn kind(&self) -> Kind {
        if let Some(item) = self.stack.iter().rev().find(|o| o.tag == Tag::Li) {
            Kind::Item { marker: item.marker, depth: self.lists().max(1) }
        } else if let Some(level) = self.stack.iter().rev().find_map(|o| match o.tag {
            Tag::H(level) => Some(level),
            _ => None,
        }) {
            Kind::Heading(level)
        } else if let Some(quotes) = self.depth(Tag::Blockquote) {
            Kind::Quote(quotes)
        } else {
            Kind::Paragraph
        }
    }

    fn flags(&self) -> (Flags, Option<String>) {
        let mut flags = Flags::default();
        let mut link = None;
        for open in &self.stack {
            match open.tag {
                Tag::B | Tag::Strong => flags.bold = true,
                Tag::I | Tag::Em => flags.italic = true,
                Tag::U => flags.underline = true,
                Tag::S | Tag::Del => flags.strike = true,
                Tag::A if open.href.is_some() => {
                    flags.underline = true;
                    link = open.href.clone();
                }
                _ => {}
            }
        }
        (flags, link)
    }

    /// How many `tag`s are open, if any: a quote's and a list's depth.
    fn depth(&self, tag: Tag) -> Option<u8> {
        match self.stack.iter().filter(|o| o.tag == tag).count() {
            0 => None,
            n => Some(u8::try_from(n).unwrap_or(u8::MAX)),
        }
    }

    fn lists(&self) -> u8 {
        u8::try_from(self.stack.iter().filter(|o| matches!(o.tag, Tag::Ul | Tag::Ol)).count()).unwrap_or(u8::MAX)
    }

    fn skipping(&self) -> bool {
        self.stack.iter().any(|o| matches!(o.tag, Tag::Script | Tag::Style))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The blurb a book's page was showing as markup. The dots in it are the
    /// server's prose, not glyphs this UI draws, which is why they are spelled
    /// out here.
    const BLURB: &str = "<b>#1 <i>NEW YORK TIMES </i>BESTSELLER </b>\u{2022} <b>Now a major motion picture directed by Steven Spielberg.</b><br />\
        <br /><b>\"Enchanting . . . <i>Willy Wonka </i>meets <i>The Matrix</i>.\"—<i>USA Today </i>\u{2022} <b>\"A triumph\"</b></b>";

    /// Every run in every block, in reading order.
    fn runs(blocks: &[Block]) -> impl Iterator<Item = &Run> {
        blocks.iter().flat_map(|b| b.runs.iter())
    }

    /// The text the blocks read as, with a newline between blocks.
    fn text(blocks: &[Block]) -> String {
        let mut out = String::new();
        for (i, block) in blocks.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            for run in &block.runs {
                out.push_str(&run.text);
            }
        }
        out
    }

    fn run_with(blocks: &[Block], needle: &str) -> Run {
        runs(blocks).find(|r| r.text.contains(needle)).unwrap_or_else(|| panic!("no run holds {needle:?} in {:?}", text(blocks))).clone()
    }

    #[test]
    fn a_blurb_reads_as_prose_with_its_bold_and_italic_where_they_were() {
        let blocks = parse(BLURB);
        let read = text(&blocks);
        assert!(!read.contains('<') && !read.contains('>'), "no tags show through: {read}");
        assert!(read.contains(&format!("#1 NEW YORK TIMES BESTSELLER {}", '\u{2022}')), "the words and the bullet: {read}");
        let headline = run_with(&blocks, "#1 ");
        assert!(headline.bold && !headline.italic, "the badge is bold: {headline:?}");
        let paper = run_with(&blocks, "NEW YORK TIMES");
        assert!(paper.bold && paper.italic, "and its paper is bold italic: {paper:?}");
        assert!(run_with(&blocks, "Steven Spielberg").bold);
        // Two breaks in a row are two paragraphs; the blurb's praise is the
        // second of them.
        assert_eq!(blocks.len(), 2, "{:?}", blocks.iter().map(|b| b.kind.clone()).collect::<Vec<_>>());
        let wonka = run_with(&blocks, "Willy Wonka");
        assert!(wonka.italic && wonka.bold, "the quip is bold with its film titles italic: {wonka:?}");
        assert!(run_with(&blocks, "USA Today").italic);
        assert!(run_with(&blocks, "meets").bold && !run_with(&blocks, "meets").italic);
    }

    #[test]
    fn a_pair_of_breaks_starts_a_new_paragraph() {
        assert_eq!(parse("one<br /><br />two").len(), 2);
        assert_eq!(parse("<p>one</p><p>two</p>").len(), 2);
        assert_eq!(parse("<div>one</div><div>two</div>").len(), 2);
        // One break is a row's end within the paragraph.
        let blocks = parse("one<br />two");
        assert_eq!(blocks.len(), 1);
        assert_eq!(text(&blocks), "one\ntwo");
    }

    #[test]
    fn entities_decode_to_their_characters() {
        let blocks = parse("Tom &amp; Jerry &lt;3 &quot;hi&quot; it&rsquo;s 5 &mdash; 6 &hellip; &#8217;&#x2019; a&nbsp;b");
        assert_eq!(text(&blocks), "Tom & Jerry <3 \"hi\" it’s 5 — 6 … ’’ a\u{a0}b");
        // An `&` that starts no entity is the prose's.
        assert_eq!(text(&parse("a & b &notanentity; c")), "a & b &notanentity; c");
    }

    #[test]
    fn plain_text_keeps_its_line_breaks_as_paragraphs() {
        let blocks = parse("One line.\nAnother line.\n\nAfter the gap.");
        assert_eq!(blocks.len(), 3);
        assert_eq!(text(&blocks), "One line.\nAnother line.\nAfter the gap.");
        assert!(blocks.iter().all(|b| b.kind == Kind::Paragraph));
    }

    #[test]
    fn headings_lists_and_quotes_keep_their_place() {
        let blocks = parse("<h1>Praise</h1><ul><li>First</li><li>Second</li></ul><ol><li>One</li><li>Two</li></ol><blockquote>Quietly.</blockquote>");
        let kinds: Vec<Kind> = blocks.iter().map(|b| b.kind.clone()).collect();
        assert_eq!(kinds[0], Kind::Heading(1));
        assert!(matches!(&kinds[1], Kind::Item { marker: Marker::Dot, depth } if *depth == 1));
        assert!(matches!(&kinds[3], Kind::Item { marker: Marker::Number(1), depth } if *depth == 1));
        assert!(matches!(&kinds[4], Kind::Item { marker: Marker::Number(2), .. }));
        assert_eq!(kinds[5], Kind::Quote(1));
        // A list nested in an item is a level deeper.
        let nested = parse("<ul><li>Outer<ul><li>Inner</li></ul></li></ul>");
        assert!(matches!(&nested[1].kind, Kind::Item { depth, .. } if *depth == 2), "{:?}", nested[1].kind);
    }

    #[test]
    fn script_and_style_are_dropped_whole() {
        let blocks = parse("Before<script>alert('x')</script>after<style>p { color: red }</style>.");
        assert_eq!(text(&blocks), "Beforeafter.");
    }

    #[test]
    fn only_http_and_https_links_are_clickable() {
        let blocks = parse("<a href=\"https://example.com/a?b=1&amp;c=2\">the site</a>");
        let link = run_with(&blocks, "the site");
        assert_eq!(link.link.as_deref(), Some("https://example.com/a?b=1&c=2"));
        // The words stay whatever the scheme is; only the opening goes.
        for href in ["javascript:alert(1)", "JavaScript:alert(1)", "data:text/html,<b>x</b>", "mailto:sam@example.com", "/relative", "#anchor", ""] {
            let blocks = parse(&format!("<a href=\"{href}\">words</a>"));
            assert_eq!(text(&blocks), "words", "{href}");
            assert!(runs(&blocks).all(|r| r.link.is_none()), "{href} is not a link: {blocks:?}");
        }
    }

    #[test]
    fn a_link_is_openable_where_its_text_is() {
        let blocks = parse("See <a href=\"https://example.com\">the site</a> today.");
        let paragraph = blocks[0].paragraph(&Style {
            font: FontId::proportional(BODY_SIZE),
            line_height: LINE_HEIGHT,
            bold: semibold(),
            text: Color32::WHITE,
            muted: Color32::GRAY,
            link: Color32::BLUE,
        });
        let at = paragraph.job.text.find("the site").expect("the link's words");
        assert_eq!(paragraph.link_at_byte(at), Some("https://example.com"));
        assert_eq!(paragraph.link_at_byte(at + "the site".len() + 1), None);
        assert_eq!(paragraph.link_at_byte(0), None, "the words before it open nothing");
        // A link's underline and colour are the theme's, not the format's.
        let format = paragraph.job.sections.iter().find(|s| s.byte_range.start.0 <= at && at < s.byte_range.end.0).map(|s| &s.format).expect("a section");
        assert_eq!(format.color, Color32::BLUE);
        assert!(format.underline.width > 0.0);
    }

    #[test]
    fn clicking_a_link_opens_only_what_may_be_opened() {
        use egui_kittest::kittest::Queryable;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        /// A window with one linked paragraph in it, and the URLs that were
        /// asked to open.
        struct State {
            blocks: Vec<Block>,
            urls: Arc<AtomicUsize>,
            /// The theme's fonts arrive a frame after they're installed.
            installed: bool,
        }
        for (href, opens) in [("https://example.com/review", true), ("javascript:alert(1)", false)] {
            let mut h = egui_kittest::Harness::builder().build_ui_state(
                |ui, state: &mut State| {
                    if !state.installed {
                        state.installed = true;
                        crate::theme::install(ui.ctx(), crate::theme::ThemePref::Light);
                        ui.ctx().request_repaint();
                        return;
                    }
                    let style = Style::reading(ui);
                    draw(ui, &state.blocks, &style);
                    ui.ctx().output(|out| {
                        for command in &out.commands {
                            if matches!(command, egui::OutputCommand::OpenUrl(_)) {
                                state.urls.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    });
                },
                State { blocks: parse(&format!("<a href=\"{href}\">the review</a>")), urls: Arc::new(AtomicUsize::new(0)), installed: false },
            );
            h.run();
            h.run();
            h.get_by_label("the review").click();
            h.run_steps(2);
            let opened = h.state().urls.load(Ordering::Relaxed);
            if opens {
                assert!(opened > 0, "{href} should open");
            } else {
                assert_eq!(opened, 0, "{href} is plain words and must not open anything");
            }
        }
    }

    #[test]
    fn malformed_markup_keeps_its_words() {
        // Stray angle brackets are characters; unclosed tags end with the text.
        assert_eq!(text(&parse("a < b and 3 > 2")), "a < b and 3 > 2");
        assert_eq!(text(&parse("<3 hearts")), "<3 hearts");
        assert_eq!(text(&parse("before <b>unfinished")), "before unfinished");
        assert_eq!(text(&parse("before <b>bold <i>both")), "before bold both");
        assert_eq!(text(&parse("<p>unclosed paragraph")), "unclosed paragraph");
        assert_eq!(text(&parse("<!-- a comment -->visible")), "visible");
        // Attributes with and without quotes, and tags whose names this reader
        // doesn't know, keep the text inside them.
        assert_eq!(text(&parse("<span class=big id='x'>kept</span>")), "kept");
        assert_eq!(text(&parse("<o:p>word</o:p> <hr> <img src=\"x.png\">")), "word");
        assert_eq!(text(&parse("<a href=https://example.com>x</a>")), "x");
        assert_eq!(run_with(&parse("<a href=https://example.com>x</a>"), "x").link.as_deref(), Some("https://example.com"));
        // A tag that never closes its quote takes the rest with it, and still
        // doesn't panic.
        let _ = parse("<a href=\"never closes");
        let _ = parse("<<<>>>><p></p></div></b>");
        let _ = parse("\u{0}\u{feff}&#0;&#xD800;&#x110000;");
    }

    #[test]
    fn every_link_a_description_makes_is_one_that_can_open() {
        // A few hundred strings of markup's own pieces: whatever comes out,
        // the links are http(s), the blocks have text, and nothing panics.
        let pieces = [
            "<b>",
            "</b>",
            "<i>",
            "</i>",
            "<p>",
            "</p>",
            "<br />",
            "<ul>",
            "<li>",
            "</li>",
            "<a href=\"javascript:x\">",
            "<a href='https://example.com'>",
            "<a href=https://example.com>",
            "</a>",
            "<script>",
            "</script>",
            "&amp;",
            "&#8217;",
            "&",
            "<",
            ">",
            "\"",
            "'",
            " ",
            "\n",
            "word",
            "<h2>",
            "</h2>",
        ];
        let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
        let mut next = |bound: usize| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 33) as usize % bound
        };
        for _ in 0..400 {
            let mut html = String::new();
            for _ in 0..next(24) {
                html.push_str(pieces[next(pieces.len())]);
            }
            let blocks = parse(&html);
            for block in &blocks {
                assert!(!block.runs.is_empty(), "{html:?} made an empty block");
            }
            for run in runs(&blocks) {
                if let Some(link) = &run.link {
                    assert!(openable(link).is_some(), "{html:?} made the link {link:?}");
                }
            }
        }
        // Text without a tag in it is its own words.
        let plain = "just some words, no markup at all";
        assert_eq!(text(&parse(plain)), plain);
    }

    #[test]
    fn markup_collapses_whitespace_but_prose_keeps_one_break_per_line() {
        let blocks = parse("<p>  one   two\n\tthree  </p>");
        assert_eq!(text(&blocks), "one two three");
        assert_eq!(text(&parse("  one  two  ")), "one  two", "prose keeps its spaces, less the edges");
    }
}
