use crate::app::App;
use crate::css::parse_color;
use crate::dom::{Dom, NodeId, ROOT};
use crate::selector;
use eframe::egui;
use std::{
    collections::{HashMap, HashSet},
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
        Arc, Mutex, MutexGuard, OnceLock,
    },
    thread,
    time::Duration,
};

pub(crate) enum Msg {
    Click { target: NodeId, x: f32, y: f32 },
    Input { target: NodeId, value: String },
    Change { target: NodeId, checked: bool },
    /// `mouseenter` (hover started) / `mouseleave` (hover ended) — non-bubbling.
    Hover { enter: bool, target: NodeId },
    /// Keyboard events (`keydown` / `keyup`), dispatched to the focused element.
    Key {
        target: NodeId,
        key: String,
        code: String,
        pressed: bool,
        shift: bool,
        ctrl: bool,
        alt: bool,
        meta: bool,
        repeat: bool,
    },
}

type Listener = Arc<Mutex<dyn FnMut(&mut Event) + Send>>;

/// A decoded image, shared between the loader thread and the UI thread.
pub(crate) struct DecodedImage { pub w: u32, pub h: u32, pub rgba: Vec<u8> }

pub(crate) enum ImgState { Loading, Ready(Arc<DecodedImage>), Failed }

/// `<img>` sources that have been (or are being) loaded and decoded.
#[derive(Default)]
pub(crate) struct Images { map: Mutex<HashMap<String, ImgState>> }

/// Pixel buffer backing a `<canvas>` element (straight RGBA).
pub(crate) struct CanvasBuf {
    pub w: u32,
    pub h: u32,
    pub pixels: Vec<u8>,
    pub fill: [u8; 4],
    pub stroke: [u8; 4],
    pub line_width: f32,
    pub font_size: f32,
    pub version: u64,
    /// Pending publication (version bump) at the end of the current tick.
    pub dirty: bool,
    pub path: Vec<(f32, f32)>,
    pub texts: Vec<(String, f32, f32, f32, [u8; 4])>, // text, x, y, font size, color
}

impl CanvasBuf {
    fn new(w: u32, h: u32) -> Self {
        let w = w.clamp(1, 8192); let h = h.clamp(1, 8192);
        CanvasBuf { w, h, pixels: vec![0; (w as usize) * (h as usize) * 4], fill: [0, 0, 0, 255], stroke: [0, 0, 0, 255], line_width: 1.0, font_size: 16.0, version: 0, dirty: false, path: vec![], texts: vec![] }
    }
}

/// 2D drawing context for a `<canvas>` element (`el.get_context("2d")`).
#[derive(Clone)]
pub struct Canvas2D { shared: Arc<Shared>, id: NodeId }

pub(crate) struct Shared {
    dom: Mutex<Dom>,
    listeners: Mutex<HashMap<NodeId, HashMap<String, Vec<Listener>>>>,
    pub dirty: AtomicBool,
    alive: AtomicBool,
    ctx: OnceLock<egui::Context>,
    pub title: Mutex<Option<String>>,
    /// Node currently under the pointer (`usize::MAX` = none), for `:hover`.
    pub hover: std::sync::atomic::AtomicUsize,
    /// Element with keyboard focus (`usize::MAX` = none). `keydown`/`keyup`
    /// events target this element, or `<body>` when nothing is focused.
    pub focus: std::sync::atomic::AtomicUsize,
    /// Elements whose UI state must be re-seeded from the DOM (programmatic
    /// `set_value` / `set_checked`).
    pub reset: Mutex<HashSet<NodeId>>,
    pub images: Images,
    pub canvases: Mutex<HashMap<NodeId, CanvasBuf>>,
    /// `true` while a `set_interval` callback runs: canvas ops accumulate in
    /// the buffer and are published (version bump) only at the end of the
    /// tick, so the UI never paints a half-drawn frame (flicker).
    pub canvas_drawing: std::sync::atomic::AtomicBool,
}

pub(crate) const NO_HOVER: usize = usize::MAX;

impl Shared {
    pub fn dom(&self) -> MutexGuard<'_, Dom> { self.dom.lock().unwrap_or_else(|e| e.into_inner()) }

    fn invalidate(&self) {
        self.dirty.store(true, Ordering::Release);
        if let Some(c) = self.ctx.get() { c.request_repaint(); }
    }

    /// Publish all canvases mutated during the current tick: one version bump
    /// per canvas and a single repaint request (atomic animation frames).
    fn publish_canvases(&self) {
        let mut any = false;
        {
            let mut cs = self.canvases.lock().unwrap_or_else(|e| e.into_inner());
            for b in cs.values_mut() {
                if b.dirty {
                    b.dirty = false;
                    b.version += 1;
                    any = true;
                }
            }
        }
        if any { self.invalidate(); }
    }
}

// ------------------------------------------------------------------ key names

/// JS-style `KeyboardEvent.key` string for an egui [`egui::Key`]. Letters
/// are lowercase unless Shift is held (matching browsers).
pub(crate) fn key_name(k: egui::Key, shift: bool) -> String {
    use egui::Key as K;
    match k {
        K::ArrowDown => "ArrowDown".into(),
        K::ArrowLeft => "ArrowLeft".into(),
        K::ArrowRight => "ArrowRight".into(),
        K::ArrowUp => "ArrowUp".into(),
        K::Space => " ".into(),
        K::Colon => ":".into(),
        K::Comma => ",".into(),
        K::Backslash => "\\".into(),
        K::Slash => "/".into(),
        K::Pipe => "|".into(),
        K::Questionmark => "?".into(),
        K::Exclamationmark => "!".into(),
        K::OpenBracket => "[".into(),
        K::CloseBracket => "]".into(),
        K::OpenCurlyBracket => "{".into(),
        K::CloseCurlyBracket => "}".into(),
        K::Backtick => "`".into(),
        K::Minus => "-".into(),
        K::Period => ".".into(),
        K::Plus => "+".into(),
        K::Equals => "=".into(),
        K::Semicolon => ";".into(),
        K::Quote => "'".into(),
        K::Num0 => "0".into(),
        K::Num1 => "1".into(),
        K::Num2 => "2".into(),
        K::Num3 => "3".into(),
        K::Num4 => "4".into(),
        K::Num5 => "5".into(),
        K::Num6 => "6".into(),
        K::Num7 => "7".into(),
        K::Num8 => "8".into(),
        K::Num9 => "9".into(),
        K::A => letter('a', shift),
        K::B => letter('b', shift),
        K::C => letter('c', shift),
        K::D => letter('d', shift),
        K::E => letter('e', shift),
        K::F => letter('f', shift),
        K::G => letter('g', shift),
        K::H => letter('h', shift),
        K::I => letter('i', shift),
        K::J => letter('j', shift),
        K::K => letter('k', shift),
        K::L => letter('l', shift),
        K::M => letter('m', shift),
        K::N => letter('n', shift),
        K::O => letter('o', shift),
        K::P => letter('p', shift),
        K::Q => letter('q', shift),
        K::R => letter('r', shift),
        K::S => letter('s', shift),
        K::T => letter('t', shift),
        K::U => letter('u', shift),
        K::V => letter('v', shift),
        K::W => letter('w', shift),
        K::X => letter('x', shift),
        K::Y => letter('y', shift),
        K::Z => letter('z', shift),
        other => other.name().to_string(), // Escape, Tab, Enter, Home, F1..F35, ...
    }
}

/// JS-style `KeyboardEvent.code` string (physical key). egui exposes only
/// logical keys here, so digits/letters use `Digit{n}` / `KeyX` names.
pub(crate) fn code_name(k: egui::Key) -> String {
    use egui::Key as K;
    match k {
        K::A => "KeyA".into(),
        K::B => "KeyB".into(),
        K::C => "KeyC".into(),
        K::D => "KeyD".into(),
        K::E => "KeyE".into(),
        K::F => "KeyF".into(),
        K::G => "KeyG".into(),
        K::H => "KeyH".into(),
        K::I => "KeyI".into(),
        K::J => "KeyJ".into(),
        K::K => "KeyK".into(),
        K::L => "KeyL".into(),
        K::M => "KeyM".into(),
        K::N => "KeyN".into(),
        K::O => "KeyO".into(),
        K::P => "KeyP".into(),
        K::Q => "KeyQ".into(),
        K::R => "KeyR".into(),
        K::S => "KeyS".into(),
        K::T => "KeyT".into(),
        K::U => "KeyU".into(),
        K::V => "KeyV".into(),
        K::W => "KeyW".into(),
        K::X => "KeyX".into(),
        K::Y => "KeyY".into(),
        K::Z => "KeyZ".into(),
        K::Num0 => "Digit0".into(),
        K::Num1 => "Digit1".into(),
        K::Num2 => "Digit2".into(),
        K::Num3 => "Digit3".into(),
        K::Num4 => "Digit4".into(),
        K::Num5 => "Digit5".into(),
        K::Num6 => "Digit6".into(),
        K::Num7 => "Digit7".into(),
        K::Num8 => "Digit8".into(),
        K::Num9 => "Digit9".into(),
        K::Space => "Space".into(),
        K::ArrowDown => "ArrowDown".into(),
        K::ArrowLeft => "ArrowLeft".into(),
        K::ArrowRight => "ArrowRight".into(),
        K::ArrowUp => "ArrowUp".into(),
        other => other.name().to_string(),
    }
}

fn letter(c: char, shift: bool) -> String {
    if shift { c.to_ascii_uppercase() } else { c }.to_string()
}

// ------------------------------------------------------------------ Event

pub struct Event {
    pub kind: String,
    /// Innermost element that was hit.
    pub target: Element,
    /// Element whose listener is currently running (differs from `target` when bubbling).
    pub current_target: Element,
    pub x: f32,
    pub y: f32,
    /// New text for `input` events.
    pub value: Option<String>,
    /// New state for `change` events on checkboxes.
    pub checked: Option<bool>,
    /// The key for `keydown` / `keyup` events, e.g. `"a"`, `"Enter"`, `"ArrowUp"`, `" "`.
    pub key: String,
    /// The physical key code, e.g. `"KeyA"`, `"Digit1"`, `"Enter"`.
    pub code: String,
    /// Whether Shift is held (keyboard events).
    pub shift_key: bool,
    /// Whether Ctrl is held (keyboard events).
    pub ctrl_key: bool,
    /// Whether Alt is held (keyboard events).
    pub alt_key: bool,
    /// Whether the Meta/Command key is held (keyboard events).
    pub meta_key: bool,
    /// True for auto-repeated `keydown` events while a key is held.
    pub repeat: bool,
    stopped: bool,
}

impl Event {
    pub fn stop_propagation(&mut self) { self.stopped = true; }
}

fn base_event(shared: &Arc<Shared>, kind: &str, target: NodeId) -> Event {
    Event {
        kind: kind.to_string(),
        target: Element::new(shared, Some(target)),
        current_target: Element::new(shared, Some(target)),
        x: 0.0, y: 0.0, value: None, checked: None,
        key: String::new(), code: String::new(),
        shift_key: false, ctrl_key: false, alt_key: false, meta_key: false,
        repeat: false, stopped: false,
    }
}

fn dispatch(shared: &Arc<Shared>, kind: &str, target: NodeId, x: f32, y: f32, value: Option<String>, checked: Option<bool>) {
    let mut ev = base_event(shared, kind, target);
    ev.x = x; ev.y = y; ev.value = value; ev.checked = checked;
    let path: Vec<NodeId> = std::iter::successors(Some(target), |&n| shared.dom().parent(n)).collect();
    dispatch_path(shared, path, ev);
}

fn dispatch_no_bubble(shared: &Arc<Shared>, kind: &str, target: NodeId) {
    dispatch_path(shared, vec![target], base_event(shared, kind, target));
}

fn dispatch_key(
    shared: &Arc<Shared>,
    kind: &str,
    target: NodeId,
    key: &str,
    code: &str,
    shift: bool,
    ctrl: bool,
    alt: bool,
    meta: bool,
    repeat: bool,
) {
    let mut ev = base_event(shared, kind, target);
    ev.key = key.to_string();
    ev.code = code.to_string();
    ev.shift_key = shift;
    ev.ctrl_key = ctrl;
    ev.alt_key = alt;
    ev.meta_key = meta;
    ev.repeat = repeat;
    let path: Vec<NodeId> = std::iter::successors(Some(target), |&n| shared.dom().parent(n)).collect();
    dispatch_path(shared, path, ev);
}

fn dispatch_path(shared: &Arc<Shared>, path: Vec<NodeId>, mut ev: Event) {
    for node in path {
        let ls: Vec<Listener> = {
            let l = shared.listeners.lock().unwrap_or_else(|e| e.into_inner());
            l.get(&node).and_then(|m| m.get(&ev.kind)).cloned().unwrap_or_default()
        };
        if ls.is_empty() { continue; }
        ev.current_target = Element::new(shared, Some(node));
        for l in ls {
            // No locks are held here, so handlers may freely read/mutate the DOM.
            let mut f = l.lock().unwrap_or_else(|e| e.into_inner());
            let _ = catch_unwind(AssertUnwindSafe(|| (*f)(&mut ev)));
        }
        if ev.stopped { break; }
    }
}

/// Where keyboard events land: the focused element, or `<body>` (falling back
/// to the document root) when nothing is focused.
fn key_target(shared: &Shared, focus: NodeId) -> NodeId {
    if focus == NO_HOVER {
        shared.dom().find_tag(ROOT, "body").unwrap_or(ROOT)
    } else {
        focus
    }
}

fn event_loop(shared: Arc<Shared>, rx: Receiver<Msg>) {
    while let Ok(m) = rx.recv() {
        match m {
            Msg::Click { target, x, y } => dispatch(&shared, "click", target, x, y, None, None),
            Msg::Input { target, value } => {
                shared.dom().set_attr(target, "value", &value); // no re-style needed
                dispatch(&shared, "input", target, 0.0, 0.0, Some(value), None);
            }
            Msg::Change { target, checked } => {
                if checked { shared.dom().set_attr(target, "checked", ""); } else { shared.dom().remove_attr(target, "checked"); }
                dispatch(&shared, "change", target, 0.0, 0.0, None, Some(checked));
            }
            Msg::Hover { enter, target } => {
                dispatch_no_bubble(&shared, if enter { "mouseenter" } else { "mouseleave" }, target);
            }
            Msg::Key { target, key, code, pressed, shift, ctrl, alt, meta, repeat } => {
                let t = key_target(&shared, target);
                dispatch_key(&shared, if pressed { "keydown" } else { "keyup" }, t, &key, &code, shift, ctrl, alt, meta, repeat);
            }
        }
    }
}

// ------------------------------------------------------------------ Element

/// A handle to a DOM element. Cheap to clone, `Send + Sync`. If a selector matched
/// nothing the handle is "empty": reads return defaults and writes are no-ops,
/// so chaining like `document.query_selector("#x").add_event_listener(..)` never panics.
#[derive(Clone)]
pub struct Element { shared: Arc<Shared>, id: Option<NodeId> }

impl Element {
    fn new(shared: &Arc<Shared>, id: Option<NodeId>) -> Element { Element { shared: shared.clone(), id } }

    fn read<R: Default>(&self, f: impl FnOnce(&Dom, NodeId) -> R) -> R {
        match self.id { Some(id) => f(&self.shared.dom(), id), None => R::default() }
    }
    fn write(&self, f: impl FnOnce(&mut Dom, NodeId)) {
        if let Some(id) = self.id {
            f(&mut self.shared.dom(), id);
            self.shared.invalidate();
        }
    }

    pub fn exists(&self) -> bool { self.id.is_some() }

    pub fn query_selector(&self, sel: &str) -> Element {
        let id = self.id.and_then(|id| self.shared.dom().query(id, &selector::parse_list(sel), false).first().copied());
        Element::new(&self.shared, id)
    }

    pub fn query_selector_all(&self, sel: &str) -> Vec<Element> {
        let Some(id) = self.id else { return vec![] };
        let ids = self.shared.dom().query(id, &selector::parse_list(sel), true);
        ids.into_iter().map(|i| Element::new(&self.shared, Some(i))).collect()
    }

    /// Listen for `kind` ("click", "input", "change", "mouseenter", "mouseleave")
    /// on this element (pointer events bubble; mouse enter/leave do not).
    /// Handlers run on the dispatcher thread, never on the UI thread.
    pub fn add_event_listener(&self, kind: &str, f: impl FnMut(&mut Event) + Send + 'static) {
        if let Some(id) = self.id {
            let l: Listener = Arc::new(Mutex::new(f));
            self.shared.listeners.lock().unwrap_or_else(|e| e.into_inner())
                .entry(id).or_default().entry(kind.to_string()).or_default().push(l);
        }
    }

    pub fn remove_event_listeners(&self, kind: &str) {
        if let Some(id) = self.id {
            if let Some(m) = self.shared.listeners.lock().unwrap_or_else(|e| e.into_inner()).get_mut(&id) { m.remove(kind); }
        }
    }

    pub fn tag_name(&self) -> String { self.read(|d, i| d.tag(i).unwrap_or("").to_string()) }
    pub fn text(&self) -> String { self.text_content() }
    /// Concatenated text of this element and all its descendants.
    pub fn text_content(&self) -> String { self.read(|d, i| d.text_content(i)) }
    /// Replace all children with a single text node (like `Node.textContent`).
    pub fn set_text_content(&self, text: impl AsRef<str>) { self.set_text(text) }
    pub fn set_text(&self, text: impl AsRef<str>) { self.write(|d, i| d.set_text(i, text.as_ref())) }
    /// The element's children serialized back to HTML.
    pub fn inner_html(&self) -> String { self.read(|d, i| { let mut s = String::new(); d.serialize_children(i, &mut s); s }) }
    pub fn set_inner_html(&self, html: impl AsRef<str>) { self.write(|d, i| d.set_inner_html(i, html.as_ref())) }
    pub fn append_html(&self, html: impl AsRef<str>) { self.write(|d, i| d.append_html(i, html.as_ref())) }

    /// Move `child` so it becomes this element's last child (like `appendChild`;
    /// moves an already-attached element rather than copying it).
    pub fn append(&self, child: &Element) {
        let Some(cid) = child.id else { return };
        self.write(|d, i| d.adopt(cid, i));
    }

    /// Remove this element (and its subtree) from the document; drops its
    /// event listeners too. No-op on detached/already-removed handles.
    pub fn remove(&self) {
        let Some(id) = self.id else { return };
        let gone: Vec<NodeId> = {
            let d = self.shared.dom();
            if d.parent(id).is_none() && id != ROOT { return; }
            let mut v = vec![id];
            v.extend(d.descendants(id));
            v
        };
        self.shared.dom().remove(id);
        self.shared.invalidate();
        let mut ls = self.shared.listeners.lock().unwrap_or_else(|e| e.into_inner());
        for g in gone { ls.remove(&g); }
    }

    pub fn attr(&self, name: &str) -> Option<String> { self.read(|d, i| d.attr(i, name).map(str::to_string)) }
    pub fn set_attr(&self, name: &str, val: impl AsRef<str>) { self.write(|d, i| d.set_attr(i, name, val.as_ref())) }
    pub fn remove_attr(&self, name: &str) { self.write(|d, i| d.remove_attr(i, name)) }

    pub fn has_class(&self, c: &str) -> bool { self.read(|d, i| d.attr(i, "class").map_or(false, |s| s.split_whitespace().any(|w| w == c))) }
    pub fn add_class(&self, c: &str) { self.class_list().add(c) }
    pub fn remove_class(&self, c: &str) { self.class_list().remove(c) }
    pub fn toggle_class(&self, c: &str) { self.class_list().toggle(c); }

    /// The element's class attribute as a `ClassList` (add/remove/toggle/contains).
    pub fn class_list(&self) -> ClassList { ClassList { el: self.clone() } }

    /// The element's inline CSS (`CssStyle` handle: set/get/remove properties).
    pub fn style(&self) -> CssStyle { CssStyle { el: self.clone() } }

    /// Set one inline CSS property (`""` removes it): `el.style().set("background-color", "#f00")`.
    pub fn set_style(&self, prop: &str, val: &str) { self.style().set(prop, val) }

    /// Current text of an `<input>` / `<textarea>`.
    pub fn value(&self) -> String {
        self.read(|d, i| d.attr(i, "value").map(str::to_string).unwrap_or_else(|| if d.tag(i) == Some("textarea") { d.text_content(i) } else { String::new() }))
    }
    pub fn set_value(&self, v: impl AsRef<str>) {
        if let Some(id) = self.id { self.shared.reset.lock().unwrap_or_else(|e| e.into_inner()).insert(id); }
        self.write(|d, i| d.set_attr(i, "value", v.as_ref()));
    }
    pub fn checked(&self) -> bool { self.read(|d, i| d.attr(i, "checked").is_some()) }
    pub fn set_checked(&self, on: bool) {
        if let Some(id) = self.id { self.shared.reset.lock().unwrap_or_else(|e| e.into_inner()).insert(id); }
        self.write(|d, i| if on { d.set_attr(i, "checked", "") } else { d.remove_attr(i, "checked") });
    }

    /// A 2D drawing context for a `<canvas>` element, creating the backing
    /// buffer if needed (size = `width`/`height` attributes, default 300×150).
    pub fn get_context(&self, kind: &str) -> Option<Canvas2D> {
        if kind != "2d" { return None; }
        let id = self.id?;
        let (w, h) = self.read(|d, i| {
            let n = |a: &str| d.attr(i, a).and_then(|v| v.trim().parse::<u32>().ok()).unwrap_or(0);
            (n("width"), n("height"))
        });
        let mut cs = self.shared.canvases.lock().unwrap_or_else(|e| e.into_inner());
        cs.entry(id).or_insert_with(|| CanvasBuf::new(if w > 0 { w } else { 300 }, if h > 0 { h } else { 150 }));
        Some(Canvas2D { shared: self.shared.clone(), id })
    }

    pub fn parent(&self) -> Element {
        let id = self.id.and_then(|i| self.shared.dom().parent(i)).filter(|&p| p != ROOT);
        Element::new(&self.shared, id)
    }
    pub fn children(&self) -> Vec<Element> {
        let Some(id) = self.id else { return vec![] };
        let kids: Vec<NodeId> = self.shared.dom().element_children(id).collect();
        kids.into_iter().map(|k| Element::new(&self.shared, Some(k))).collect()
    }
}

// ------------------------------------------------------------------ ClassList / CssStyle

/// The `class` attribute of an element (like the DOM's `classList`).
#[derive(Clone)]
pub struct ClassList { el: Element }

impl ClassList {
    pub fn add(&self, class: &str) {
        for c in class.split_whitespace() {
            if self.contains(c) { continue; }
            self.el.write(|d, i| {
                let s = format!("{} {}", d.attr(i, "class").unwrap_or(""), c);
                d.set_attr(i, "class", s.trim());
            });
        }
    }
    pub fn remove(&self, class: &str) {
        for c in class.split_whitespace() {
            self.el.write(|d, i| {
                let s = d.attr(i, "class").unwrap_or("").split_whitespace().filter(|w| *w != c).collect::<Vec<_>>().join(" ");
                d.set_attr(i, "class", &s);
            });
        }
    }
    /// Toggles the class, returns `true` when it is now present.
    pub fn toggle(&self, class: &str) -> bool {
        if self.contains(class) { self.remove(class); false } else { self.add(class); true }
    }
    pub fn contains(&self, class: &str) -> bool { self.el.has_class(class) }
    pub fn len(&self) -> usize { self.el.read(|d, i| d.attr(i, "class").map_or(0, |s| s.split_whitespace().count())) }
    pub fn is_empty(&self) -> bool { self.len() == 0 }
    pub fn item(&self, idx: usize) -> Option<String> {
        self.el.read(|d, i| d.attr(i, "class").and_then(|s| s.split_whitespace().nth(idx)).map(str::to_string))
    }
}

impl std::fmt::Display for ClassList {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.el.read(|d, i| d.attr(i, "class").unwrap_or("").to_string()))
    }
}

/// An element's inline CSS (like the DOM's `element.style`).
#[derive(Clone)]
pub struct CssStyle { el: Element }

impl CssStyle {
    /// Set a property; an empty value removes it. `set("width", "120px")`.
    pub fn set(&self, prop: &str, val: impl AsRef<str>) { self.el.write(|d, i| d.set_style_prop(i, prop, val.as_ref())) }
    pub fn set_property(&self, prop: &str, val: impl AsRef<str>) { self.set(prop, val) }
    pub fn get(&self, prop: &str) -> Option<String> {
        self.el.read(|d, i| {
            let s = d.attr(i, "style")?;
            s.split(';').filter_map(|d| d.split_once(':')).find(|(k, _)| k.trim().eq_ignore_ascii_case(prop)).map(|(_, v)| v.trim().to_string())
        })
    }
    pub fn get_property_value(&self, prop: &str) -> Option<String> { self.get(prop) }
    pub fn remove(&self, prop: &str) { self.set(prop, "") }
    pub fn css_text(&self) -> String { self.el.read(|d, i| d.attr(i, "style").unwrap_or("").to_string()) }
    pub fn set_css_text(&self, css: &str) { self.el.write(|d, i| d.set_attr(i, "style", css)) }
}

// ------------------------------------------------------------------ Canvas2D

impl Canvas2D {
    fn with(&self, f: impl FnOnce(&mut CanvasBuf)) {
        let published = {
            let mut cs = self.shared.canvases.lock().unwrap_or_else(|e| e.into_inner());
            let Some(b) = cs.get_mut(&self.id) else { return };
            f(b);
            b.dirty = true;
            // Outside a tick (e.g. a click handler) publish immediately so
            // single-shot draws show up without waiting for anything.
            if !self.shared.canvas_drawing.load(Ordering::Relaxed) {
                b.dirty = false;
                b.version += 1;
                true
            } else {
                false
            }
        };
        if published { self.shared.invalidate(); }
    }

    pub fn set_fill_style(&self, css_color: &str) {
        self.with(|b| if let Some(c) = parse_color(css_color) { b.fill = [c.r(), c.g(), c.b(), c.a()] });
    }
    pub fn set_stroke_style(&self, css_color: &str) {
        self.with(|b| if let Some(c) = parse_color(css_color) { b.stroke = [c.r(), c.g(), c.b(), c.a()] });
    }
    pub fn set_line_width(&self, w: f32) { self.with(|b| b.line_width = w.max(0.1)) }
    /// Font size used by `fill_text`.
    pub fn set_font_size(&self, size: f32) { self.with(|b| b.font_size = size.max(1.0)) }

    pub fn size(&self) -> (u32, u32) {
        self.shared.canvases.lock().unwrap_or_else(|e| e.into_inner()).get(&self.id).map_or((0, 0), |b| (b.w, b.h))
    }

    /// Resize the backing buffer, clearing it.
    pub fn set_size(&self, w: u32, h: u32) {
        self.with(|b| *b = CanvasBuf::new(w, h));
    }

    // ---- paths ----
    pub fn begin_path(&self) { self.with(|b| b.path.clear()); }
    pub fn close_path(&self) { self.with(|b| if let Some(&f) = b.path.first() { b.path.push(f); }) }
    pub fn move_to(&self, x: f32, y: f32) { self.with(|b| { b.path.clear(); b.path.push((x, y)); }); }
    pub fn line_to(&self, x: f32, y: f32) { self.with(|b| b.path.push((x, y))); }
    pub fn fill(&self) { let pts = self.take_path(); self.with(|b| fill_poly(b, &pts, b.fill)); }
    pub fn stroke(&self) { let pts = self.take_path(); self.with(|b| { for w in pts.windows(2) { draw_line(b, w[0], w[1], b.stroke, b.line_width); } }); }
    fn take_path(&self) -> Vec<(f32, f32)> {
        let mut cs = self.shared.canvases.lock().unwrap_or_else(|e| e.into_inner());
        cs.get_mut(&self.id).map(|b| std::mem::take(&mut b.path)).unwrap_or_default()
    }

    // ---- shapes ----
    pub fn fill_rect(&self, x: f32, y: f32, w: f32, h: f32) {
        self.with(|b| { let c = b.fill; fill_rect_impl(b, x, y, w, h, c); });
    }
    /// Erase a rectangle to transparent.
    pub fn clear_rect(&self, x: f32, y: f32, w: f32, h: f32) {
        self.with(|b| { let c = [0, 0, 0, 0]; fill_rect_impl(b, x, y, w, h, c); });
    }
    pub fn clear(&self) {
        self.with(|b| { b.pixels.iter_mut().for_each(|p| *p = 0); b.texts.clear(); });
    }
    pub fn stroke_rect(&self, x: f32, y: f32, w: f32, h: f32) {
        self.with(|b| {
            let (c, lw) = (b.stroke, b.line_width);
            for (x1, y1, x2, y2) in [(x, y, x + w, y), (x, y + h, x + w, y + h), (x, y, x, y + h), (x + w, y, x + w, y + h)] {
                draw_line(b, (x1, y1), (x2, y2), c, lw);
            }
        });
    }
    pub fn fill_circle(&self, cx: f32, cy: f32, r: f32) { self.with(|b| fill_circle(b, cx, cy, r, b.fill, 0.0)); }
    pub fn stroke_circle(&self, cx: f32, cy: f32, r: f32) {
        self.with(|b| { let (c, lw) = (b.stroke, b.line_width); circle_band(b, cx, cy, r, lw, c); });
    }
    pub fn draw_line(&self, x1: f32, y1: f32, x2: f32, y2: f32) {
        self.with(|b| { let c = b.stroke; let lw = b.line_width; draw_line(b, (x1, y1), (x2, y2), c, lw); });
    }
    /// Draw a closed polygon (convex or concave, even-odd rule).
    pub fn fill_polygon(&self, pts: &[(f32, f32)]) {
        self.with(|b| { let c = b.fill; fill_poly(b, pts.to_vec().as_slice(), c); });
    }
    /// Draw text with the current font size and fill color. Rendered as
    /// a crisp overlay at draw time (egui fonts), not into the pixel buffer.
    pub fn fill_text(&self, text: impl Into<String>, x: f32, y: f32) {
        self.with(|b| b.texts.push((text.into(), x, y, b.font_size, b.fill)));
    }
}

fn blend(b: &mut CanvasBuf, x: i32, y: i32, c: [u8; 4], cov: f32) {
    if cov <= 0.0 || x < 0 || y < 0 || x >= b.w as i32 || y >= b.h as i32 { return; }
    let i = ((y as u32) * b.w + x as u32) as usize * 4;
    let a = (c[3] as f32 / 255.0).min(1.0) * cov;
    if a >= 1.0 { b.pixels[i..i + 4].copy_from_slice(&c); return; }
    let inv = 1.0 - a;
    for k in 0..3 { b.pixels[i + k] = (c[k] as f32 * a + b.pixels[i + k] as f32 * inv).round() as u8; }
    b.pixels[i + 3] = ((c[3] as f32 * a + b.pixels[i + 3] as f32 * inv)).round() as u8;
}

fn fill_rect_impl(b: &mut CanvasBuf, x: f32, y: f32, w: f32, h: f32, c: [u8; 4]) {
    let (x0, y0) = (x.round().max(0.0) as i32, y.round().max(0.0) as i32);
    let (x1, y1) = ((x + w).round() as i32, (y + h).round() as i32);
    for yy in y0..y1 { for xx in x0..x1 { blend(b, xx, yy, c, 1.0); } }
}

fn fill_circle(b: &mut CanvasBuf, cx: f32, cy: f32, r: f32, c: [u8; 4], band: f32) {
    let outer = r + band / 2.0 + 0.5;
    for yy in (cy - outer).round() as i32..=(cy + outer).round() as i32 {
        for xx in (cx - outer).round() as i32..=(cx + outer).round() as i32 {
            let d = ((xx as f32 + 0.5 - cx).powi(2) + (yy as f32 + 0.5 - cy).powi(2)).sqrt();
            // 1px soft edge: solid disc when band == 0, ring band otherwise
            let cov = if band <= 0.0 {
                (r + 0.5 - d).clamp(0.0, 1.0)
            } else {
                (band / 2.0 + 0.5 - (d - r).abs()).clamp(0.0, 1.0)
            };
            if cov > 0.0 { blend(b, xx, yy, c, cov); }
        }
    }
}

fn circle_band(b: &mut CanvasBuf, cx: f32, cy: f32, r: f32, lw: f32, c: [u8; 4]) {
    fill_circle(b, cx, cy, r, c, lw);
}

fn draw_line(b: &mut CanvasBuf, p1: (f32, f32), p2: (f32, f32), c: [u8; 4], lw: f32) {
    let (dx, dy) = (p2.0 - p1.0, p2.1 - p1.0);
    let len = (dx * dx + dy * dy).sqrt().max(0.01);
    let steps = (len * 2.0).ceil() as usize;
    let r = (lw / 2.0).max(0.5);
    for s in 0..=steps {
        let t = s as f32 / steps as f32;
        let (x, y) = (p1.0 + dx * t, p1.1 + dy * t);
        // stamp a soft disc
        for yy in (y - r).round() as i32..=(y + r).round() as i32 {
            for xx in (x - r).round() as i32..=(x + r).round() as i32 {
                let d = ((xx as f32 + 0.5 - x).powi(2) + (yy as f32 + 0.5 - y).powi(2)).sqrt();
                let cov = (r - d).max(0.0).min(1.0);
                if cov > 0.0 { blend(b, xx, yy, c, cov); }
            }
        }
    }
}

fn fill_poly(b: &mut CanvasBuf, pts: &[(f32, f32)], c: [u8; 4]) {
    if pts.len() < 2 { return; }
    let (mut ymin, mut ymax) = (f32::MAX, f32::MIN);
    for (x, y) in pts { ymin = ymin.min(*y); ymax = ymax.max(*y); let _ = x; }
    for row in ymin.round().max(0.0) as i32..=ymax.round() as i32 {
        let ycen = row as f32 + 0.5;
        let mut xs: Vec<f32> = vec![];
        for w in pts.windows(2) {
            let (x1, y1, x2, y2) = (w[0].0, w[0].1, w[1].0, w[1].1);
            if (y1 <= ycen && y2 > ycen) || (y2 <= ycen && y1 > ycen) {
                xs.push(x1 + (ycen - y1) * (x2 - x1) / (y2 - y1));
            }
        }
        xs.sort_unstable_by(f32::total_cmp);
        for pair in xs.chunks(2) {
            if let [xa, xb] = pair {
                for xx in (*xa).round() as i32..(*xb).round() as i32 { blend(b, xx, row, c, 1.0); }
            }
        }
    }
}

// ------------------------------------------------------------------ images

/// Returns the decoded image for `src`, starting a background load on first use.
/// Sources: file paths (`"img/logo.png"`, `"file://…"`), `data:` URIs (base64).
pub(crate) fn get_image(shared: &Arc<Shared>, src: &str) -> Option<Arc<DecodedImage>> {
    let start = {
        let mut m = shared.images.map.lock().unwrap_or_else(|e| e.into_inner());
        match m.get(src) {
            Some(ImgState::Ready(d)) => return Some(d.clone()),
            Some(_) => return None, // loading or failed
            None => { m.insert(src.to_string(), ImgState::Loading); true }
        }
    };
    if start {
        let (sh, src) = (shared.clone(), src.to_string());
        thread::spawn(move || {
            let ok = decode_src(&src);
            let mut m = sh.images.map.lock().unwrap_or_else(|e| e.into_inner());
            match ok {
                Some(d) => { m.insert(src.clone(), ImgState::Ready(Arc::new(d))); }
                None => { log::warn!("ricehtml: failed to load image: {src}"); m.insert(src.clone(), ImgState::Failed); }
            }
            drop(m);
            sh.invalidate();
        });
    }
    None
}

fn decode_src(src: &str) -> Option<DecodedImage> {
    let bytes: Vec<u8> = if let Some(rest) = src.strip_prefix("data:") {
        let (meta, payload) = rest.split_once(',')?;
        if !meta.ends_with("base64") { return None; }
        b64_decode(payload)?
    } else if let Some(path) = src.strip_prefix("file://") {
        std::fs::read(path).ok()?
    } else {
        std::fs::read(src).ok()?
    };
    let img = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format().ok()?.decode().ok()?;
    Some(DecodedImage { w: img.width(), h: img.height(), rgba: img.to_rgba8().into_raw() })
}

fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut acc: u32 = 0; let mut bits = 0;
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' | b'\r' | b'\n' | b' ' | b'\t' => continue,
            _ => return None,
        } as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xFF) as u8);
        }
    }
    Some(out)
}

// ------------------------------------------------------------------ Document

#[derive(Clone)]
pub struct Document { shared: Arc<Shared> }

impl Document {
    fn root(&self) -> Element { Element::new(&self.shared, Some(ROOT)) }
    pub fn query_selector(&self, sel: &str) -> Element { self.root().query_selector(sel) }
    pub fn query_selector_all(&self, sel: &str) -> Vec<Element> { self.root().query_selector_all(sel) }
    pub fn get_element_by_id(&self, id: &str) -> Element { self.query_selector(&format!("#{id}")) }
    pub fn body(&self) -> Element { self.query_selector("body") }
    /// Create a detached element (e.g. `document.create_element("li")`); attach
    /// it later with `parent.append(&el)`.
    pub fn create_element(&self, tag: &str) -> Element {
        let id = self.shared.dom().new_element(tag);
        Element::new(&self.shared, Some(id))
    }
    /// Listen for events bubbling up from anywhere in the document.
    pub fn add_event_listener(&self, kind: &str, f: impl FnMut(&mut Event) + Send + 'static) { self.root().add_event_listener(kind, f) }

    pub fn set_title(&self, t: impl Into<String>) {
        *self.shared.title.lock().unwrap_or_else(|e| e.into_inner()) = Some(t.into());
        if let Some(c) = self.shared.ctx.get() { c.request_repaint(); }
    }

    /// Call `f` every `every` until the window closes (own thread). The whole
    /// callback is one animation tick: canvas draws within it are published
    /// atomically at the end, never mid-draw.
    pub fn set_interval(&self, every: Duration, mut f: impl FnMut() + Send + 'static) {
        let shared = self.shared.clone();
        thread::spawn(move || loop {
            thread::sleep(every);
            if !shared.alive.load(Ordering::Relaxed) { break; }
            shared.canvas_drawing.store(true, Ordering::Relaxed);
            f();
            shared.publish_canvases();
            shared.canvas_drawing.store(false, Ordering::Relaxed);
        });
    }

    pub fn set_timeout(&self, after: Duration, f: impl FnOnce() + Send + 'static) {
        let shared = self.shared.clone();
        thread::spawn(move || {
            thread::sleep(after);
            if shared.alive.load(Ordering::Relaxed) { f(); }
        });
    }

    pub fn is_open(&self) -> bool { self.shared.alive.load(Ordering::Relaxed) }
}

static DOCUMENT: OnceLock<Document> = OnceLock::new();

/// Global access to the document (panics if called before `spawn`).
pub fn document() -> Document { DOCUMENT.get().expect("ricehtml::document() called before spawn()").clone() }

// ------------------------------------------------------------------ spawn

#[derive(Debug, Clone)]
pub struct Options {
    pub size: [f32; 2],
    /// TTF/OTF bytes used for `font-weight: bold` (egui ships no bold face).
    pub bold_font: Option<&'static [u8]>,
}
impl Default for Options {
    fn default() -> Self { Options { size: [800.0, 600.0], bold_font: None } }
}

pub fn spawn<F: FnOnce(Document) + Send + 'static>(html: &str, background: F) -> eframe::Result {
    spawn_with(html, Options::default(), background)
}

/// Opens the window (blocks the calling thread until it closes) and runs `background`
/// on its own thread with a handle to the document.
pub fn spawn_with<F: FnOnce(Document) + Send + 'static>(html: &str, opts: Options, background: F) -> eframe::Result {
    let dom = Dom::parse(html);
    let title = dom.find_tag(ROOT, "title")
        .map(|t| dom.text_content(t).trim().to_string())
        .unwrap_or_else(|| "Untitled".into());

    let shared = Arc::new(Shared {
        dom: Mutex::new(dom),
        listeners: Mutex::new(HashMap::new()),
        dirty: AtomicBool::new(true),
        alive: AtomicBool::new(true),
        ctx: OnceLock::new(),
        title: Mutex::new(None),
        hover: std::sync::atomic::AtomicUsize::new(NO_HOVER),
        focus: std::sync::atomic::AtomicUsize::new(NO_HOVER),
        reset: Mutex::new(HashSet::new()),
        images: Images::default(),
        canvases: Mutex::new(HashMap::new()),
        canvas_drawing: std::sync::atomic::AtomicBool::new(false),
    });
    let doc = Document { shared: shared.clone() };
    let _ = DOCUMENT.set(doc.clone());

    let (tx, rx) = mpsc::channel();
    { let s = shared.clone(); thread::spawn(move || event_loop(s, rx)); }
    thread::spawn(move || background(doc));

    let native = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title(title.clone()).with_inner_size(opts.size),
        ..Default::default()
    };
    let (s2, bold) = (shared.clone(), opts.bold_font);
    let res = eframe::run_native(&title, native, Box::new(move |cc| {
        cc.egui_ctx.set_visuals(egui::Visuals::light());
        if let Some(bytes) = bold {
            let mut fonts = egui::FontDefinitions::default();
            fonts.font_data.insert("rice-bold".into(), Arc::new(egui::FontData::from_static(bytes)));
            let mut fam = vec!["rice-bold".to_string()];
            fam.extend(fonts.families[&egui::FontFamily::Proportional].clone());
            fonts.families.insert(egui::FontFamily::Name("bold".into()), fam);
            cc.egui_ctx.set_fonts(fonts);
        }
        let _ = s2.ctx.set(cc.egui_ctx.clone());
        Ok(Box::new(App::new(s2, tx, bold.is_some())))
    }));
    shared.alive.store(false, Ordering::Relaxed);
    res
}

// ------------------------------------------------------------------ tests

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Key as K;

    #[test]
    fn key_names() {
        assert_eq!(key_name(K::Enter, false), "Enter");
        assert_eq!(key_name(K::Space, false), " ");
        assert_eq!(key_name(K::ArrowUp, false), "ArrowUp");
        assert_eq!(key_name(K::ArrowDown, false), "ArrowDown");
        assert_eq!(key_name(K::Escape, false), "Escape");
        assert_eq!(key_name(K::Backspace, false), "Backspace");
        assert_eq!(key_name(K::Tab, false), "Tab");
        assert_eq!(key_name(K::F2, false), "F2");
        assert_eq!(key_name(K::Home, false), "Home");
        assert_eq!(key_name(K::Num1, false), "1");
        assert_eq!(key_name(K::Period, false), ".");
        assert_eq!(key_name(K::Slash, false), "/");
        assert_eq!(key_name(K::A, false), "a");
        assert_eq!(key_name(K::A, true), "A");
        assert_eq!(key_name(K::Z, false), "z");
        assert_eq!(key_name(K::Z, true), "Z");
    }

    #[test]
    fn code_names() {
        assert_eq!(code_name(K::A), "KeyA");
        assert_eq!(code_name(K::Z), "KeyZ");
        assert_eq!(code_name(K::Num0), "Digit0");
        assert_eq!(code_name(K::Num9), "Digit9");
        assert_eq!(code_name(K::Space), "Space");
        assert_eq!(code_name(K::Enter), "Enter");
        assert_eq!(code_name(K::ArrowLeft), "ArrowLeft");
        assert_eq!(code_name(K::F12), "F12");
    }

    fn test_shared(html: &str) -> Arc<Shared> {
        Arc::new(Shared {
            dom: Mutex::new(Dom::parse(html)),
            listeners: Mutex::new(HashMap::new()),
            dirty: AtomicBool::new(false),
            alive: AtomicBool::new(true),
            ctx: OnceLock::new(),
            title: Mutex::new(None),
            hover: std::sync::atomic::AtomicUsize::new(NO_HOVER),
            focus: std::sync::atomic::AtomicUsize::new(NO_HOVER),
            reset: Mutex::new(HashSet::new()),
            images: Images::default(),
            canvases: Mutex::new(HashMap::new()),
            canvas_drawing: std::sync::atomic::AtomicBool::new(false),
        })
    }

    #[test]
    fn key_dispatch_targets_focus_and_bubbles() {
        let shared = test_shared(r#"<html><body><div id="box"><input id="field"></div></body></html>"#);
        let field = { let d = shared.dom(); d.find_tag(ROOT, "input").unwrap() };
        let body = { let d = shared.dom(); d.find_tag(ROOT, "body").unwrap() };

        // No focus → key events target <body>; focused input targets itself.
        assert_eq!(key_target(&shared, NO_HOVER), body);
        assert_eq!(key_target(&shared, field), field);

        let field_events: Arc<Mutex<Vec<(String, String, bool, bool, bool, bool, bool)>>> = Arc::default();
        {
            let fe = field_events.clone();
            Element::new(&shared, Some(field)).add_event_listener("keydown", move |ev: &mut Event| {
                fe.lock().unwrap().push((
                    ev.key.clone(), ev.code.clone(),
                    ev.shift_key, ev.ctrl_key, ev.alt_key, ev.meta_key, ev.repeat,
                ));
            });
        }

        let doc_keyups: Arc<Mutex<usize>> = Arc::default();
        {
            let ku = doc_keyups.clone();
            Element::new(&shared, Some(ROOT)).add_event_listener("keyup", move |ev: &mut Event| {
                assert_eq!(ev.current_target.tag_name(), "#document");
                *ku.lock().unwrap() += 1;
            });
        }

        dispatch_key(&shared, "keydown", field, "A", "KeyA", true, false, false, false, true);
        {
            let fe = field_events.lock().unwrap();
            assert_eq!(fe.len(), 1);
            assert_eq!(fe[0], ("A".into(), "KeyA".into(), true, false, false, false, true));
        }

        // keyup bubbles all the way to the document root.
        dispatch_key(&shared, "keyup", field, "Enter", "Enter", false, false, false, false, false);
        assert_eq!(*doc_keyups.lock().unwrap(), 1);

        // stop_propagation on <body> keeps the event from reaching the document.
        let stops: Arc<Mutex<usize>> = Arc::default();
        {
            let st = stops.clone();
            Element::new(&shared, Some(body)).add_event_listener("keydown", move |ev: &mut Event| {
                ev.stop_propagation();
                *st.lock().unwrap() += 1;
            });
        }
        let doc_keydowns: Arc<Mutex<usize>> = Arc::default();
        {
            let kd = doc_keydowns.clone();
            Element::new(&shared, Some(ROOT)).add_event_listener("keydown", move |_ev: &mut Event| {
                *kd.lock().unwrap() += 1;
            });
        }
        dispatch_key(&shared, "keydown", field, "a", "KeyA", false, false, false, false, false);
        assert_eq!(*stops.lock().unwrap(), 1);
        assert_eq!(*doc_keydowns.lock().unwrap(), 0);
    }
}
