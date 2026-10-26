//! Work each frame does around the drawing: the colors from the album art,
//! the blurred cover behind the Player and Mini, the waveform for the seek
//! bar, scroll bars that show while scrolling, and what's being dragged.

use super::{DragTracks, OndApp};
use crate::nav::Route;
use crate::theme::{self, Tokens};
use crate::window::WindowMode;
use std::time::Duration;

impl OndApp {
    /// The colors in use: the chosen accent, or the album art's when colors
    /// adapt to it. Two tints ease on their own: the playing cover's, for the
    /// now-playing display, the Player, Mini and the window around the page;
    /// and the page's, which on an album page is that album's and elsewhere
    /// the playing one's. The first goes where `Tokens::of` finds it;
    /// `full_ui` swaps in the second around the page. A cover's colors are
    /// worked out in the background: until they arrive the old ones stay.
    pub(super) fn art_tint(&mut self, ctx: &egui::Context) {
        let accent = self.ui.prefs.accent;
        if self.ui.accent_applied != Some(accent) {
            theme::set_accent(ctx, accent);
            self.ui.accent_applied = Some(accent);
        }
        let base = self.ui.tint_cache.base(accent);
        let adapt = self.ui.prefs.adapt_colors;
        // The covers' ids are borrowed (this runs every frame): a cover's tints
        // once its look is worked out, `None` meanwhile.
        let full = self.ui.prefs.mode == WindowMode::Full;
        let (art, cache) = (&mut self.art, &mut self.ui.tint_cache);
        let mut cover_tints = |cover: &str| art.look(cover).map(|look| cache.cover(cover, look.palette.as_ref()));
        let playing_cover = self.player.queue.current.as_ref().and_then(|c| self.data.cover_for(&c.track));
        let playing = match playing_cover.filter(|_| adapt) {
            Some(cover) => cover_tints(cover).or(self.ui.tint.target()).unwrap_or(base),
            None => base,
        };
        let page_cover = match self.ui.nav.current() {
            Route::Album(id) if full => Some(self.data.album(id).and_then(|a| a.cover_art.as_deref())),
            _ => None,
        };
        let page = match page_cover {
            Some(Some(cover)) if adapt => cover_tints(cover).or(self.ui.page_tint.target()).unwrap_or(playing),
            Some(None) if adapt => base,
            _ => playing,
        };
        let now = ctx.input(|i| i.time);
        let (playing, moving) = self.ui.tint.step(playing, now);
        let (page, page_moving) = self.ui.page_tint.step(page, now);
        self.ui.page_tints = Some(page);
        theme::set_tints(ctx, playing);
        if moving || page_moving {
            ctx.request_repaint();
        }
    }

    /// The cover the page takes its colors from: `Some` on an album page in
    /// the full window (`Some(None)` for an album without one), `None` on
    /// any other page. (`art_tint` works it out in place.)
    #[cfg(test)]
    pub(super) fn page_cover(&self) -> Option<Option<String>> {
        match self.ui.nav.current() {
            Route::Album(id) if self.prefs_now().mode == WindowMode::Full => Some(self.data.album(id).and_then(|a| a.cover_art.clone())),
            _ => None,
        }
    }

    /// Waveform seek bar (experimental): has the playing track's peaks read
    /// once a complete copy is on disk (downloaded, or fully streamed).
    pub(super) fn want_waveform(&mut self) {
        self.waveforms.poll();
        if !self.ui.prefs.experimental.waveform {
            return;
        }
        let Some(track) = self.player.queue.current.as_ref().map(|c| &c.track) else { return };
        let (downloads, cache, transcode) = (&self.downloads, &self.cache, self.output.transcode());
        self.waveforms.want(&track.id, track.suffix.as_deref(), || {
            downloads
                .local_path(&track.id)
                .or_else(|| cache.complete_path(&streamcache::CacheKey::original(track.id.clone(), track.suffix.clone())))
                .or_else(|| transcode.and_then(|t| cache.complete_path(&streamcache::CacheKey::transcode(&track.id, &t.format, t.kbps))))
        });
    }

    /// The blurred cover to draw behind the Player or Mini (experimental),
    /// oldest layer first; empty when that's off or nothing plays.
    pub(crate) fn backdrop_layers(&mut self, ctx: &egui::Context) -> Vec<crate::visuals::backdrop::Layer> {
        let cover = self.player.queue.current.as_ref().and_then(|c| self.data.cover_for(&c.track).map(str::to_owned));
        let Some(cover) = cover.filter(|_| self.ui.prefs.experimental.backdrop) else {
            self.ui.backdrop.clear();
            return Vec::new();
        };
        let look = self.art.look(&cover);
        let next = look.as_ref().and_then(|l| l.backdrop.as_ref());
        let (layers, fading) = self.ui.backdrop.step(next.map(|b| (cover.as_str(), b)), ctx.input(|i| i.time));
        if fading {
            ctx.request_repaint();
        }
        layers
    }

    /// What's being dragged, following the pointer.
    pub(super) fn drag_ghost(&self, ctx: &egui::Context) {
        let Some(drag) = egui::DragAndDrop::payload::<DragTracks>(ctx) else { return };
        let Some(at) = ctx.pointer_interact_pos() else { return };
        let t = Tokens::of_ctx(ctx);
        let text = match drag.tracks.len() {
            1 => drag.tracks[0].title.clone(),
            n => format!("{n} songs"),
        };
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("drag-ghost")));
        let font = egui::FontId::proportional(12.0);
        let galley = painter.layout_no_wrap(text, font, t.text);
        let rect = egui::Rect::from_min_size(at + egui::vec2(14.0, 10.0), galley.size() + egui::vec2(20.0, 10.0));
        painter.rect(rect, 6.0, t.panel, egui::Stroke::new(1.0, t.hairline), egui::StrokeKind::Inside);
        painter.galley(rect.min + egui::vec2(10.0, 5.0), galley, t.text);
    }

    /// Scroll bars show while scrolling and fade a second after.
    pub(super) fn scroll_bars(&mut self, ctx: &egui::Context) {
        let (now, scrolling) = ctx.input(|i| (i.time, i.smooth_scroll_delta != egui::Vec2::ZERO || i.events.iter().any(|e| matches!(e, egui::Event::MouseWheel { .. }))));
        if scrolling {
            self.ui.last_scroll = now;
        }
        let since = now - self.ui.last_scroll;
        let show = since < 1.0;
        if show != self.ui.bars_shown {
            self.ui.bars_shown = show;
            theme::scroll_bars(ctx, show);
        }
        if show {
            ctx.request_repaint_after(Duration::from_secs_f64(1.0 - since + 0.02));
        }
    }
}
