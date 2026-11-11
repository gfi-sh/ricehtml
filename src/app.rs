use crate::render::{build, Drawer, Hit, Tree, UiState};
use crate::runtime::{Msg, Shared, NO_HOVER};
use eframe::egui;
use eframe::egui::emath::TSTransform;
use std::sync::{atomic::Ordering, mpsc::Sender, Arc};

pub struct App {
    shared: Arc<Shared>,
    tx: Sender<Msg>,
    bold_font: bool,
    tree: Option<Tree>,
    state: UiState,
    hits: Vec<Hit>,
    /// Last observed pointer position (for hover hysteresis).
    last_pos: Option<egui::Pos2>,
}

impl App {
    pub fn new(shared: Arc<Shared>, tx: Sender<Msg>, bold_font: bool) -> Self {
        App { shared, tx, bold_font, tree: None, state: UiState::default(), hits: vec![], last_pos: None }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let mut out: Vec<Msg> = vec![];

        // Track :hover from last frame's hit rects. Only re-evaluate when the
        // pointer actually moved: if a hover handler (or :hover style) shifts
        // the layout under a *stationary* cursor, re-testing would flip
        // hover on/off every frame — an endless flicker loop.
        let moved = match (self.last_pos, ctx.input(|i| i.pointer.latest_pos())) {
            (None, Some(_)) => true, // first frame with a pointer
            (Some(a), Some(b)) => a != b,
            _ => false,
        };
        self.last_pos = ctx.input(|i| i.pointer.latest_pos());
        if moved {
            if let Some(pos) = self.last_pos {
                let new_hover = self.hits.iter().filter(|h| h.rect.contains(pos))
                    .max_by_key(|h| h.depth).map(|h| h.id).map_or(NO_HOVER, |id| id as usize);
                let old = self.shared.hover.load(Ordering::Relaxed);
                if new_hover != old {
                    self.shared.hover.store(new_hover, Ordering::Relaxed);
                    self.shared.dirty.store(true, Ordering::Release);
                    if old != NO_HOVER { out.push(Msg::Hover { enter: false, target: old }); }
                    if new_hover != NO_HOVER { out.push(Msg::Hover { enter: true, target: new_hover }); }
                    ctx.request_repaint();
                }
            }
        }

        // Rebuild style + layout data ONLY when the DOM changed.
        if self.shared.dirty.swap(false, Ordering::AcqRel) || self.tree.is_none() {
            let hover = {
                let h = self.shared.hover.load(Ordering::Relaxed);
                (h != NO_HOVER).then_some(h as usize)
            };
            let dom = self.shared.dom();
            self.tree = Some(build(&dom, self.bold_font, hover));
            // Re-seed widget state only for elements changed programmatically
            // (set_value / set_checked). Keeping the rest preserves text-input
            // contents and the caret across DOM rebuilds.
            let resets = std::mem::take(&mut *self.shared.reset.lock().unwrap_or_else(|e| e.into_inner()));
            for id in resets {
                let v = dom.attr(id, "value").unwrap_or("").to_string();
                self.state.inputs.insert(id, v);
                self.state.checks.insert(id, dom.attr(id, "checked").is_some());
            }
            drop(dom);
        }
        if let Some(t) = self.shared.title.lock().unwrap_or_else(|e| e.into_inner()).take() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(t));
        }

        self.hits.clear();
        let mut msgs = vec![];
        let Some(tree) = self.tree.as_mut() else { return };
        egui::CentralPanel::default().frame(egui::Frame::new().fill(tree.page_bg)).show(ctx, |ui| {
            let mut d = Drawer {
                shared: &self.shared, ctx: ctx.clone(), hits: &mut self.hits,
                state: &mut self.state, out: &mut msgs, depth: 0,
                ts: TSTransform::IDENTITY, node_rects: Default::default(), no_fill: false,
            };
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |sui| {
                d.draw_box(sui, &mut tree.root, false);
            });
            // Out-of-flow boxes (`absolute`/`fixed`) paint last, using the
            // containing blocks recorded during the main pass. `fixed` boxes
            // land in their own layers, viewport-anchored and unclipped.
            d.draw_oof(ui, &mut tree.oof);
        });
        out.extend(msgs);

        // One hit-test per click instead of one interact() per element.
        if ctx.input(|i| i.pointer.primary_clicked()) {
            if let Some(pos) = ctx.input(|i| i.pointer.interact_pos()) {
                if let Some(h) = self.hits.iter().filter(|h| h.rect.contains(pos)).max_by_key(|h| h.depth) {
                    out.push(Msg::Click { target: h.id, x: pos.x, y: pos.y });
                }
            }
        }
        for m in out { let _ = self.tx.send(m); }
        // No request_repaint here: egui repaints on input, and DOM edits wake it via Shared::invalidate.
    }
}
