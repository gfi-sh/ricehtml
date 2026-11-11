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
