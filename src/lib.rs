//! # RiceHTML
//!
//! Describe a UI in HTML + CSS, script it from a background thread with a DOM-like API.
//!
//! No JavaScript, no web view, no IPC bridge: RiceHTML parses a small HTML/CSS
//! subset and renders it natively with [egui](https://github.com/emilk/egui).
//!
//! ## DOM-like API
//! `document.create_element("li")` builds detached elements, `parent.append(&el)`
//! attaches them; `el.class_list()` / `el.style()` are `ClassList` / `CssStyle`
//! handles; `el.text_content()` / `el.inner_html()` / `el.value()` read state and
//! `el.set_text_content(..)` / `el.set_inner_html(..)` / `el.set_value(..)` write it.
//! `el.get_context("2d")` returns a `Canvas2D` drawing context for `<canvas>`.
//!
//! ## CSS
//! Selectors: tag, class, id, attributes, `>`/descendant combinators, `:first-child`,
//! `:last-child`, `:hover`, `:checked`. Properties: display (block/inline/flex/none),
//! position (relative/absolute/fixed + top/right/bottom/left), visibility, opacity,
//! transform (translate/scale), width/height (+ min/max), margin, padding, border,
//! border-radius, background/color, fonts, text-align, flex, gap. `<img>` loads
//! file paths and `data:` URIs in the background.
//!
//! ## Threads
//! * main thread: window + rendering (repaints only on input or DOM change)
//! * `background` thread: runs your function
//! * dispatcher thread: runs event listeners (so a slow handler never freezes the UI)
//!
//! ## Rendering pipeline
//! DOM mutation -> dirty flag + `request_repaint` -> (next frame only) selector-indexed
//! cascade -> immutable render tree with pre-built text layouts -> egui draw.
//! Frames with no DOM change just re-walk the cached tree.

#![forbid(unsafe_code)]

mod app;
mod css;
mod dom;
mod render;
mod runtime;
mod selector;

pub use eframe;

/// Return type of [`spawn`] / [`spawn_with`], re-exported for convenience.
pub type Result<T = ()> = eframe::Result<T>;

pub use runtime::{
    document, spawn, spawn_with, Canvas2D, ClassList, CssStyle, Document, Element, Event, Options,
};
