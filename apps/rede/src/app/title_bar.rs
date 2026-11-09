//! The title bar: the window's drag area, the mode buttons and the account.

use super::{Action, RedeApp};
use shell::theme::{self, Tokens};
use std::time::Duration;

impl RedeApp {
    /// The title bar: the account's menu at the right, and the window's own
    /// drag and double-click everywhere else.
    pub(super) fn title_bar(&mut self, ui: &mut egui::Ui) {
        let mut full = ui.available_rect_before_wrap();
        shell::csd::set_bar(ui.ctx(), full, None);
        // The bar is the window's title bar too: its empty parts move the
        // window, and a double-click zooms it. Widgets drawn after this take
        // the pointer where they are.
        let whole = full.expand2(egui::vec2(12.0, 0.0));
        let bar = ui.interact(whole, ui.scope_id().with("titlebar"), egui::Sense::click_and_drag());
        if bar.drag_started() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        if bar.double_clicked() {
            shell::chrome::title_double_click(ui.ctx());
        }
        // Our own window buttons sit at one end on Linux; the traffic lights at
        // the other on macOS.
        use shell::csd::{Side, reserved};
        full.max.x -= reserved(ui.ctx(), Side::Right);
        let right = egui::Rect::from_min_max(egui::pos2(full.right() - 240.0, full.top()), full.max);
        ui.scope_builder(egui::UiBuilder::new().max_rect(right).layout(egui::Layout::right_to_left(egui::Align::Center)), |ui| {
            if self.mode == crate::window::Mode::Full {
                self.title_right(ui);
            }
            self.view().mode_buttons(ui);
        });
    }

    /// The syncing indicator and the account menu, at the right of the bar.
    fn title_right(&mut self, ui: &mut egui::Ui) {
        let t = Tokens::of(ui);
        ui.spacing_mut().item_spacing.x = 10.0;
        if let Some(label) = self.sync_label() {
            ui.label(egui::RichText::new(label).size(12.0).color(t.muted));
            syncing_glyph(ui, &t);
        } else {
            ui.data_mut(|d| d.remove::<f64>(syncing_since_id()));
        }
        let Some(account) = self.session.account.clone() else {
            ui.label(egui::RichText::new("Not signed in").size(12.0).color(t.muted));
            return;
        };
        let button = account_button(ui, &t, &account.username);
        egui::Popup::menu(&button).show(|ui| {
            ui.set_min_width(200.0);
            ui.label(egui::RichText::new(&account.username).font(egui::FontId::new(13.0, theme::semibold())).color(t.text));
            ui.label(egui::RichText::new(&account.server).size(11.0).color(t.muted));
            ui.separator();
            if ui.button("Sign out").clicked() {
                ui.close();
                self.actions.push(Action::SignOut);
            }
        });
    }
}

/// The sync spinner: it turns while a sync runs, then stops after
/// [`SYNC_SPIN`], so a listing that stalls can't hold the window at 20 frames a
/// second for ever. The start is kept in egui's own memory, per window.
fn syncing_glyph(ui: &mut egui::Ui, t: &Tokens) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
    shell::icons::paint(ui.painter(), rect, shell::icons::Icon::Spinner, t.muted);
    let now = ui.input(|i| i.time);
    let since = ui.data_mut(|d| *d.get_temp_mut_or_insert_with(syncing_since_id(), || now));
    if now - since < SYNC_SPIN {
        ui.ctx().request_repaint_after(SPIN_FRAME);
    }
}

/// The account's control: a person and the name, quiet at rest like Ond's
/// toolbar buttons, with a fill on hover and a deeper one while pressed or
/// while its menu is open. It's named by the account, so it reads (and is
/// found) as who is signed in.
fn account_button(ui: &mut egui::Ui, t: &Tokens, name: &str) -> egui::Response {
    let font = egui::FontId::proportional(12.5);
    // Laid out without a colour, so it's painted in whichever ink the state
    // calls for.
    let galley = ui.painter().layout_no_wrap(name.to_owned(), font, egui::Color32::PLACEHOLDER);
    let (icon, chevron, pad, gap) = (13.0, 8.0, 8.0, 6.0);
    let width = pad + icon + gap + galley.size().x + gap + chevron + pad;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, 28.0), egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&resp));
        let fill = if resp.is_pointer_button_down_on() || open {
            t.selected
        } else if resp.hovered() {
            t.hover
        } else {
            egui::Color32::TRANSPARENT
        };
        ui.painter().rect_filled(rect, 6.0, fill);
        shell::widgets::focus_ring(ui, &resp, rect, 6.0);
        let ink = if resp.hovered() || open { t.text } else { t.muted };
        let mut x = rect.left() + pad;
        let y = rect.center().y;
        shell::icons::paint(ui.painter(), egui::Rect::from_center_size(egui::pos2(x + icon / 2.0, y), egui::vec2(icon, icon)), shell::icons::Icon::Person, ink);
        x += icon + gap;
        let text_w = galley.size().x;
        ui.painter().galley(egui::pos2(x, y - galley.size().y / 2.0), galley, ink);
        x += text_w + gap;
        shell::icons::paint(
            ui.painter(),
            egui::Rect::from_center_size(egui::pos2(x + chevron / 2.0, y + 0.5), egui::vec2(chevron, chevron)),
            shell::icons::Icon::ChevronDown,
            t.muted,
        );
    }
    let label = name.to_owned();
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &label));
    resp
}

/// How long the spinner turns before it goes still.
const SYNC_SPIN: f64 = 30.0;
const SPIN_FRAME: Duration = Duration::from_millis(50);

fn syncing_since_id() -> egui::Id {
    egui::Id::unique("sync-since")
}
