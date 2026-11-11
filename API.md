# RiceHTML API Reference

Complete reference for the RiceHTML API. See [README.md](README.md) for a
quick start.

## Contents

- [Startup](#startup)
- [Options](#options)
- [Document](#document)
- [Element](#element)
- [ClassList](#classlist)
- [CssStyle](#cssstyle)
- [Event](#event)
- [Canvas2D](#canvas2d)
- [Timers](#timers)
- [Markup reference](#markup-reference)
- [Threading](#threading)

---

## Startup

### `spawn(html, background)`

Parses the HTML, opens a window, and runs `background` on a background thread
with a `Document` handle. Blocks the calling thread until the window closes.

```rust
use ricehtml::Document;

fn main() -> ricehtml::Result {
    ricehtml::spawn(
        r#"<html><body><h1>Hello</h1></body></html>"#,
        |doc: Document| {
            doc.body().set_text_content("Hello from RiceHTML");
        },
    )
}
```

Return type is `ricehtml::Result` (an alias for `eframe::Result`).

### `spawn_with(html, options, background)`

Same as `spawn`, with an `Options` struct:

```rust
ricehtml::spawn_with(
    r#"<html><body>Hi</body></html>"#,
    ricehtml::Options {
        size: [1024.0, 768.0],
        bold_font: Some(include_bytes!("../assets/bold.ttf")),
    },
    |doc| { /* ... */ },
)?;
```

## Options

| Field | Type | Default | Purpose |
| --- | --- | --- | --- |
| `size` | `[f32; 2]` | `[800.0, 600.0]` | Initial window size (width, height). |
| `bold_font` | `Option<&'static [u8]>` | `None` | TTF/OTF font data used for `font-weight: bold` (egui ships no bold face). |

### `document()`

Returns a `Document` handle from anywhere in your program. Panics if called
before `spawn`/`spawn_with`.

```rust
fn update_title() {
    ricehtml::document().set_title("New title".into());
}
```

## Document

A handle to the whole document. `Clone`, `Send + Sync`.

| Method | Description |
| --- | --- |
| `query_selector(sel)` | First element matching `sel` (document-wide). Returns an empty handle if nothing matches. |
| `query_selector_all(sel)` | All matching elements as a `Vec<Element>`. |
| `get_element_by_id(id)` | Shorthand for `query_selector(&format!("#{id}"))`. |
| `body()` | The `<body>` element. |
| `create_element(tag)` | Create a detached element (e.g. `create_element("li")`). Attach it later with `parent.append(&el)`. |
| `add_event_listener(kind, f)` | Listen for events bubbling up from anywhere in the document. |
| `set_title(title)` | Set the window title. |
| `is_open()` | `true` while the window is open. |
| `set_timeout(after, f)` | Run `f` once after the `Duration`. |
| `set_interval(every, f)` | Run `f` every `Duration` until the window closes. |

## Element

A handle to a DOM element. `Clone`, `Send + Sync`. Handles from selectors that
matched nothing are "empty": reads return defaults and writes are no-ops, so
chaining never panics.

### Selection and existence

| Method | Description |
| --- | --- |
| `exists()` | `true` if the handle refers to an element in the document. |
| `query_selector(sel)` | First descendant matching `sel`. |
| `query_selector_all(sel)` | All matching descendants. |
| `tag_name()` | The element's tag name (lowercase). |

### Tree

| Method | Description |
| --- | --- |
| `parent()` | The parent element (empty handle for the root). |
| `children()` | Element children as `Vec<Element>`. |
| `append(&child)` | Make `child` the last child. If `child` is already attached, it is **moved** (not copied). |
| `remove()` | Remove this element and its subtree from the document; drops its event listeners. |
| `append_html(html)` | Parse a fragment and append it as children. |
| `set_inner_html(html)` | Replace all children with parsed markup. |
| `inner_html()` | Children serialized back to HTML. |

### Text

| Method | Description |
| --- | --- |
| `text_content()` / `text()` | Concatenated text of the element and its descendants. |
| `set_text_content(text)` / `set_text(text)` | Replace all children with a single text node. |

### Attributes

| Method | Description |
| --- | --- |
| `attr(name)` | Attribute value, or `None`. |
| `set_attr(name, value)` | Set an attribute. |
| `remove_attr(name)` | Remove an attribute. |

### Classes

| Method | Description |
| --- | --- |
| `has_class(class)` | Whether the class is present. |
| `add_class(class)` | Add one or more classes (space-separated). |
| `remove_class(class)` | Remove one or more classes (space-separated). |
| `toggle_class(class)` | Toggle a class. |
| `class_list()` | `ClassList` handle (see below). |

### Styles

| Method | Description |
| --- | --- |
| `style()` | `CssStyle` handle for the element's inline CSS (see below). |
| `set_style(prop, value)` | Set one inline CSS property (`""` removes it). |

### Input values

| Method | Description |
| --- | --- |
| `value()` | Current text of an `<input>` / `<textarea>`. |
| `set_value(value)` | Set the value programmatically. |
| `checked()` | `true` if the element has a `checked` attribute. |
| `set_checked(bool)` | Set or clear `checked`. |

### Events

| Method | Description |
| --- | --- |
| `add_event_listener(kind, f)` | Register a listener. Supported `kind`s: `"click"`, `"input"`, `"change"`, `"mouseenter"`, `"mouseleave"`, `"keydown"`, `"keyup"`. Pointer events bubble; enter/leave do not. Keyboard events target the focused element (the last clicked input/button/link/…, or `<body>` when nothing is focused) and bubble. Handlers run on the dispatcher thread, never the UI thread. |
| `remove_event_listeners(kind)` | Remove all listeners for `kind` on this element. |

### Canvas

| Method | Description |
| --- | --- |
| `get_context("2d")` | A `Canvas2D` for a `<canvas>` element, or `None` for any other context type. |

## ClassList

`class_list()` returns this. Mirrors the DOM `classList`.

| Method | Description |
| --- | --- |
| `add(class)` | Add classes (space-separated). |
| `remove(class)` | Remove classes (space-separated). |
| `toggle(class)` | Toggle; returns `true` if the class is now present. |
| `contains(class)` | Whether the class is present. |
| `len()` | Number of classes. |
| `is_empty()` | Whether there are no classes. |
| `item(idx)` | The class at `idx`, or `None`. |

`ClassList` also implements `Display` (the class attribute as a string).

```rust
let list = el.class_list();
list.add("active");
if list.contains("active") {
    list.remove("active");
}
```

## CssStyle

`style()` returns this. Operates on the element's inline `style` attribute.

| Method | Description |
| --- | --- |
| `set(prop, value)` | Set a property. An empty value removes it. |
| `set_property(prop, value)` | Alias for `set`. |
| `get(prop)` | Current value of the property, or `None`. |
| `get_property_value(prop)` | Alias for `get`. |
| `remove(prop)` | Remove a property. |
| `css_text()` | The whole inline style string. |
| `set_css_text(css)` | Replace the entire inline style. |

```rust
el.style().set("display", "flex");
el.style().set("gap", "8px");
let gap = el.style().get("gap"); // Some("8px")
el.style().remove("gap");
```

## Event

Passed to event listeners by mutable reference.

| Field | Description |
| --- | --- |
| `kind` | Event name: `"click"`, `"input"`, `"change"`, `"mouseenter"`, `"mouseleave"`, `"keydown"`, `"keyup"`. |
| `target` | Innermost element that was hit. |
| `current_target` | Element whose listener is currently running (differs from `target` while bubbling). |
| `x`, `y` | Pointer position for click events. |
| `value` | `Some(new_text)` for `input` events. |
| `checked` | `Some(new_state)` for `change` events on checkboxes. |
| `key` | The key for `keydown`/`keyup` events, e.g. `"a"`, `"Enter"`, `"ArrowUp"`, `" "`. Letters are lowercase unless Shift is held. |
| `code` | Physical key code, e.g. `"KeyA"`, `"Digit1"`, `"Enter"`. |
| `shift_key` / `ctrl_key` / `alt_key` / `meta_key` | Whether the given modifier was held for a keyboard event. |
| `repeat` | `true` for auto-repeated `keydown` events while a key is held. |

| Method | Description |
| --- | --- |
| `stop_propagation()` | Stop the event from bubbling further. |

```rust
el.add_event_listener("click", |ev| {
    ev.stop_propagation();
    let clicked = ev.target.tag_name();
    println!("clicked a {clicked} at ({}, {})", ev.x, ev.y);
});
```

## Canvas2D

From `el.get_context("2d")`. The backing buffer defaults to 300×150 unless
the `<canvas>` has `width`/`height` attributes. Drawing is immediate: shapes
and paths are rasterized into the buffer.

| Method | Description |
| --- | --- |
| `set_fill_style(css_color)` | Fill color (any CSS color string). |
| `set_stroke_style(css_color)` | Stroke color. |
| `set_line_width(width)` | Stroke width. |
| `set_font_size(size)` | Font size for `fill_text`. |
| `size()` | `(width, height)` of the buffer. |
| `set_size(w, h)` | Resize the buffer (clears it). |
| `begin_path()` | Clear the current path. |
| `close_path()` | Append the first path point (line back to start). |
| `move_to(x, y)` | Start a new path at `(x, y)`. |
| `line_to(x, y)` | Extend the path to `(x, y)`. |
| `fill()` | Fill the current path. **Consumes the path.** |
| `stroke()` | Stroke the current path. **Consumes the path.** |
| `fill_rect(x, y, w, h)` | Filled rectangle. |
| `stroke_rect(x, y, w, h)` | Stroked rectangle. |
| `clear_rect(x, y, w, h)` | Erase a rectangle to transparent. |
| `clear()` | Erase the whole canvas. |
| `fill_circle(cx, cy, r)` | Filled circle. |
| `stroke_circle(cx, cy, r)` | Stroked circle. |
| `draw_line(x1, y1, x2, y2)` | Line using the current stroke. |
| `fill_polygon(&[(f32, f32)])` | Filled polygon (even-odd rule). |
| `fill_text(text, x, y)` | Text with the current font size and fill color. |

```rust
if let Some(c) = canvas.get_context("2d") {
    c.set_fill_style("#e74c3c");
    c.fill_rect(10.0, 10.0, 80.0, 60.0);

    c.set_stroke_style("#2c3e50");
    c.set_line_width(2.0);
    c.begin_path();
    c.move_to(10.0, 200.0);
    c.line_to(100.0, 200.0);
    c.line_to(55.0, 140.0);
    c.close_path();
    c.stroke();

    c.fill_text("hello", 20.0, 120.0);
}
```

## Timers

### `set_timeout(after, f)`

Runs `f` once after `after`, on its own thread.

```rust
doc.set_timeout(std::time::Duration::from_secs(2), || {
    println!("two seconds passed");
});
```

### `set_interval(every, f)`

Runs `f` every `every` until the window closes. The whole callback is one
animation tick: canvas draws inside it are published atomically at the end,
so the UI never shows a half-drawn frame.

```rust
doc.set_interval(std::time::Duration::from_millis(16), || {
    draw_frame(&canvas); // canvas ops
});
```

---

## Markup reference

### HTML elements

Elements, text, attributes, comments, `<style>` blocks, and escaped text.
Supported tags:

- Layout: `<html>`, `<head>`, `<body>`, `<div>`, `<span>`, headers `<h1>`–`<h6>`, `<p>`, lists `<ul>`/`<ol>`/`<li>`, `<hr>`, `<br>`
- Forms: `<input>`, `<textarea>`, `<button>`, `<label>`, `<a>`
- Media: `<img>`, `<canvas>`
- Other: `<title>`, `<style>`, `<b>`/`<strong>`, `<i>`/`<em>`, `<u>`, `<s>`, `<code>`, `<pre>`

Unsupported tags are ignored.

### CSS selectors

- Tag, `.class`, `#id`, `*`
- Attribute selectors: `[attr]`, `[attr=value]`, `[attr*=value]`, `[attr^=value]`, `[attr$=value]`, `[attr~=value]`
- Combinators: `>` (child), descendant (whitespace)
- Pseudo-classes: `:first-child`, `:last-child`, `:hover`, `:checked`
- Comma-separated lists: `h1, h2 { ... }`

### CSS properties

- **Display & layout**: `display` (block, inline, flex, none), `width`, `height`, `min-width`, `min-height`, `max-width`, `max-height` (px or %), `flex-direction` (row, column, and `*-reverse`), `flex-wrap`, `gap`, `justify-content` (flex-start, flex-end, center, space-between, space-around, space-evenly), `align-items` (stretch, flex-start, flex-end, center)
- **Box**: `margin` / `margin-top|right|bottom|left` (including `auto`), `padding` / `padding-top|right|bottom|left`, `border`, `border-width`, `border-color`, `border-style`, `border-radius`
- **Positioning**: `position` (relative, absolute, fixed), `top`, `right`, `bottom`, `left`
- **Appearance**: `background`, `background-color`, `color`, `visibility`, `opacity`, `transform` (`translate()`, `scale()`)
- **Typography**: `font-size`, `font-weight`, `font-style`, `font-family` (monospace families), `text-align`, `text-decoration`, `white-space`

### Events

`click`, `input`, `change` bubble up the tree; `mouseenter`, `mouseleave`
do not. `keydown`, `keyup` target the focused element (the last clicked
input/button/link/…, or `<body>` when nothing is focused) and bubble, so
listeners on the document receive all keyboard events, for example:

```rust
document.add_event_listener("keydown", |ev| {
    if ev.key == "Enter" {
        println!("Enter pressed (ctrl={})", ev.ctrl_key);
    }
});
```

---

## Threading

| Thread | Work |
| --- | --- |
| main | window, input, layout, painting |
| background | your `spawn`/`spawn_with` closure |
| dispatcher | event listeners |
| timer | `set_timeout` / `set_interval` callbacks |

All handles are `Send + Sync`; the library holds no global mutable state beyond
the document handle registered by `document()`. The crate contains no
`unsafe` code (`#![forbid(unsafe_code)]`).
