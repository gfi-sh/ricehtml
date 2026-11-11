use crate::css::{AlignItems, Display, JustifyContent, Length, Position, Sheet, Style, Xform};
use crate::dom::{is_ws, Dom, Kind, NodeId, ROOT};
use crate::runtime::{get_image, Msg, Shared};
use eframe::egui::{
    self,
    emath::TSTransform,
    epaint::shape_transform::adjust_colors,
    layers::ShapeIdx,
    text::{LayoutJob, TextFormat},
    Align, Align2, Color32, FontFamily, FontId, Layout, Order, Stroke, Vec2,
};
use std::collections::HashMap;
use std::sync::Arc;

// ================================================================= render tree
// Built once per DOM change; walked every frame without touching the DOM.

pub struct Tree { pub root: RBox, pub page_bg: Color32, pub oof: Vec<Oof> }
pub struct RBox { pub id: NodeId, pub style: Style, pub content: Content }
pub enum Content { Items(Vec<Item>), Widget(Widget), Hr }
pub enum Item { Box(RBox), Inline { segs: Vec<Seg>, align: Align } }
pub enum Seg { Text(LayoutJob), Widget(RBox) }
pub enum Widget {
    Button { text: String, disabled: bool },
    Link { text: String, href: String },
    Text { value: String, hint: String, password: bool, multiline: bool, disabled: bool },
    Check { checked: bool, disabled: bool },
    Image { src: String },
    Canvas { w: u32, h: u32 },
}

/// An out-of-flow (`position: absolute` / `fixed`) box, extracted at build
/// time so the paint pass never mutates the tree. `cb` is the node id of the
/// containing block (nearest positioned ancestor; `None` = root/body).
pub struct Oof { pub cb: Option<NodeId>, pub fixed: bool, pub rbox: RBox }

pub fn build(dom: &Dom, bold_font: bool, hover: Option<NodeId>) -> Tree {
    let hovered = move |n: NodeId| Some(n) == hover;
    let authors: Vec<String> = dom.descendants(ROOT).into_iter()
        .filter(|&n| dom.tag(n) == Some("style")).map(|n| dom.text_content(n)).collect();
    let sheet = Sheet::new(&authors);
    let mut b = Builder { dom, sheet: &sheet, bold: bold_font, oof: vec![] };
    let base = Style::root();
    let html = dom.find_tag(ROOT, "html");
    let hstyle = html.map(|h| sheet.compute(dom, h, &base, &hovered)).unwrap_or(base);
    let body = dom.find_tag(ROOT, "body");
    let (cid, mut cstyle) = match body {
        Some(bd) => (bd, sheet.compute(dom, bd, &hstyle, &hovered)),
        None => (ROOT, hstyle),
    };
    let page_bg = cstyle.background.or(hstyle.background).unwrap_or(Color32::WHITE);
    if cstyle.display == Display::None { cstyle.display = Display::Block; }
    hide_colors(&mut cstyle);
    let items = b.build_children(cid, &cstyle, None, None);
    let oof = std::mem::take(&mut b.oof);
    Tree { root: RBox { id: cid, style: cstyle, content: Content::Items(items) }, page_bg, oof }
}

/// `visibility: hidden` — neutralize colors up front so text/bg is invisible
/// while the element still occupies its space.
fn hide_colors(st: &mut Style) {
    if st.hidden {
        st.color = Color32::TRANSPARENT;
        st.background = None;
        st.border_width = 0.0;
    }
}

struct Builder<'a> { dom: &'a Dom, sheet: &'a Sheet, bold: bool, oof: Vec<Oof> }

impl Builder<'_> {
    fn build_children(&mut self, parent: NodeId, st: &Style, marker: Option<String>, cb: Option<NodeId>) -> Vec<Item> {
        let flex = st.display == Display::Flex;
        let (mut items, mut ib) = (vec![], InlineBuilder::default());
        if let Some(m) = marker { ib.text(&m, st, self.bold); }
        self.walk(parent, st, &mut ib, &mut items, flex, cb);
        flush(&mut ib, &mut items, st);
        if !flex { collapse_margins(&mut items); }
        items
    }

    fn walk(&mut self, parent: NodeId, pst: &Style, ib: &mut InlineBuilder, items: &mut Vec<Item>, flex: bool, cb_node: Option<NodeId>) {
        let mut li = 0;
        let children = self.dom.children(parent).to_vec();
        for c in children {
            match &self.dom.node(c).kind {
                Kind::Text(t) => {
                    let t = t.clone();
                    if flex {
                        if !t.chars().all(is_ws) {
                            let mut b = InlineBuilder::default();
                            b.text(&t, pst, self.bold);
                            flush(&mut b, items, pst);
                        }
                    } else { ib.text(&t, pst, self.bold); }
                }
                Kind::Element { .. } => {
                    let mut st = self.sheet.compute(self.dom, c, pst, &|_| false);
                    if st.display == Display::None { continue; }
                    hide_colors(&mut st);
                    let tag = self.dom.tag(c).unwrap_or("").to_string();
                    let is_li = tag == "li";
                    if is_li { li += 1; }

                    // Out-of-flow: extract at build time (never mutates the tree
                    // during paint, so repaints without rebuilds stay correct).
                    if st.position == Position::Absolute || st.position == Position::Fixed {
                        // Build the subtree first (nested out-of-flow boxes get
                        // extracted), keeping them ordered after this box.
                        let saved = std::mem::take(&mut self.oof);
                        let content = Content::Items(self.build_children(c, &st, None, Some(c)));
                        let nested = std::mem::replace(&mut self.oof, saved);
                        self.oof.push(Oof {
                            cb: if st.position == Position::Fixed { None } else { cb_node.or(Some(parent)) },
                            fixed: st.position == Position::Fixed,
                            rbox: RBox { id: c, style: st, content },
                        });
                        self.oof.extend(nested);
                        continue;
                    }

                    match tag.as_str() {
                        "br" => ib.newline(&st, self.bold),
                        "hr" => {
                            flush(ib, items, pst);
                            items.push(Item::Box(RBox { id: c, style: st, content: Content::Hr }));
                        }
                        "img" => {
                            let src = self.dom.attr(c, "src").unwrap_or("").to_string();
                            if src.is_empty() { continue; }
                            let b = RBox { id: c, style: st, content: Content::Widget(Widget::Image { src }) };
                            if flex { items.push(Item::Box(b)); } else { ib.widget(b, pst, self.bold); }
                        }
                        "canvas" => {
                            let dim = |a: &str| self.dom.attr(c, a).and_then(|v| v.trim().parse::<u32>().ok()).unwrap_or(0);
                            let b = RBox { id: c, style: st, content: Content::Widget(Widget::Canvas {
                                w: if dim("width") > 0 { dim("width") } else { 300 },
                                h: if dim("height") > 0 { dim("height") } else { 150 },
                            }) };
                            if flex { items.push(Item::Box(b)); } else { ib.widget(b, pst, self.bold); }
                        }
                        "video" | "audio" | "svg" | "select" | "option" | "iframe" | "object" | "embed" => {}
                        "button" | "a" | "input" | "textarea" => {
                            if let Some(w) = self.widget(c, &tag) {
                                let b = RBox { id: c, style: st, content: Content::Widget(w) };
                                if flex { items.push(Item::Box(b)); } else { ib.widget(b, pst, self.bold); }
                            }
                        }
                        _ => {
                            if flex || st.display != Display::Inline {
                                flush(ib, items, pst);
                                let marker = is_li.then(|| {
                                    if self.dom.tag(parent) == Some("ol") { format!("{li}. ") } else { "• ".to_string() }
                                });
                                let nested_cb = if st.positioned() { Some(c) } else { cb_node };
                                let content = Content::Items(self.build_children(c, &st, marker, nested_cb));
                                items.push(Item::Box(RBox { id: c, style: st, content }));
                            } else {
                                let nested_cb = if st.positioned() { Some(c) } else { cb_node };
                                self.walk(c, &st, ib, items, false, nested_cb);
                            }
                        }
                    }
                }
            }
        }
    }

    fn widget(&self, id: NodeId, tag: &str) -> Option<Widget> {
        let d = self.dom;
        let disabled = d.attr(id, "disabled").is_some();
        let text = || collapse(&d.text_content(id));
        match tag {
            "button" => Some(Widget::Button { text: text(), disabled }),
            "a" => { let t = text(); if t.is_empty() { None } else { Some(Widget::Link { text: t, href: d.attr(id, "href").unwrap_or("").to_string() }) } }
            "textarea" => Some(Widget::Text {
                value: d.attr(id, "value").map(str::to_string).unwrap_or_else(|| d.text_content(id)),
                hint: d.attr(id, "placeholder").unwrap_or("").to_string(), password: false, multiline: true, disabled,
            }),
            "input" => {
                let ty = d.attr(id, "type").unwrap_or("text").to_ascii_lowercase();
                match ty.as_str() {
                    "checkbox" | "radio" => Some(Widget::Check { checked: d.attr(id, "checked").is_some(), disabled }),
                    "button" | "submit" | "reset" => Some(Widget::Button { text: d.attr(id, "value").unwrap_or("Submit").to_string(), disabled }),
                    "hidden" | "image" | "file" | "range" | "color" => None,
                    _ => Some(Widget::Text {
                        value: d.attr(id, "value").unwrap_or("").to_string(),
                        hint: d.attr(id, "placeholder").unwrap_or("").to_string(),
                        password: ty == "password", multiline: false, disabled,
                    }),
                }
            }
            _ => None,
        }
    }
}

fn collapse(s: &str) -> String { s.split(is_ws).filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" ") }

fn flush(ib: &mut InlineBuilder, items: &mut Vec<Item>, st: &Style) {
    if let Some(it) = std::mem::take(ib).finish(st.text_align) { items.push(it); }
}

/// Approximate CSS vertical margin collapsing between adjacent block siblings.
fn collapse_margins(items: &mut [Item]) {
    let mut prev: Option<f32> = None;
    for it in items.iter_mut() {
        match it {
            Item::Box(b) => {
                if let Some(pb) = prev { let c = pb.min(b.style.margin[0]); b.style.margin[0] -= c; }
                prev = Some(b.style.margin[2]);
            }
            _ => prev = None,
        }
    }
}

#[derive(Default)]
struct InlineBuilder { segs: Vec<Seg>, job: LayoutJob, pending: bool }

fn text_format(st: &Style, bold_font: bool) -> TextFormat {
    let family = if st.mono { FontFamily::Monospace }
        else if st.bold && bold_font { FontFamily::Name("bold".into()) }
        else { FontFamily::Proportional };
    TextFormat {
        font_id: FontId::new(st.font_size, family),
        color: st.color,
        italics: st.italic,
        underline: if st.underline { Stroke::new(1.0, st.color) } else { Stroke::NONE },
        strikethrough: if st.strike { Stroke::new(1.0, st.color) } else { Stroke::NONE },
        ..Default::default()
    }
}

impl InlineBuilder {
    fn has_content(&self) -> bool { !self.job.text.is_empty() || !self.segs.is_empty() }

    fn text(&mut self, s: &str, st: &Style, bold: bool) {
        let f = text_format(st, bold);
        if st.pre {
            if !s.is_empty() { self.job.append(s, 0.0, f); self.pending = false; }
            return;
        }
        let starts = s.chars().next().map_or(false, is_ws);
        let ends = s.chars().last().map_or(false, is_ws);
        let out = s.split(is_ws).filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" ");
        if out.is_empty() { if starts { self.pending = true; } return; }
        let lead = (self.pending || starts) && self.has_content();
        self.job.append(&if lead { format!(" {out}") } else { out }, 0.0, f);
        self.pending = ends;
    }

    fn newline(&mut self, st: &Style, bold: bool) {
        self.job.append("\n", 0.0, text_format(st, bold));
        self.pending = false;
    }

    fn widget(&mut self, b: RBox, st: &Style, bold: bool) {
        if self.pending && self.has_content() { self.job.append(" ", 0.0, text_format(st, bold)); }
        self.pending = false;
        self.flush_job();
        self.segs.push(Seg::Widget(b));
    }

    fn flush_job(&mut self) {
        if !self.job.text.is_empty() { self.segs.push(Seg::Text(std::mem::take(&mut self.job))); }
    }

    fn finish(mut self, align: Align) -> Option<Item> {
        self.flush_job();
        if self.segs.is_empty() { None } else { Some(Item::Inline { segs: self.segs, align }) }
    }
}

// ================================================================= drawing

pub struct Hit { pub id: NodeId, pub rect: egui::Rect, pub depth: u32 }

#[derive(Default)]
pub struct UiState {
    pub inputs: HashMap<NodeId, String>,
    pub checks: HashMap<NodeId, bool>,
    pub tex: HashMap<String, egui::TextureHandle>,
    pub canvas_ver: HashMap<NodeId, u64>,
}

pub struct Drawer<'a> {
    pub shared: &'a Arc<Shared>,
    pub ctx: egui::Context,
    pub hits: &'a mut Vec<Hit>,
    pub state: &'a mut UiState,
    pub out: &'a mut Vec<Msg>,
    pub depth: u32,
    /// Transforms of ancestors that are applied *after* the current box paints
    /// (used to move recorded hit rects into final screen space).
    pub ts: TSTransform,
    /// Rect of every box painted this frame, keyed by node id. Used to resolve
    /// the containing block of `position: absolute` boxes (which are extracted
    /// at build time and painted after the main pass).
    pub node_rects: std::collections::HashMap<NodeId, egui::Rect>,
    /// Set while drawing out-of-flow (`absolute`/`fixed`) boxes so they
    /// shrink-wrap instead of filling their placement band.
    pub no_fill: bool,
}

fn margin_of(m: [f32; 4]) -> egui::Margin {
    let c = |v: f32| v.round().clamp(0.0, 127.0) as i8;
    egui::Margin { left: c(m[3]), right: c(m[1]), top: c(m[0]), bottom: c(m[2]) }
}

/// Apply `a` first, then `b`.
fn compose(a: TSTransform, b: TSTransform) -> TSTransform {
    TSTransform::new(
        Vec2::new(b.scaling * a.translation.x + b.translation.x, b.scaling * a.translation.y + b.translation.y),
        a.scaling * b.scaling,
    )
}

fn pct(l: Length, tot: f32) -> Option<f32> {
    match l { Length::Px(v) => Some(v), Length::Percent(v) => Some(tot * v / 100.0), Length::Auto => None }
}

/// Final size of a replaced element (image/canvas/…) given its natural size.
fn size_for(st: &Style, ui: &egui::Ui, natural: Vec2) -> Vec2 {
    let aw = ui.available_width();
    let ah = ui.available_height();
    let x = match st.width { Length::Px(v) => v, Length::Percent(p) => aw * p / 100.0, _ => natural.x };
    let y = match st.height { Length::Px(v) => v, Length::Percent(p) => ah * p / 100.0, _ => natural.y };
    Vec2::new(x.max(0.0), y.max(0.0))
}

impl Drawer<'_> {
    fn items(&mut self, ui: &mut egui::Ui, items: &mut [Item], row: bool) {
        for it in items {
            match it {
                // Out-of-flow boxes are not in the tree's item lists: they are
                // extracted at build time into `Tree::oof` and painted after
                // the main pass.
                Item::Box(b) => { self.draw_box(ui, b, row); }
                Item::Inline { segs, align } => self.inline(ui, segs, *align),
            }
        }
    }

    pub fn draw_box(&mut self, ui: &mut egui::Ui, b: &mut RBox, in_row: bool) -> egui::Rect {
        self.depth += 1;
        let s = b.style; // Style is Copy
        let hidden = s.hidden;

        // Predicted top-left of the box (layout cursor + outer margin): known
        // before painting, so child hit rects can account for our transform.
        let own_ts = if s.transform.is_identity() { None } else {
            let o = ui.cursor().min + Vec2::new(s.margin[3], s.margin[0]);
            let t: Xform = s.transform;
            Some(TSTransform::new(Vec2::new(t.tx + o.x * (1.0 - t.s), t.ty + o.y * (1.0 - t.s)), t.s))
        };
        let saved_ts = self.ts;
        if let Some(t) = own_ts { self.ts = compose(t, saved_ts); }

        let start = self.ctx.graphics(|g| g.get(ui.layer_id()).map_or(ShapeIdx(0), |l| l.next_idx()));
        let rect = match &mut b.content {
            Content::Hr => {
                ui.add_space(s.margin[0]);
                let r = ui.separator().rect;
                ui.add_space(s.margin[2]);
                r
            }
            Content::Widget(w) => self.widget(ui, b.id, &s, w),
            Content::Items(items) => self.container(ui, &s, items, in_row),
        };
        let end = self.ctx.graphics(|g| g.get(ui.layer_id()).map_or(ShapeIdx(0), |l| l.next_idx()));
        self.ts = saved_ts;

        // Transform: moves the shapes just painted (does not affect flow).
        if let Some(t) = own_ts {
            self.ctx.graphics_mut(|g| { g.entry(ui.layer_id()).transform_range(start, end, t); });
        }
        // Opacity: multiply every painted color of this subtree.
        if s.opacity < 1.0 {
            let op = s.opacity;
            self.ctx.graphics_mut(|g| {
                let list = g.entry(ui.layer_id());
                for i in start.0..end.0 {
                    list.mutate_shape(ShapeIdx(i), |cs| adjust_colors(&mut cs.shape, move |c| *c = c.gamma_multiply(op)));
                }
            });
        }

        let mut hit = rect.intersect(ui.clip_rect());
        let t = own_ts.map_or(saved_ts, |o| compose(o, saved_ts));
        hit = t.mul_rect(hit);
        if !hidden {
            self.hits.push(Hit { id: b.id, rect: hit, depth: self.depth });
        }
        self.node_rects.insert(b.id, rect);
        self.depth -= 1;
        rect
    }

    fn container(&mut self, ui: &mut egui::Ui, s: &Style, items: &mut [Item], in_row: bool) -> egui::Rect {
        let avail = ui.available_width();
        let block = !in_row && s.display != Display::Inline;
        let mut width = match s.width {
            Length::Px(w) => Some(w),
            Length::Percent(p) => Some((avail * p / 100.0).max(0.0)),
            Length::Auto => s.max_width.filter(|_| block).map(|m| m.min(avail)),
        };
        if let Some(w) = &mut width {
            if let Some(mw) = pct(s.min_width, avail) { *w = w.max(mw); }
            if let Some(m) = s.max_width { *w = w.min(m); }
        }
        let fill = width.is_none() && block && !self.no_fill;
        let chrome = s.background.is_some() || s.border_width > 0.0
            || s.padding.iter().any(|&p| p > 0.0) || s.margin.iter().any(|&m| m > 0.0);

        let mut body = |this: &mut Self, ui: &mut egui::Ui| -> egui::Rect {
            if let Some(w) = width { ui.set_width(w); } else if fill { ui.set_width(ui.available_width()); }
            let ah = ui.available_height();
            let mut hpx = match s.height {
                Length::Px(h) => Some(h),
                Length::Percent(p) => Some(ah * p / 100.0),
                Length::Auto => None,
            };
            if let Some(mh) = pct(s.min_height, ah) { hpx = Some(hpx.map_or(mh, |h| h.max(mh))); }
            if let Some(h) = hpx { ui.set_min_height(h); }
            if let Some(mh) = s.max_height { ui.set_max_height(ah.min(mh)); }
            if s.display == Display::Flex {
                if s.flex_row {
                    this.flex_row_layout(ui, items, s)
                } else {
                    this.flex_col_layout(ui, items, s)
                }
            } else {
                ui.vertical(|ui| { ui.spacing_mut().item_spacing = Vec2::ZERO; this.items(ui, items, false); }).response.rect
            }
        };

        let mut run = |this: &mut Self, ui: &mut egui::Ui| -> egui::Rect {
            if chrome {
                egui::Frame::new()
                    .fill(s.background.unwrap_or(Color32::TRANSPARENT))
                    .stroke(Stroke::new(s.border_width, s.border_color))
                    .corner_radius(s.radius.clamp(0.0, 255.0) as u8)
                    .inner_margin(margin_of(s.padding))
                    .outer_margin(margin_of(s.margin))
                    .show(ui, |ui| body(this, ui)).response.rect
            } else {
                ui.scope(|ui| body(this, ui)).inner
            }
        };

        if s.margin_auto_h() && width.is_some() {
            ui.vertical_centered(|ui| run(self, ui)).inner
        } else {
            run(self, ui)
        }
    }

    /// Main-axis distribution for `justify-content`.
    fn justify_spacing(&self, j: JustifyContent, free: f32, gap: f32, n: usize) -> (f32, f32) {
        if n <= 1 { return (0.0, gap); }
        match j {
            JustifyContent::FlexStart => (0.0, gap),
            JustifyContent::FlexEnd => (free, gap),
            JustifyContent::Center => (free * 0.5, gap),
            JustifyContent::SpaceBetween => (0.0, gap + free / (n - 1) as f32),
            JustifyContent::SpaceAround => (free / (2.0 * n as f32), gap + free / n as f32),
            JustifyContent::SpaceEvenly => (free / (n + 1) as f32, gap + free / (n + 1) as f32),
        }
    }

    /// Custom flex-row layout: measure every item's width first (pure
    /// measurement — nothing is painted), then place items left to right using
    /// `justify-content` for main-axis distribution and `align-items` for the
    /// cross axis. Cross-axis center/end alignment is applied after painting by
    /// translating each item's shapes, so it stays exact regardless of the
    /// height estimates used for `stretch`.
    fn flex_row_layout(&mut self, ui: &mut egui::Ui, items: &mut [Item], s: &Style) -> egui::Rect {
        let start = ui.cursor().min;
        let avail_w = ui.available_width().max(0.0);
        let avail_h = ui.available_height().max(1.0);
        let right = ui.max_rect().right().max(start.x);
        let gap = s.gap;
        let wrap = s.flex_wrap;

        let widths: Vec<f32> = items.iter().map(|it| self.measure_item(ui, it)).collect();
        let n = widths.len();
        let order: Vec<usize> = if s.flex_reverse { (0..n).rev().collect() } else { (0..n).collect() };
        let widths: Vec<f32> = order.iter().map(|&i| widths[i]).collect();

        // Split into lines (only when flex-wrap: wrap).
        let mut lines: Vec<(usize, usize)> = vec![];
        let mut li = 0;
        for j in 0..n {
            if wrap && j > li && start.x + widths[j] > right {
                lines.push((li, j));
                li = j;
            }
        }
        lines.push((li, n));

        let mut y = start.y;
        let mut used = egui::Rect::NOTHING;
        for (a, b) in lines {
            let count = b - a;
            let total: f32 = widths[a..b].iter().sum::<f32>() + gap * count.saturating_sub(1) as f32;
            let free = (avail_w - total).max(0.0);
            let (lead, between) = self.justify_spacing(s.justify_content, free, gap, count);

            // Cross-axis stretch: allocate each item the (estimated) height of
            // the tallest item in the line.
            let stretch_h = if s.align_items == AlignItems::Stretch {
                (a..b).map(|i| self.measure_item_h(ui, &items[order[i]], widths[i])).fold(0.0, f32::max)
            } else { 0.0 };

            let mut x = start.x + lead;
            let layer = ui.layer_id();
            let mut items_drawn: Vec<(ShapeIdx, ShapeIdx, usize, usize, f32)> = vec![];
            for i in a..b {
                let mw = widths[i].max(0.0);
                let child_h = if stretch_h > 0.0 { stretch_h } else { avail_h };

                // Stretch: temporarily fix the item's cross size (only when it
                // has no explicit height).
                let saved_h = match &mut items[order[i]] {
                    Item::Box(b) if stretch_h > 0.0 && b.style.height == Length::Auto => {
                        b.style.height = Length::Px(stretch_h);
                        Some(Length::Auto)
                    }
                    _ => None,
                };

                let gstart = self.ctx.graphics(|g| g.get(layer).map_or(ShapeIdx(0), |l| l.next_idx()));
                let hstart = self.hits.len();

                let mut child = ui.new_child(egui::UiBuilder::new()
                    .max_rect(egui::Rect::from_min_size(egui::pos2(x, y), egui::Vec2::new(mw, child_h))));
                let r = match &mut items[order[i]] {
                    Item::Box(b) => self.draw_box(&mut child, b, true),
                    Item::Inline { segs, align } => { self.inline(&mut child, segs, *align); child.min_rect() }
                };
                let gend = self.ctx.graphics(|g| g.get(layer).map_or(ShapeIdx(0), |l| l.next_idx()));

                if let Some(_) = saved_h {
                    if let Item::Box(b) = &mut items[order[i]] { b.style.height = Length::Auto; }
                }

                items_drawn.push((gstart, gend, hstart, self.hits.len(), r.height()));
                x += mw + between;
            }

            // Cross-axis alignment: the line height is only known after every
            // item has been drawn, so translate each item's shapes and hit
            // rects now. This keeps center/end alignment exact.
            let line_h = if stretch_h > 0.0 {
                stretch_h
            } else {
                items_drawn.iter().map(|d| d.4).fold(0.0, f32::max)
            };
            for (gstart, gend, hstart, hend, h) in items_drawn {
                let dy = match s.align_items {
                    AlignItems::Center => (line_h - h).max(0.0) * 0.5,
                    AlignItems::FlexEnd => (line_h - h).max(0.0),
                    _ => 0.0,
                };
                if dy > 0.0 {
                    self.ctx.graphics_mut(|g| {
                        g.entry(layer).transform_range(gstart, gend, TSTransform::from_translation(Vec2::new(0.0, dy)));
                    });
                    for h in &mut self.hits[hstart..hend] { h.rect.min.y += dy; h.rect.max.y += dy; }
                }
            }

            let line_bottom = y + line_h;
            let line_right = start.x + lead + total + (between - gap) * count.saturating_sub(1) as f32;
            used = used.union(egui::Rect::from_min_max(egui::pos2(start.x, y), egui::pos2(line_right, line_bottom)));
            y = line_bottom + gap;
        }

        if used.is_positive() {
            ui.advance_cursor_after_rect(used);
        } else {
            ui.advance_cursor_after_rect(egui::Rect::from_min_size(start, egui::Vec2::ZERO));
        }
        used
    }

    /// Custom flex-column layout: items stack vertically (main axis) using
    /// `justify-content` for spacing and `align-items` for horizontal
    /// alignment. Heights are estimated before painting; horizontal
    /// center/end alignment is applied exactly by translating painted shapes.
    fn flex_col_layout(&mut self, ui: &mut egui::Ui, items: &mut [Item], s: &Style) -> egui::Rect {
        let start = ui.cursor().min;
        let content_w = ui.available_width().max(0.0);
        let avail_h = match s.height {
            Length::Px(v) => v.max(0.0),
            _ => ui.available_height().max(0.0),
        };
        let gap = s.gap;

        let widths: Vec<f32> = items.iter().map(|it| {
            if s.align_items == AlignItems::Stretch { content_w } else { self.measure_item(ui, it) }
        }).collect();
        let heights: Vec<f32> = items.iter().zip(&widths).map(|(it, &w)| self.measure_item_h(ui, it, w)).collect();
        let n = widths.len();
        let order: Vec<usize> = if s.flex_reverse { (0..n).rev().collect() } else { (0..n).collect() };

        let total_h: f32 = heights.iter().sum::<f32>() + gap * n.saturating_sub(1) as f32;
        let free = (avail_h - total_h).max(0.0);
        let (lead, between) = self.justify_spacing(s.justify_content, free, gap, n);

        let mut y = start.y + lead;
        let mut used = egui::Rect::NOTHING;
        for &i in &order {
            let mw = widths[i].max(0.0);
            let layer = ui.layer_id();
            let gstart = self.ctx.graphics(|g| g.get(layer).map_or(ShapeIdx(0), |l| l.next_idx()));
            let hstart = self.hits.len();

            let mut child = ui.new_child(egui::UiBuilder::new()
                .max_rect(egui::Rect::from_min_size(egui::pos2(start.x, y), egui::Vec2::new(mw, avail_h.max(1.0)))));
            let r = match &mut items[i] {
                Item::Box(b) => self.draw_box(&mut child, b, false),
                Item::Inline { segs, align } => { self.inline(&mut child, segs, *align); child.min_rect() }
            };
            let gend = self.ctx.graphics(|g| g.get(layer).map_or(ShapeIdx(0), |l| l.next_idx()));

            // align-items on the cross (horizontal) axis.
            let dx = match s.align_items {
                AlignItems::Center => (content_w - r.width()).max(0.0) * 0.5,
                AlignItems::FlexEnd => (content_w - r.width()).max(0.0),
                _ => 0.0,
            };
            if dx > 0.0 {
                self.ctx.graphics_mut(|g| {
                    g.entry(layer).transform_range(gstart, gend, TSTransform::from_translation(Vec2::new(dx, 0.0)));
                });
                for h in &mut self.hits[hstart..] { h.rect.min.x += dx; h.rect.max.x += dx; }
            }

            let h = heights[i].max(r.height());
            used = used.union(r.translate(Vec2::new(dx, 0.0)));
            y += h + between;
        }

        used = used.union(egui::Rect::from_min_max(start, egui::pos2(start.x + content_w, y - between)));
        if used.is_positive() {
            ui.advance_cursor_after_rect(used);
        } else {
            ui.advance_cursor_after_rect(egui::Rect::from_min_size(start, egui::Vec2::ZERO));
        }
        used
    }

    /// Intrinsic (shrink-wrap) width of a row item, without painting anything.
    fn measure_item(&mut self, ui: &egui::Ui, it: &Item) -> f32 {
        match it {
            Item::Box(b) => self.measure_box(ui, b),
            Item::Inline { segs, .. } => segs.iter().map(|s| match s {
                Seg::Text(job) => {
                    let mut j = job.clone();
                    j.wrap.max_width = f32::INFINITY;
                    self.ctx.fonts(|f| f.layout_job(j)).size().x
                }
                Seg::Widget(b) => self.measure_box(ui, b),
            }).fold(0.0, f32::max),
        }
    }

    /// Intrinsic width of a box: explicit `width` wins; otherwise compute from
    /// content (widget natural size / children), plus border, padding, margins.
    fn measure_box(&mut self, ui: &egui::Ui, b: &RBox) -> f32 {
        let s = b.style;
        let fixed = match s.width {
            Length::Px(w) => Some(w),
            Length::Percent(p) => Some(ui.available_width() * p / 100.0),
            Length::Auto => None,
        };
        let mut inner = fixed.unwrap_or_else(|| match &b.content {
            Content::Widget(w) => self.measure_widget(ui, b, w),
            Content::Hr => 0.0,
            Content::Items(items) => {
                if s.display == Display::Flex && s.flex_row {
                    let mut t = 0.0;
                    let mut first = true;
                    for it in items {
                        if !first { t += s.gap; }
                        first = false;
                        t += self.measure_item(ui, it);
                    }
                    t
                } else {
                    items.iter().map(|it| self.measure_item(ui, it)).fold(0.0, f32::max)
                }
            }
        });
        inner += s.padding[1] + s.padding[3] + 2.0 * s.border_width;
        if let Length::Px(mn) = s.min_width { inner = inner.max(mn); }
        if let Some(mx) = s.max_width { inner = inner.min(mx); }
        inner + s.margin[1] + s.margin[3]
    }

    fn measure_widget(&mut self, ui: &egui::Ui, b: &RBox, w: &Widget) -> f32 {
        let s = b.style;
        let mut base = match w {
            Widget::Button { text, .. } | Widget::Link { text, .. } => {
                let family = if s.mono { FontFamily::Monospace } else { FontFamily::Proportional };
                let g = ui.painter().layout_no_wrap(text.clone(), FontId::new(s.font_size, family), Color32::WHITE);
                g.size().x + 12.0
            }
            Widget::Text { multiline, .. } => if *multiline { 240.0 } else { 160.0 },
            Widget::Check { .. } => ui.spacing().interact_size.y,
            Widget::Image { src } => match get_image(self.shared, src) {
                Some(d) => d.w as f32,
                None => match s.width { Length::Px(v) => v, _ => 100.0 },
            },
            Widget::Canvas { w, .. } => *w as f32,
        };
        if let Length::Px(v) = s.width { base = v; }
        if let Length::Px(mn) = s.min_width { base = base.max(mn); }
        if let Some(mx) = s.max_width { base = base.min(mx); }
        base
    }

    /// Estimated (shrink-wrap) height of a row item when constrained to
    /// `width` — used for cross-axis stretch and column layout. Nothing is
    /// painted; the values approximate the real layout closely enough for
    /// spacing decisions.
    fn measure_item_h(&mut self, ui: &egui::Ui, it: &Item, width: f32) -> f32 {
        match it {
            Item::Box(b) => self.measure_box_h(ui, b, width),
            Item::Inline { segs, .. } => segs.iter().map(|s| match s {
                Seg::Text(job) => {
                    let mut j = job.clone();
                    j.wrap.max_width = width.max(1.0);
                    self.ctx.fonts(|f| f.layout_job(j)).size().y
                }
                Seg::Widget(b) => self.measure_box_h(ui, b, width),
            }).fold(0.0, f32::max),
        }
    }

    fn measure_box_h(&mut self, ui: &egui::Ui, b: &RBox, width: f32) -> f32 {
        let s = b.style;
        let fixed = match s.height { Length::Px(v) => Some(v), _ => None };
        let inner = fixed.unwrap_or_else(|| match &b.content {
            Content::Widget(w) => self.measure_widget_h(ui, b, w),
            Content::Hr => 8.0,
            Content::Items(items) => {
                let cw = (width - s.padding[1] - s.padding[3] - 2.0 * s.border_width
                    - s.margin[1] - s.margin[3]).max(0.0);
                if s.display == Display::Flex && s.flex_row {
                    items.iter().map(|it| self.measure_item_h(ui, it, cw)).fold(0.0, f32::max)
                } else {
                    let mut t = 0.0;
                    let mut first = true;
                    for it in items {
                        if !first { t += s.gap; }
                        first = false;
                        t += self.measure_item_h(ui, it, cw);
                    }
                    t
                }
            }
        });
        let inner = inner + s.padding[0] + s.padding[2] + 2.0 * s.border_width;
        let inner = if let Length::Px(mn) = s.min_height { inner.max(mn) } else { inner };
        let inner = if let Some(mx) = s.max_height { inner.min(mx) } else { inner };
        inner + s.margin[0] + s.margin[2]
    }

    fn measure_widget_h(&mut self, ui: &egui::Ui, b: &RBox, w: &Widget) -> f32 {
        let s = b.style;
        let mut base = match w {
            Widget::Button { text, .. } | Widget::Link { text, .. } => {
                let family = if s.mono { FontFamily::Monospace } else { FontFamily::Proportional };
                let g = ui.painter().layout_no_wrap(text.clone(), FontId::new(s.font_size, family), Color32::WHITE);
                g.size().y + 8.0
            }
            Widget::Text { multiline, .. } => s.font_size * if *multiline { 3.0 } else { 2.0 },
            Widget::Check { .. } => ui.spacing().interact_size.y,
            Widget::Image { src } => match get_image(self.shared, src) {
                Some(d) => d.h as f32,
                None => match s.height { Length::Px(v) => v, _ => 100.0 },
            },
            Widget::Canvas { h, .. } => *h as f32,
        };
        if let Length::Px(v) = s.height { base = v; }
        if let Length::Px(mn) = s.min_height { base = base.max(mn); }
        if let Some(mx) = s.max_height { base = base.min(mx); }
        base
    }

    /// Draw an out-of-flow (`absolute`/`fixed`) box against `area`. Drawn
    /// anchored at its top/left offsets, then shifted so any right/bottom
    /// anchor lines up (shrink-wrapped, like CSS). With `own_layer` the box
    /// paints into a dedicated layer above everything (used for `fixed`).
    fn draw_out_of_flow(&mut self, ui: &mut egui::Ui, b: &mut RBox, area: egui::Rect, own_layer: bool) {
        let st = b.style;
        let off = |k: usize, tot: f32| pct(st.offset[k], tot);
        let (left, right) = (off(3, area.width()), off(1, area.width()));
        let (top, bottom) = (off(0, area.height()), off(2, area.height()));

        // Both sides set => the box is stretched to the band (CSS behavior).
        // The overrides are transient: the tree is never mutated during paint.
        let (saved_w, saved_h) = (b.style.width, b.style.height);
        if let (Some(l), Some(r)) = (left, right) { b.style.width = Length::Px((r - l).max(1.0)); }
        if let (Some(t), Some(bo)) = (top, bottom) { b.style.height = Length::Px((bo - t).max(1.0)); }

        let min = egui::pos2(
            area.left() + left.unwrap_or(0.0),
            area.top() + top.unwrap_or(0.0),
        );
        let extent = Vec2::new(
            (area.right() - min.x).max(1.0),
            (area.bottom() - min.y).max(1.0),
        );
        let r = egui::Rect::from_min_size(min, extent);
        let mut child = if own_layer {
            let lid = egui::LayerId::new(Order::Middle, egui::Id::new(("rice-fixed", b.id)));
            ui.new_child(egui::UiBuilder::new().layer_id(lid).max_rect(r))
        } else {
            ui.new_child(egui::UiBuilder::new().max_rect(r))
        };
        let hstart = self.hits.len();
        // Shapes go into the child's own layer when we gave it one, else
        // into the layer of the ui we paint from.
        let layer = child.layer_id();
        let s_start = self.ctx.graphics(|g| g.get(layer).map_or(ShapeIdx(0), |l| l.next_idx()));
        let saved_nf = self.no_fill;
        self.no_fill = true;
        let drawn = self.draw_box(&mut child, b, false);
        self.no_fill = saved_nf;
        let s_end = self.ctx.graphics(|g| g.get(layer).map_or(ShapeIdx(0), |l| l.next_idx()));

        b.style.width = saved_w;
        b.style.height = saved_h;

        // Shift the painted subtree so right/bottom anchors line up.
        let mut delta = Vec2::ZERO;
        if let Some(r) = right { delta.x = area.right() - r - drawn.max.x; }
        if let Some(bo) = bottom { delta.y = area.bottom() - bo - drawn.max.y; }
        if delta != Vec2::ZERO {
            self.ctx.graphics_mut(|g| g.entry(layer).transform_range(s_start, s_end, TSTransform::from_translation(delta)));
            for h in &mut self.hits[hstart..] { h.rect.min += delta; h.rect.max += delta; }
        }
    }

    /// Paint all out-of-flow boxes (`absolute`/`fixed`) — called after the main
    /// pass so they can overlap everything, and so repaints without a rebuild
    /// always draw them (they live in the tree, not in the paint pass).
    pub fn draw_oof(&mut self, ui: &mut egui::Ui, oof: &mut [Oof]) {
        let screen = self.ctx.screen_rect();
        for o in oof {
            let area = if o.fixed {
                screen
            } else {
                o.cb.and_then(|id| self.node_rects.get(&id).copied()).unwrap_or(screen)
            };
            self.draw_out_of_flow(ui, &mut o.rbox, area, o.fixed);
        }
    }

    fn inline(&mut self, ui: &mut egui::Ui, segs: &mut [Seg], align: Align) {
        if let [Seg::Text(job)] = segs {
            let mut job = job.clone();
            job.halign = align;
            job.wrap.max_width = ui.available_width();
            if align == Align::Min { ui.label(job); }
            else { ui.with_layout(Layout::top_down(align), |ui| ui.label(job)); }
            return;
        }
        ui.horizontal_wrapped(|ui| {
            for s in segs {
                match s {
                    Seg::Text(job) => { ui.label(job.clone()); }
                    Seg::Widget(b) => { self.draw_box(ui, b, true); }
                }
            }
        });
    }

    fn widget(&mut self, ui: &mut egui::Ui, id: NodeId, s: &Style, w: &mut Widget) -> egui::Rect {
        if s.hidden {
            // visibility: hidden — reserve space, paint nothing
            let natural = match w {
                Widget::Canvas { w, h } => Vec2::new(*w as f32, *h as f32),
                Widget::Image { .. } => Vec2::new(match s.width { Length::Px(v) => v, _ => 0.0 }, match s.height { Length::Px(v) => v, _ => 0.0 }),
                Widget::Button { text, .. } | Widget::Link { text, .. } => {
                    let galley = ui.painter().layout_no_wrap(text.clone(), FontId::proportional(s.font_size), Color32::WHITE);
                    galley.size() + Vec2::new(0.0, 0.0)
                }
                Widget::Text { multiline, .. } => Vec2::new(if *multiline { 240.0 } else { 160.0 }, s.font_size * if *multiline { 3.0 } else { 2.0 }),
                Widget::Check { .. } => Vec2::splat(ui.spacing().interact_size.y),
            };
            return ui.allocate_exact_size(size_for(s, ui, natural), egui::Sense::hover()).0;
        }

        let mut inner = |this: &mut Self, ui: &mut egui::Ui| -> egui::Rect {
            match w {
                Widget::Button { text, disabled } => {
                    let mut btn = egui::Button::new(egui::RichText::new(text.as_str()).size(s.font_size).color(s.color));
                    if let Some(bg) = s.background { btn = btn.fill(bg); }
                    if s.border_width > 0.0 { btn = btn.stroke(Stroke::new(s.border_width, s.border_color)); }
                    if s.radius > 0.0 { btn = btn.corner_radius(s.radius.min(255.0) as u8); }
                    let aw = ui.available_width();
                    let mut min = Vec2::new(
                        match s.width { Length::Px(v) => v, Length::Percent(p) => aw * p / 100.0, _ => 0.0 },
                        match s.height { Length::Px(v) => v, Length::Percent(p) => ui.available_height() * p / 100.0, _ => 0.0 },
                    );
                    if let Some(mw) = pct(s.min_width, aw) { min.x = min.x.max(mw); }
                    if let Some(mh) = pct(s.min_height, ui.available_height()) { min.y = min.y.max(mh); }
                    if let Some(m) = s.max_width { min.x = min.x.min(m); }
                    if let Some(m) = s.max_height { min.y = min.y.min(m); }
                    ui.add_enabled(!*disabled, btn.min_size(min)).rect
                }
                Widget::Link { text, href } => ui.scope(|ui| {
                    ui.visuals_mut().hyperlink_color = s.color;
                    let rt = egui::RichText::new(text.as_str()).size(s.font_size);
                    if href.is_empty() || href.starts_with('#') { ui.link(rt).rect } else { ui.hyperlink_to(rt, href).rect }
                }).inner,
                Widget::Text { value, hint, password, multiline, disabled } => {
                    let buf = this.state.inputs.entry(id).or_insert_with(|| value.clone());
                    let aw = ui.available_width();
                    let wd = match s.width {
                        Length::Px(v) => v,
                        Length::Percent(p) => (aw * p / 100.0).max(0.0),
                        Length::Auto => if *multiline { 240.0 } else { 160.0 },
                    };
                    let wd = pct(s.min_width, aw).map_or(wd, |mw| wd.max(mw));
                    let bg = s.background.unwrap_or(ui.visuals().extreme_bg_color);
                    let family = if s.mono { FontFamily::Monospace } else { FontFamily::Proportional };
                    ui.scope(|ui| {
                        // Style the field chrome from CSS: background, border, radius.
                        let v = ui.visuals_mut();
                        let stroke = if s.border_width > 0.0 { Stroke::new(s.border_width, s.border_color) } else { v.widgets.inactive.bg_stroke };
                        let radius = (s.radius.clamp(0.0, 255.0) as u8).into();
                        for wb in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
                            wb.bg_fill = bg;
                            wb.bg_stroke = stroke;
                            wb.corner_radius = radius;
                            wb.expansion = 0.0;
                        }
                        if s.border_width > 0.0 { v.selection.stroke = Stroke::new(s.border_width, s.border_color); }
                        let te = if *multiline { egui::TextEdit::multiline(buf) } else { egui::TextEdit::singleline(buf) }
                            .id(egui::Id::new(("rice-input", id)))
                            .hint_text(hint.as_str())
                            .password(*password)
                            .desired_width(wd)
                            .text_color(s.color)
                            .background_color(bg)
                            .font(FontId::new(s.font_size, family));
                        let r = ui.add_enabled(!*disabled, te);
                        if r.changed() { this.out.push(Msg::Input { target: id, value: buf.clone() }); }
                        r.rect
                    }).inner
                }
                Widget::Check { checked, disabled } => {
                    let c = this.state.checks.entry(id).or_insert(*checked);
                    let r = ui.add_enabled(!*disabled, egui::Checkbox::new(c, ""));
                    if r.changed() { this.out.push(Msg::Change { target: id, checked: *c }); }
                    r.rect
                }
                Widget::Image { src } => match get_image(this.shared, src) {
                    Some(d) => {
                        let natural = Vec2::new(d.w as f32, d.h as f32);
                        let size = size_for(s, ui, natural);
                        let handle = this.state.tex.entry(format!("img:{src}")).or_insert_with(|| {
                            let img = egui::ColorImage::from_rgba_unmultiplied([d.w as usize, d.h as usize], &d.rgba);
                            this.ctx.load_texture(format!("rice-img:{src}"), img, Default::default())
                        });
                        ui.add(egui::Image::new(egui::load::SizedTexture::new(handle.id(), size))).rect
                    }
                    None => {
                        // still loading (or failed): reserve the styled size
                        let natural = Vec2::new(
                            match s.width { Length::Px(v) => v, _ => 0.0 },
                            match s.height { Length::Px(v) => v, _ => 0.0 },
                        );
                        let size = size_for(s, ui, natural);
                        ui.allocate_exact_size(size, egui::Sense::hover()).0
                    }
                },
                Widget::Canvas { w: cw, h: ch } => {
                    let natural = Vec2::new(*cw as f32, *ch as f32);
                    let size = size_for(s, ui, natural);
                    // Only clone the pixel buffer when the version changed
                    // since our last upload; the text overlay re-draws each frame.
                    let known = this.state.canvas_ver.get(&id).copied();
                    let have_tex = this.state.tex.contains_key(&format!("canvas:{id}"));
                    let snapshot = {
                        let cs = this.shared.canvases.lock().unwrap_or_else(|e| e.into_inner());
                        cs.get(&id).map(|c| (
                            c.version, c.w, c.h,
                            if known == Some(c.version) && have_tex { Vec::new() } else { c.pixels.clone() },
                            c.texts.clone(),
                        ))
                    };
                    let Some((version, bw, bh, pixels, texts)) = snapshot else {
                        return ui.allocate_exact_size(size, egui::Sense::hover()).0;
                    };
                    let rebuild = this.state.canvas_ver.get(&id).map_or(true, |v| *v != version);
                    // Reuse the texture allocated on the first frame: bumping
                    // pixels via `set` keeps the same TextureId. Calling
                    // `load_texture` again would allocate a brand-new texture
                    // every redraw (leaking TextureIds and causing flicker).
                    let mut created = false;
                    let handle = this.state.tex.entry(format!("canvas:{id}")).or_insert_with(|| {
                        created = true;
                        let img = egui::ColorImage::from_rgba_unmultiplied([bw as usize, bh as usize], &pixels);
                        this.ctx.load_texture(format!("rice-canvas:{id}"), img, Default::default())
                    });
                    if rebuild && !created {
                        handle.set(egui::ColorImage::from_rgba_unmultiplied([bw as usize, bh as usize], &pixels), Default::default());
                        this.state.canvas_ver.insert(id, version);
                    }
                    let rect = ui.add(egui::Image::new(egui::load::SizedTexture::new(handle.id(), size))).rect;
                    // fill_text draws render as a crisp font overlay
                    for (t, x, y, fs, col) in &texts {
                        ui.painter().text(
                            rect.min + Vec2::new(*x, *y),
                            Align2::LEFT_TOP, t.clone(),
                            FontId::proportional(*fs),
                            Color32::from_rgba_unmultiplied(col[0], col[1], col[2], col[3]),
                        );
                    }
                    rect
                }
            }
        };
        if s.margin.iter().any(|&m| m > 0.0) {
            egui::Frame::new().outer_margin(margin_of(s.margin)).show(ui, |ui| inner(self, ui)).inner
        } else {
            inner(self, ui)
        }
    }
}
