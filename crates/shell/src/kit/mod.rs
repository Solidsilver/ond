//! The look both apps are drawn with: every widget whose shape, colours and
//! spacing should match wherever it appears. A widget belongs here when
//! either app could use it as it is; what only makes sense in one app (Ond's
//! managed-settings tag, a track's colour) stays there as a thin wrapper.
//! Everything is re-exported here, so callers write `kit::switch`.
//!
//! - [`text`]: [`elide`], [`elide_middle`], [`elide_job`], the quiet
//!   [`link`] and [`link_line`] that underline on hover, a page's title
//!   ([`page_heading`]), the muted
//!   [`section`] label, [`key_cap`] (a combo drawn as a key), [`chip`] (a
//!   finding in tinted text), and the one [`empty_state`].
//! - [`buttons`]: the filled [`primary`], [`secondary`], [`danger`] (and
//!   [`primary_unavailable`], [`secondary_danger`]), the [`quiet_button`],
//!   [`heart_button`] and [`hover_heart`], [`tile_button`] (round, on a
//!   tile's art), [`chip_button`] (a small tinted chip, Rede's speed and
//!   sleep timer),
//!   and the [`focus_ring`] every self-painted control shows under Tab.
//! - [`controls`]: the segmented pill ([`segmented`], [`segmented_wide`],
//!   [`choice`]; [`icon_choice`] for icons), [`thin_slider`] (and [`thin_slider_marked`]; [`step_slider`] steps a
//!   whole number on one, beside its [`readout`]), [`switch`]
//!   (and [`switch_tinted`]), [`stepper`] and its [`step_button`], and
//!   [`choice_chip`] (one of a grid of choices, lit when on).
//! - [`rows`]: a [`group`] of [`setting`]s (or [`row_with`] and its
//!   [`Head`]), hairlines between them ([`divider`]), [`managed`] to lock
//!   one; [`kv_section`], [`kv_row`] and [`kv_text`] (a label and its value,
//!   the values lined up, as an info panel lists facts); [`row_fill`] (a list
//!   row's selection or hover); and [`sidebar_row`].
//! - [`menus`]: [`popover`] (or [`popover_at`]) in the theme's frame,
//!   [`padded`] to line a block up with its rows, [`popover_title`],
//!   [`popover_rule`], [`menu_row`] (a command), [`info_row`] (a fact),
//!   [`switch_row`], and [`menu_label`] (a quiet button with a chevron that
//!   opens one).
//! - [`sheets`]: [`sheet_frame`] (the frame a sheet over the window is drawn
//!   in), [`sheet_header`] (its title, and a × that closes it), and the
//!   [`shortcuts_sheet`] that sets out an app's [`ShortcutGroup`]s.
//! - [`settings`]: the Settings window over the app ([`settings_window`],
//!   which says what happened in a [`SettingsWindow`]), its tab list
//!   ([`settings_nav`], its width [`SETTINGS_NAV_W`]), the pane beside it
//!   ([`settings_pane`]), and About's [`about_header`] with the
//!   [`app_icon`].
//! - [`toasts`]: [`Toasts`], the messages over the foot of the window that
//!   fade, or stay with a button until clicked away (a [`Toasts::note`]
//!   fades with its button).

pub mod buttons;
pub mod controls;
pub mod menus;
pub mod rows;
pub mod settings;
pub mod sheets;
pub mod text;
pub mod toasts;

pub use buttons::*;
pub use controls::*;
pub use menus::*;
pub use rows::*;
pub use settings::*;
pub use sheets::*;
pub use text::*;
pub use toasts::Toasts;

/// A harness whose first frame installs the theme (its fonts arrive a
/// frame later) and whose later frames draw `body`.
#[cfg(test)]
fn themed<S: 'static>(state: S, mut body: impl FnMut(&mut egui::Ui, &mut S) + 'static) -> egui_kittest::Harness<'static, (bool, S)> {
    egui_kittest::Harness::builder().build_ui_state(
        move |ui, (installed, state): &mut (bool, S)| {
            if !std::mem::replace(installed, true) {
                crate::theme::install(ui.ctx(), crate::theme::ThemePref::Light);
                ui.ctx().request_repaint();
                return;
            }
            body(ui, state);
        },
        (false, state),
    )
}
