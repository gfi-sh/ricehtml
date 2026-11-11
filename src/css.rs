use crate::dom::{Dom, NodeId};
use crate::selector::{self, Complex, Key};
use eframe::egui::{Align, Color32};
use std::{collections::HashMap, sync::OnceLock};

#[allow(unused_imports)]
use crate::dom::ROOT;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Length { Auto, Px(f32), Percent(f32) }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Display { None, Inline, Block, Flex }

/// `position`: Static/Relative flow normally; Absolute/Fixed are laid out
/// relative to their containing block and removed from the normal flow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Position { Static, Relative, Absolute, Fixed }

/// Parsed `transform` — translate offset plus a uniform scale factor
/// (egui's paint transforms are translation + uniform scaling only).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Xform { pub tx: f32, pub ty: f32, pub s: f32 }

impl Default for Xform {
    fn default() -> Self { Xform { tx: 0.0, ty: 0.0, s: 1.0 } }
}

impl Xform {
    pub fn is_identity(&self) -> bool { self.tx == 0.0 && self.ty == 0.0 && self.s == 1.0 }
}

#[derive(Clone, Copy, Debug)]
pub struct Style {
    pub display: Display,
    pub position: Position,
    /// top right bottom left offsets for positioned elements
    pub offset: [Length; 4],
    /// `visibility: hidden` — keeps its space but renders nothing (inherits).
    pub hidden: bool,
    pub transform: Xform,
    pub opacity: f32,
    // inherited
    pub color: Color32,
    pub font_size: f32,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub mono: bool,
    pub pre: bool,
    pub text_align: Align,
    // box
    pub background: Option<Color32>,
    pub margin: [f32; 4],  // top right bottom left
    pub padding: [f32; 4],
    pub auto_lr: [bool; 2],
    pub width: Length,
    pub height: Length,
    pub min_width: Length,
    pub min_height: Length,
    pub max_width: Option<f32>,
    pub max_height: Option<f32>,
    pub border_width: f32,
    pub border_color: Color32,
    pub radius: f32,
    // flex
    pub flex_row: bool,
    pub flex_wrap: bool,
    pub gap: f32,
}

impl Style {
    pub fn root() -> Style {
        Style {
            display: Display::Block, position: Position::Static, offset: [Length::Auto; 4],
            hidden: false, transform: Xform::default(), opacity: 1.0,
            color: Color32::from_rgb(0x20, 0x20, 0x20), font_size: 16.0,
            bold: false, italic: false, underline: false, strike: false, mono: false, pre: false,
            text_align: Align::Min, background: None, margin: [0.0; 4], padding: [0.0; 4],
            auto_lr: [false; 2], width: Length::Auto, height: Length::Auto,
            min_width: Length::Auto, min_height: Length::Auto, max_width: None, max_height: None,
            border_width: 0.0, border_color: Color32::GRAY, radius: 0.0,
            flex_row: true, flex_wrap: false, gap: 0.0,
        }
    }
    pub fn inherit(p: &Style) -> Style {
        Style {
            display: Display::Inline, color: p.color, font_size: p.font_size, bold: p.bold, italic: p.italic,
            underline: p.underline, strike: p.strike, mono: p.mono, pre: p.pre, text_align: p.text_align,
            hidden: p.hidden, // visibility inherits
            ..Style::root()
        }
    }
    pub fn margin_auto_h(&self) -> bool { self.auto_lr[0] && self.auto_lr[1] }
    pub fn positioned(&self) -> bool { self.position != Position::Static }
}

// ---------------------------------------------------------------- parsing

#[derive(Clone)]
pub struct Decl { pub prop: String, pub value: String, pub important: bool }

#[derive(Clone)]
pub struct Rule { sel: Complex, spec: u32, order: usize, origin: u8, decls: Vec<Decl> }

pub fn parse_decls(body: &str) -> Vec<Decl> {
    body.split(';').filter_map(|d| {
        let (k, v) = d.split_once(':')?;
        let v = v.trim();
        let (v, important) = match v.strip_suffix("!important") { Some(x) => (x.trim(), true), None => (v, false) };
        if k.trim().is_empty() || v.is_empty() { return None; }
        Some(Decl { prop: k.trim().to_ascii_lowercase(), value: v.to_string(), important })
    }).collect()
}

fn strip_comments(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(p) = rest.find("/*") {
        out.push_str(&rest[..p]);
        match rest[p + 2..].find("*/") { Some(e) => rest = &rest[p + 2 + e + 2..], None => { rest = ""; } }
    }
    out.push_str(rest);
    out
}

fn parse_sheet(css: &str, origin: u8, order: &mut usize) -> Vec<Rule> {
    let css = strip_comments(css);
    let b = css.as_bytes();
    let (mut i, mut rules) = (0, vec![]);
    while let Some(open) = css[i..].find('{') {
        let open = i + open;
        let prelude = css[i..open].trim();
        let (mut depth, mut j) = (1, open + 1);
        while j < b.len() && depth > 0 {
            match b[j] { b'{' => depth += 1, b'}' => depth -= 1, _ => {} }
            j += 1;
        }
        let body = &css[open + 1..j.saturating_sub(1).max(open + 1)];
        i = j;
        if prelude.starts_with('@') { continue; } // @media etc. unsupported
        let decls = parse_decls(body);
        for sel in selector::parse_list(prelude) {
            *order += 1;
            rules.push(Rule { spec: sel.specificity(), sel, order: *order, origin, decls: decls.clone() });
        }
    }
    rules
}

const UA_CSS: &str = r#"
html,body,div,section,article,main,header,footer,nav,aside,figure,figcaption,form,fieldset,address,
blockquote,p,ul,ol,li,dl,dt,dd,h1,h2,h3,h4,h5,h6,pre,table,tbody,thead,tfoot,tr,td,th,legend,
details,summary,menu,dir,center,hr { display:block }
head,script,style,title,meta,link,base,template,noscript { display:none }
body { margin:8px }
h1 { font-size:2em; font-weight:bold; margin:.67em 0 }
h2 { font-size:1.5em; font-weight:bold; margin:.83em 0 }
h3 { font-size:1.17em; font-weight:bold; margin:1em 0 }
h4 { font-weight:bold; margin:1.33em 0 }
h5 { font-size:.83em; font-weight:bold; margin:1.67em 0 }
h6 { font-size:.67em; font-weight:bold; margin:2.33em 0 }
p { margin:1em 0 }
blockquote { margin:1em 40px }
ul,ol,menu,dir { margin:1em 0; padding-left:40px }
ul ul, ol ol, ul ol, ol ul { margin:0 }
dd { margin-left:40px }
b,strong,th { font-weight:bold }
i,em,cite,dfn,var,address { font-style:italic }
u,ins { text-decoration:underline }
s,strike,del { text-decoration:line-through }
small { font-size:.83em } big { font-size:1.17em } sub,sup { font-size:.75em }
code,tt,kbd,samp { font-family:monospace }
pre,xmp,listing { font-family:monospace; white-space:pre; margin:1em 0; padding:8px; background:#f4f4f4 }
a { color:#0645ad; text-decoration:underline }
center { text-align:center }
hr { margin:.5em 0 }
tr { display:flex; flex-direction:row; gap:12px }
td,th { padding:2px 4px }
"#;

fn ua_rules() -> &'static Vec<Rule> {
    static UA: OnceLock<Vec<Rule>> = OnceLock::new();
    UA.get_or_init(|| { let mut o = 0; parse_sheet(UA_CSS, 0, &mut o) })
}

// ---------------------------------------------------------------- cascade

/// Rules bucketed by the rightmost selector key, so each element only tests
/// rules that could possibly match it.
pub struct Sheet {
    rules: Vec<Rule>,
    by_id: HashMap<String, Vec<usize>>,
    by_class: HashMap<String, Vec<usize>>,
    by_tag: HashMap<String, Vec<usize>>,
    any: Vec<usize>,
}

impl Sheet {
    pub fn new(authors: &[String]) -> Sheet {
        let mut rules = ua_rules().clone();
        let mut order = rules.len();
        for css in authors { rules.extend(parse_sheet(css, 1, &mut order)); }
        let (mut by_id, mut by_class, mut by_tag, mut any) = (HashMap::new(), HashMap::new(), HashMap::new(), vec![]);
        for (i, r) in rules.iter().enumerate() {
            match r.sel.key() {
                Key::Id(k) => by_id.entry(k).or_insert_with(Vec::new).push(i),
                Key::Class(k) => by_class.entry(k).or_insert_with(Vec::new).push(i),
                Key::Tag(k) => by_tag.entry(k).or_insert_with(Vec::new).push(i),
                Key::Any => any.push(i),
            }
        }
        Sheet { rules, by_id, by_class, by_tag, any }
    }

    pub fn compute(&self, dom: &Dom, id: NodeId, parent: &Style, hovered: &dyn Fn(NodeId) -> bool) -> Style {
        let mut cand: Vec<usize> = self.any.clone();
        if let Some(v) = dom.attr(id, "id").and_then(|i| self.by_id.get(i)) { cand.extend_from_slice(v); }
        if let Some(c) = dom.attr(id, "class") {
            for cl in c.split_whitespace() {
                if let Some(v) = self.by_class.get(cl) { cand.extend_from_slice(v); }
            }
        }
        if let Some(v) = dom.tag(id).and_then(|t| self.by_tag.get(t)) { cand.extend_from_slice(v); }
        cand.sort_unstable();
        cand.dedup();

        let inline = dom.attr(id, "style").map(parse_decls).unwrap_or_default();
        let mut app: Vec<(bool, u8, u32, usize, &Decl)> = vec![];
        for ri in cand {
            let r = &self.rules[ri];
            if r.sel.matches_dyn(dom, id, hovered) {
                for d in &r.decls { app.push((d.important, r.origin, r.spec, r.order, d)); }
            }
        }
        for d in &inline { app.push((d.important, 2, 0, 0, d)); }
        app.sort_by_key(|a| (a.0, a.1, a.2, a.3));

        let mut st = Style::inherit(parent);
        // font-size first so em units in other properties see the final size
        for pass in 0..2 {
            for a in &app {
                if (pass == 0) == (a.4.prop == "font-size") { apply(&mut st, &a.4.prop, &a.4.value, parent.font_size); }
            }
        }
        st
    }
}

// ---------------------------------------------------------------- values

fn tokens(v: &str) -> Vec<&str> {
    let (mut out, mut depth, mut start) = (vec![], 0i32, None);
    for (i, c) in v.char_indices() {
        match c {
            '(' => { depth += 1; start.get_or_insert(i); }
            ')' => depth -= 1,
            c if c.is_whitespace() && depth <= 0 => { if let Some(s) = start.take() { out.push(&v[s..i]); } }
            _ => { start.get_or_insert(i); }
        }
    }
    if let Some(s) = start { out.push(&v[s..]); }
    out
}

fn parse_length(tok: &str, fs: f32) -> Option<Length> {
    let t = tok.trim().to_ascii_lowercase();
    if t == "auto" { return Some(Length::Auto); }
    let split = t.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+')).unwrap_or(t.len());
    let (n, u) = t.split_at(split);
    let n: f32 = n.parse().ok()?;
    Some(match u {
        "" | "px" => Length::Px(n),
        "em" => Length::Px(n * fs),
        "rem" => Length::Px(n * 16.0),
        "pt" => Length::Px(n * 4.0 / 3.0),
        "%" => Length::Percent(n),
        _ => return None,
    })
}

fn px(tok: &str, fs: f32) -> Option<f32> {
    match parse_length(tok, fs)? { Length::Px(p) => Some(p), _ => None }
}

pub fn parse_color(s: &str) -> Option<Color32> {
    let s = s.trim().to_ascii_lowercase();
    if let Some(h) = s.strip_prefix('#') {
        let d: Vec<u8> = h.chars().map(|c| c.to_digit(16).map(|x| x as u8)).collect::<Option<_>>()?;
        return match d.len() {
            3 => Some(Color32::from_rgb(d[0] * 17, d[1] * 17, d[2] * 17)),
            4 => Some(Color32::from_rgba_unmultiplied(d[0] * 17, d[1] * 17, d[2] * 17, d[3] * 17)),
            6 => Some(Color32::from_rgb(d[0] * 16 + d[1], d[2] * 16 + d[3], d[4] * 16 + d[5])),
            8 => Some(Color32::from_rgba_unmultiplied(d[0] * 16 + d[1], d[2] * 16 + d[3], d[4] * 16 + d[5], d[6] * 16 + d[7])),
            _ => None,
        };
    }
    if s.starts_with("rgb") {
        let inner = &s[s.find('(')? + 1..s.rfind(')')?];
        let v: Vec<f32> = inner.split(|c: char| c == ',' || c == '/' || c.is_whitespace()).filter(|x| !x.is_empty())
            .map(|x| if let Some(p) = x.strip_suffix('%') { p.parse::<f32>().ok().map(|p| p * 2.55) } else { x.parse().ok() })
            .collect::<Option<_>>()?;
        if v.len() < 3 { return None; }
        let a = v.get(3).map(|a| if *a <= 1.0 { a * 255.0 } else { *a }).unwrap_or(255.0);
        return Some(Color32::from_rgba_unmultiplied(v[0] as u8, v[1] as u8, v[2] as u8, a as u8));
    }
    let (r, g, b) = match s.as_str() {
        "transparent" => return Some(Color32::TRANSPARENT),
        "black" => (0, 0, 0), "white" => (255, 255, 255), "red" => (255, 0, 0), "green" => (0, 128, 0),
        "blue" => (0, 0, 255), "yellow" => (255, 255, 0), "orange" => (255, 165, 0), "purple" => (128, 0, 128),
        "gray" | "grey" => (128, 128, 128), "silver" | "lightgray" | "lightgrey" => (211, 211, 211),
        "darkgray" | "darkgrey" => (169, 169, 169), "pink" => (255, 192, 203), "brown" => (165, 42, 42),
        "cyan" | "aqua" => (0, 255, 255), "magenta" | "fuchsia" => (255, 0, 255), "navy" => (0, 0, 128),
        "teal" => (0, 128, 128), "maroon" => (128, 0, 0), "lime" => (0, 255, 0), "olive" => (128, 128, 0),
        "gold" => (255, 215, 0), "crimson" => (220, 20, 60), "tomato" => (255, 99, 71), "coral" => (255, 127, 80),
        "indigo" => (75, 0, 130), "violet" => (238, 130, 238), "whitesmoke" => (245, 245, 245),
        _ => return None,
    };
    Some(Color32::from_rgb(r, g, b))
}

fn box4(v: &str, fs: f32) -> Option<([f32; 4], [bool; 4])> {
    let vals: Vec<Length> = tokens(v).into_iter().map(|t| parse_length(t, fs)).collect::<Option<_>>()?;
    let idx: [usize; 4] = match vals.len() { 1 => [0, 0, 0, 0], 2 => [0, 1, 0, 1], 3 => [0, 1, 2, 1], 4 => [0, 1, 2, 3], _ => return None };
    let (mut p, mut a) = ([0.0; 4], [false; 4]);
    for k in 0..4 {
        match vals[idx[k]] { Length::Px(x) => p[k] = x, Length::Auto => a[k] = true, Length::Percent(_) => {} }
    }
    Some((p, a))
}

fn border_width(t: &str, fs: f32) -> Option<f32> {
    match t { "thin" => Some(1.0), "medium" => Some(2.0), "thick" => Some(4.0), _ => px(t, fs) }
}

/// Parse `transform: translate(x, y) scale(s)` etc. Composes the listed
/// functions into one (translation + uniform scaling). Unsupported functions
/// (`rotate`, `skew`, 3D) are ignored, `none` yields identity.
fn parse_transform(v: &str, fs: f32) -> Option<Xform> {
    let mut x = Xform::default();
    let mut any = false;
    for piece in v.split(')') {
        let Some((name, arg)) = piece.split_once('(') else { continue };
        let name = name.trim().to_ascii_lowercase();
        let args: Vec<&str> = arg.split(',').map(str::trim).collect();
        let n = |i: usize| args.get(i).and_then(|t| px(t, fs)).unwrap_or(0.0);
        let sc = |i: usize| args.get(i).and_then(|t| t.trim().parse::<f32>().ok()).unwrap_or(1.0);
        match name.as_str() {
            "none" => {}
            "translate" => { x.tx += n(0); x.ty += if args.len() > 1 { n(1) } else { 0.0 }; any = true; }
            "translatex" => { x.tx += n(0); any = true; }
            "translatey" => { x.ty += n(0); any = true; }
            "scale" => { x.s *= sc(0); any = true; } // non-uniform (sx, sy) degrades to sx
            "scalex" | "scaley" => { x.s *= sc(0); any = true; }
            _ => {} // rotate / skew / 3d unsupported (no rotation in egui paint transforms)
        }
    }
    if any { Some(x) } else { None }
}

fn apply(s: &mut Style, prop: &str, v: &str, parent_fs: f32) {
    let fs = s.font_size;
    let lv = v.to_ascii_lowercase();
    match prop {
        "display" => {
            s.display = match lv.split_whitespace().next().unwrap_or("") {
                "none" => Display::None,
                "flex" | "inline-flex" => Display::Flex,
                "inline" | "inline-block" | "contents" => Display::Inline,
                _ => Display::Block,
            }
        }
        "color" => if let Some(c) = parse_color(v) { s.color = c },
        "background-color" | "background" => {
            if lv == "none" { s.background = None; }
            for t in tokens(v) {
                // skip whole functions like linear-gradient(...) — their inner
                // color stops are not the element's background
                if t.contains('(') { continue; }
                if let Some(c) = parse_color(t) { s.background = if c.a() == 0 { None } else { Some(c) }; }
            }
        }
        "font-size" => {
            let px = match lv.trim() {
                "smaller" => parent_fs / 1.2, "larger" => parent_fs * 1.2,
                "xx-small" => 9.0, "x-small" => 10.0, "small" => 13.0, "medium" => 16.0,
                "large" => 18.0, "x-large" => 24.0, "xx-large" => 32.0,
                t => match parse_length(t, parent_fs) {
                    Some(Length::Px(p)) => p,
                    Some(Length::Percent(p)) => parent_fs * p / 100.0,
                    _ => return,
                },
            };
            s.font_size = px.max(1.0);
        }
        "font-weight" => s.bold = match lv.as_str() { "bold" | "bolder" => true, "normal" | "lighter" => false, n => n.parse::<u32>().map_or(s.bold, |w| w >= 600) },
        "font-style" => s.italic = lv == "italic" || lv == "oblique",
        "text-decoration" | "text-decoration-line" => {
            if lv.contains("none") { s.underline = false; s.strike = false; }
            if lv.contains("underline") { s.underline = true; }
            if lv.contains("line-through") { s.strike = true; }
        }
        "text-align" => s.text_align = match lv.as_str() { "center" => Align::Center, "right" | "end" => Align::Max, _ => Align::Min },
        "font-family" => s.mono = ["monospace", "courier", "consolas", "menlo"].iter().any(|m| lv.contains(m)),
        "white-space" => s.pre = lv.starts_with("pre"),
        "margin" => if let Some((p, a)) = box4(v, fs) { s.margin = p; s.auto_lr = [a[3], a[1]]; },
        "padding" => if let Some((p, _)) = box4(v, fs) { s.padding = p; },
        "margin-top" | "margin-right" | "margin-bottom" | "margin-left" => {
            let k = ["top", "right", "bottom", "left"].iter().position(|x| prop.ends_with(x)).unwrap();
            match parse_length(v, fs) {
                Some(Length::Px(p)) => { s.margin[k] = p; if k == 3 { s.auto_lr[0] = false } if k == 1 { s.auto_lr[1] = false } }
                Some(Length::Auto) => { s.margin[k] = 0.0; if k == 3 { s.auto_lr[0] = true } if k == 1 { s.auto_lr[1] = true } }
                _ => {}
            }
        }
        "padding-top" | "padding-right" | "padding-bottom" | "padding-left" => {
            let k = ["top", "right", "bottom", "left"].iter().position(|x| prop.ends_with(x)).unwrap();
            if let Some(p) = px(v, fs) { s.padding[k] = p; }
        }
        "width" => if let Some(l) = parse_length(v, fs) { s.width = l },
        "height" => if let Some(l) = parse_length(v, fs) { s.height = l },
        "min-width" => if let Some(l) = parse_length(v, fs) { s.min_width = l },
        "min-height" => if let Some(l) = parse_length(v, fs) { s.min_height = l },
        "max-width" => if lv != "none" { s.max_width = px(v, fs) },
        "max-height" => if lv != "none" { s.max_height = px(v, fs) },
        "position" => s.position = match lv.as_str() {
            "relative" => Position::Relative,
            "absolute" => Position::Absolute,
            "fixed" => Position::Fixed,
            _ => Position::Static,
        },
        "top" => s.offset[0] = parse_length(v, fs).unwrap_or(Length::Auto),
        "right" => s.offset[1] = parse_length(v, fs).unwrap_or(Length::Auto),
        "bottom" => s.offset[2] = parse_length(v, fs).unwrap_or(Length::Auto),
        "left" => s.offset[3] = parse_length(v, fs).unwrap_or(Length::Auto),
        "visibility" => s.hidden = lv == "hidden" || lv == "collapse",
        "opacity" => s.opacity = v.trim().parse::<f32>().unwrap_or(1.0).clamp(0.0, 1.0),
        "transform" => if let Some(x) = parse_transform(v, fs) { s.transform = x },
        "border" => {
            let mut color = s.color;
            for t in tokens(v) {
                match t.to_ascii_lowercase().as_str() {
                    "none" | "hidden" => s.border_width = 0.0,
                    "solid" | "dashed" | "dotted" | "double" => {}
                    _ => if let Some(c) = parse_color(t) { color = c } else if let Some(w) = border_width(t, fs) { s.border_width = w },
                }
            }
            s.border_color = color;
        }
        "border-width" => if let Some(w) = tokens(v).first().and_then(|t| border_width(t, fs)) { s.border_width = w },
        "border-color" => if let Some(c) = parse_color(v) { s.border_color = c },
        "border-style" => if lv == "none" || lv == "hidden" { s.border_width = 0.0 },
        "border-radius" => if let Some(r) = tokens(v).first().and_then(|t| match parse_length(t, fs) {
            Some(Length::Px(p)) => Some(p), Some(Length::Percent(_)) => Some(255.0), _ => None }) { s.radius = r },
        "flex-direction" => s.flex_row = !lv.starts_with("column"),
        "flex-wrap" => s.flex_wrap = lv.starts_with("wrap"),
        "gap" | "column-gap" | "row-gap" => if let Some(g) = tokens(v).first().and_then(|t| px(t, fs)) { s.gap = g },
        _ => {}
    }
}
