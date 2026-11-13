//! A form's pieces, for signing in and for a source's settings: a
//! [`TextField`] (its [`field_label`] above, a [`field_error`] and a
//! [`field_help`] line under it), the full-width [`form_button`] that can
//! say it's waiting, the [`or_divider`] between two ways in, a
//! [`notice`] to be read rather than acted on, and a [`folder_list`] for a
//! source that reads folders on this computer. The words are the caller's;
//! nothing here knows what the form is for.

use crate::icons::Icon;
use crate::theme::{Tokens, semibold};
use egui::{FontId, Response, RichText, Sense, Ui, vec2};

/// A field's name, above it.
pub fn field_label(ui: &mut Ui, label: &str) -> Response {
    let t = Tokens::of(ui);
    ui.label(RichText::new(label).font(FontId::new(12.0, semibold())).color(t.muted))
}

/// A line of help under a field, such as an example of what goes in it.
pub fn field_help(ui: &mut Ui, text: &str) -> Response {
    let t = Tokens::of(ui);
    ui.label(RichText::new(text).size(11.0).color(t.muted))
}

/// What went wrong, in the danger colour. Under a field, `about` is the
/// field's label; under the buttons, it names what failed as a whole (a
/// sign-in). A screen reader says it with the error, so the user knows which
/// one it's about.
pub fn field_error(ui: &mut Ui, about: &str, error: &str) -> Response {
    let t = Tokens::of(ui);
    let resp = ui.add(egui::Label::new(RichText::new(error).size(12.0).color(t.danger)).wrap());
    let name = format!("{about}: {error}");
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &name));
    resp
}

/// A labelled one-line text field, as wide as the column it's in. Built up
/// and then drawn with [`show`](Self::show), which says whether Enter was
/// pressed in it.
#[derive(Clone, Copy)]
pub struct TextField<'a> {
    label: &'a str,
    hint: &'a str,
    password: bool,
    focus: bool,
    error: Option<&'a str>,
    help: Option<&'a str>,
}

impl<'a> TextField<'a> {
    /// A field named `label`, which a screen reader reads out too.
    pub fn new(label: &'a str) -> Self {
        Self { label, hint: "", password: false, focus: false, error: None, help: None }
    }

    /// Greyed-out text shown while the field is empty.
    pub fn hint(self, hint: &'a str) -> Self {
        Self { hint, ..self }
    }

    /// Hides what's typed.
    pub fn password(self, password: bool) -> Self {
        Self { password, ..self }
    }

    /// Takes keyboard focus this frame. Ask once, so the user can move it.
    pub fn focus(self, focus: bool) -> Self {
        Self { focus, ..self }
    }

    /// What's wrong with what's in it: the field is outlined in the danger
    /// colour and the error goes under it.
    pub fn error(self, error: Option<&'a str>) -> Self {
        Self { error, ..self }
    }

    /// A line of [`field_help`] under it (and under its error).
    pub fn help(self, help: Option<&'a str>) -> Self {
        Self { help, ..self }
    }

    /// Draws the label, the field, its error and its help. True when Enter
    /// was pressed in it.
    pub fn show(self, ui: &mut Ui, text: &mut String) -> bool {
        field_label(ui, self.label);
        self.show_input(ui, text)
    }

    /// The same without the label, for a caller that draws its own (with
    /// something beside it).
    pub fn show_input(self, ui: &mut Ui, text: &mut String) -> bool {
        let t = Tokens::of(ui);
        let r = ui.add(
            egui::TextEdit::singleline(text)
                .hint_text(self.hint)
                .password(self.password)
                .desired_width(f32::INFINITY)
                .margin(vec2(10.0, 7.0))
                .background_color(t.field),
        );
        let label = self.label;
        r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, true, label));
        // The border says which field the error below it is about.
        if self.error.is_some() {
            ui.painter().rect_stroke(r.rect, 4.0, egui::Stroke::new(1.0, t.danger), egui::StrokeKind::Inside);
        }
        if self.focus {
            r.request_focus();
        }
        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if let Some(error) = self.error {
            field_error(ui, self.label, error);
        }
        if let Some(help) = self.help {
            field_help(ui, help);
        }
        enter
    }
}

/// One of a form's full-width buttons: the accent fill for the main way in
/// (`primary`), the quiet field fill for another. `busy` is `Some` on the one
/// waiting for an answer, which takes no clicks and turns a spinner while
/// `Some(true)`; its `label` says what it's waiting for ("Connecting…").
pub fn form_button(ui: &mut Ui, label: &str, primary: bool, busy: Option<bool>) -> Response {
    let t = Tokens::of(ui);
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), if busy.is_some() { Sense::hover() } else { Sense::click() });
    let hovered = resp.hovered() && busy.is_none();
    let (fill, ink) = match (primary, busy.is_some(), hovered) {
        (true, true, _) => (t.accent.gamma_multiply(0.6), t.on_accent),
        (true, false, true) => (t.accent.gamma_multiply(0.88), t.on_accent),
        (true, false, false) => (t.accent, t.on_accent),
        (false, _, true) => (t.selected, t.accent),
        (false, _, false) => (t.field, t.accent),
    };
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        p.rect_filled(rect, 7.0, fill);
        // A long label (a provider's name) is cut rather than spilling out.
        let galley = super::elide(ui, label, FontId::new(13.0, semibold()), ink, rect.width() - 56.0);
        p.galley(rect.center() - galley.size() / 2.0, galley, ink);
        if busy == Some(true) {
            let r = egui::Rect::from_center_size(egui::pos2(rect.left() + 18.0, rect.center().y), vec2(14.0, 14.0));
            crate::icons::paint(p, r, Icon::Spinner, ink);
        }
        super::focus_ring(ui, &resp, rect, 7.0);
    }
    let enabled = ui.is_enabled() && busy.is_none();
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, label));
    if busy.is_none() { resp.on_hover_cursor(egui::CursorIcon::PointingHand) } else { resp }
}

/// What was done to a [`folder_list`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderListEvent {
    /// The × beside the folder at this index: take it off the list.
    Remove(usize),
    /// Choose folder…: the app opens its folder picker.
    Choose,
}

/// A list of folders, each in an outlined row with its name over its path
/// and a × that takes it off, then a quiet "Choose folder…" under them.
/// With none yet, `empty` stands in the outline. While `busy` nothing can
/// be changed. The folders and the picker are the app's; this only draws
/// them and says what was clicked.
pub fn folder_list(ui: &mut Ui, folders: &[std::path::PathBuf], empty: &str, busy: bool) -> Option<FolderListEvent> {
    let t = Tokens::of(ui);
    let mut event = None;
    let width = ui.available_width();
    let rows = folders.len().max(1) as f32;
    let (rect, _) = ui.allocate_exact_size(vec2(width, rows * FOLDER_ROW_H + 2.0), Sense::hover());
    ui.painter().rect_stroke(rect, 8.0, egui::Stroke::new(1.0, t.hairline), egui::StrokeKind::Inside);
    if folders.is_empty() {
        let g = super::elide(ui, empty, FontId::proportional(12.0), t.muted, width - 28.0);
        ui.painter().galley(egui::pos2(rect.left() + 14.0, rect.center().y - g.size().y / 2.0), g.clone(), t.muted);
        ui.interact(rect, ui.scope_id().with("kit-folder-list-empty"), Sense::hover())
            .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, g.text()));
    }
    for (i, path) in folders.iter().enumerate() {
        let row = egui::Rect::from_min_size(egui::pos2(rect.left(), rect.top() + 1.0 + i as f32 * FOLDER_ROW_H), vec2(width, FOLDER_ROW_H));
        let p = ui.painter();
        if i > 0 {
            p.hline(row.left() + 12.0..=row.right() - 12.0, row.top(), egui::Stroke::new(1.0, t.hairline));
        }
        crate::icons::paint(p, egui::Rect::from_center_size(egui::pos2(row.left() + 20.0, row.center().y), vec2(14.0, 14.0)), Icon::Folder, t.muted);
        let name = path.file_name().map_or_else(|| path.to_string_lossy(), |n| n.to_string_lossy()).into_owned();
        let full = path.to_string_lossy();
        let room = width - 36.0 - 36.0;
        let name_g = super::elide(ui, &name, FontId::proportional(13.0), t.text, room);
        let shown = super::elide_middle(ui, &full, &FontId::proportional(11.0), room);
        let path_g = ui.painter().layout_no_wrap(shown, FontId::proportional(11.0), t.muted);
        let top = row.center().y - (name_g.size().y + path_g.size().y) / 2.0;
        let x = row.left() + 36.0;
        let below = top + name_g.size().y;
        p.galley(egui::pos2(x, top), name_g, t.text);
        p.galley(egui::pos2(x, below), path_g, t.muted);
        let x_rect = egui::Rect::from_center_size(egui::pos2(row.right() - 20.0, row.center().y), vec2(24.0, 24.0));
        let label = format!("Remove {name}");
        let removed = ui
            .scope_builder(egui::UiBuilder::new().max_rect(x_rect).id_salt(("kit-folder-remove", i)), |ui| {
                ui.add_enabled_ui(!busy, |ui| crate::icons::sized_button(ui, Icon::Close, &label, vec2(24.0, 24.0), 10.0, Some(t.muted)).clicked()).inner
            })
            .inner;
        if removed {
            event = Some(FolderListEvent::Remove(i));
        }
    }
    ui.add_space(6.0);
    let choose = ui.add_enabled_ui(!busy, |ui| super::quiet_button(ui, Some(Icon::Plus), "Choose folder…")).inner;
    if choose.clicked() {
        event = Some(FolderListEvent::Choose);
    }
    event
}

/// A folder's row in a [`folder_list`]: its name over its path.
const FOLDER_ROW_H: f32 = 44.0;

/// A hairline either side of a small "or", between two ways in.
pub fn or_divider(ui: &mut Ui) {
    let t = Tokens::of(ui);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 18.0), Sense::hover());
    let p = ui.painter();
    let galley = p.layout_no_wrap("or".to_owned(), FontId::proportional(11.0), t.muted);
    let gap = galley.size().x / 2.0 + 10.0;
    let (y, mid) = (rect.center().y, rect.center().x);
    let stroke = egui::Stroke::new(1.0, t.hairline);
    p.hline(rect.left()..=mid - gap, y, stroke);
    p.hline(mid + gap..=rect.right(), y, stroke);
    p.galley(rect.center() - galley.size() / 2.0, galley, t.muted);
}

/// Something the form wants read but can't act on, outlined: a short title,
/// then the detail in muted text.
pub fn notice(ui: &mut Ui, title: &str, body: &str) {
    let t = Tokens::of(ui);
    egui::Frame::new().stroke(egui::Stroke::new(1.0, t.hairline)).corner_radius(8).inner_margin(egui::Margin::same(12)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 2.0;
        ui.label(RichText::new(title).font(FontId::new(12.5, semibold())).color(t.text));
        ui.add(egui::Label::new(RichText::new(body).size(12.0).color(t.muted)).wrap());
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::Queryable;

    #[test]
    fn a_field_is_named_by_its_label_and_its_error_by_the_field() {
        let mut h = super::super::themed(String::new(), |ui, text| {
            TextField::new("Server").hint("example.com").error(Some("Couldn't reach it.")).help(Some("Its address")).show(ui, text);
        });
        h.run_steps(2);
        assert!(h.query_by_role_and_label(egui::accesskit::Role::TextInput, "Server").is_some());
        assert!(h.query_by_label("Server: Couldn't reach it.").is_some());
        assert!(h.query_by_label("Its address").is_some());
    }

    #[test]
    fn a_busy_button_says_what_it_waits_for_and_takes_no_clicks() {
        let mut h = super::super::themed(0, |ui, clicks| {
            if form_button(ui, "Sign in", true, None).clicked() {
                *clicks += 1;
            }
            if form_button(ui, "Connecting…", true, Some(false)).clicked() {
                *clicks += 10;
            }
        });
        h.run_steps(2);
        h.get_by_label("Sign in").click();
        h.get_by_label("Connecting…").click();
        h.run_steps(2);
        assert_eq!(h.state().1, 1);
    }

    #[test]
    fn a_folder_list_names_each_remove_and_offers_another_folder() {
        let folders = vec![std::path::PathBuf::from("/Users/me/Music"), std::path::PathBuf::from("/Volumes/Archive/Lossless")];
        let mut h = super::super::themed(Vec::new(), move |ui, seen: &mut Vec<FolderListEvent>| {
            ui.set_width(320.0);
            seen.extend(folder_list(ui, &folders, "No folders yet", false));
        });
        h.run_steps(2);
        h.get_by_label("Remove Lossless").click();
        h.run_steps(2);
        h.get_by_label("Choose folder…").click();
        h.run_steps(2);
        assert_eq!(h.state().1, [FolderListEvent::Remove(1), FolderListEvent::Choose]);
    }

    #[test]
    fn an_empty_folder_list_says_so_and_a_busy_one_takes_no_clicks() {
        let mut h = super::super::themed(Vec::new(), |ui, seen: &mut Vec<FolderListEvent>| {
            ui.set_width(320.0);
            seen.extend(folder_list(ui, &[], "No folders yet", true));
        });
        h.run_steps(2);
        assert!(h.query_by_label("No folders yet").is_some());
        h.get_by_label("Choose folder…").click();
        h.run_steps(2);
        assert!(h.state().1.is_empty());
    }

    /// Renders an empty folder list and one with two folders, to look at:
    /// `cargo test -p shell --release snapshots -- --ignored` writes
    /// `target/snapshots/kit-folder-list-*.png`.
    #[test]
    #[ignore = "writes pictures to look at"]
    fn snapshots() {
        let out = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
        std::fs::create_dir_all(&out).unwrap();
        for (theme, pref) in [("light", crate::theme::ThemePref::Light), ("dark", crate::theme::ThemePref::Dark)] {
            for (name, folders) in [
                ("empty", Vec::new()),
                ("two", vec![std::path::PathBuf::from("/Users/demo/Music"), std::path::PathBuf::from("/Volumes/Archive/Lossless/Classical and Jazz")]),
            ] {
                let mut h = super::super::canvas(vec2(360.0, 200.0), pref, move |ui| {
                    egui::Frame::new().inner_margin(20).show(ui, |ui| {
                        ui.set_width(320.0);
                        folder_list(ui, &folders, "No folders yet", false);
                    });
                });
                h.run();
                h.render().unwrap().save(out.join(format!("kit-folder-list-{name}-{theme}.png"))).unwrap();
            }
        }
    }
}
