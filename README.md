# RiceHTML

A truely lightweight Rust GUI library for HTML, CSS, and Rust apps.

**Warning**: Still a beta! Expect issues.

## Quick start

```rust
use ricehtml::Document;

fn main() -> ricehtml::Result {
    ricehtml::spawn(
        r#"<html><body>
             <button id="clickMe">Click me</button>
             <p id="out"></p>
           </body></html>"#,
        |doc| {
            let out = doc.query_selector("#out");
            doc.query_selector("#clickMe").add_event_listener("click", move |_ev| {
                out.set_text("clicked!");
            });
        },
    )
}
```

`spawn` opens a window on the main thread and runs your closure on a background
thread. All UI work happens through the returned `Document` / `Element`
handles, and event listeners run on their own thread so slow handlers never
freeze the UI.

```toml
[dependencies]
ricehtml = "0.1.1"
```

Full API reference: [API.md](API.md).

## Window options

Use `spawn_with` to customize the window:

```rust
ricehtml::spawn_with(
    html,
    ricehtml::Options { // use a title tag in the head tag to set window title
        size: [1024.0, 768.0],
        ..Default::default()
    },
    setup,
)?;
```

## Building

```bash
cargo build --release
```

Requires Rust 1.88+ and the usual [eframe platform
dependencies](https://github.com/emilk/eframe_template#dependencies) (a GPU or
software renderer; X11/Wayland on Linux).

## License

RiceHTML is licensed under the [GNU General Public License v3.0](LICENSE).
