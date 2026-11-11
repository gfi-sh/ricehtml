use crate::selector::Complex;

pub type NodeId = usize;
pub const ROOT: NodeId = 0;

#[derive(Debug, Clone)]
pub enum Kind {
    Element { tag: String, attrs: Vec<(String, String)> },
    Text(String),
}

#[derive(Debug, Clone)]
pub struct Node {
    pub kind: Kind,
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
}

/// Arena DOM. Nodes are addressed by index; text nodes are recycled so
/// frequent `set_text` calls don't grow memory.
#[derive(Debug, Clone)]
pub struct Dom {
    nodes: Vec<Node>,
    free_text: Vec<NodeId>,
}

pub fn is_ws(c: char) -> bool { c.is_whitespace() && c != '\u{a0}' }

impl Dom {
    pub fn parse(html: &str) -> Dom {
        let mut d = Dom {
            nodes: vec![Node { kind: Kind::Element { tag: "#document".into(), attrs: vec![] }, parent: None, children: vec![] }],
            free_text: vec![],
        };
        d.append_html(ROOT, html);
        d
    }

    pub fn append_html(&mut self, parent: NodeId, html: &str) {
        let src = escape_raw_blocks(html);
        if let Ok(vdom) = tl::parse(&src, tl::ParserOptions::default()) {
            let p = vdom.parser();
            for h in vdom.children() {
                if let Some(n) = h.get(p) { self.import(n, p, parent); }
            }
        };
    }

    fn import(&mut self, n: &tl::Node, p: &tl::Parser, parent: NodeId) {
        match n {
            tl::Node::Tag(t) => {
                let tag = t.name().as_utf8_str().to_ascii_lowercase();
                let attrs = t.attributes().iter()
                    .map(|(k, v)| (k.to_ascii_lowercase(), v.map(|v| decode(&v)).unwrap_or_default()))
                    .collect();
                let id = self.push(Kind::Element { tag, attrs }, parent);
                for h in t.children().top().iter() {
                    if let Some(c) = h.get(p) { self.import(c, p, id); }
                }
            }
            tl::Node::Raw(b) => {
                let s = decode(&b.as_utf8_str());
                if !s.is_empty() { self.push(Kind::Text(s), parent); }
            }
            tl::Node::Comment(_) => {}
        }
    }

    fn push(&mut self, kind: Kind, parent: NodeId) -> NodeId {
        let reuse = if matches!(kind, Kind::Text(_)) { self.free_text.pop() } else { None };
        let node = Node { kind, parent: Some(parent), children: vec![] };
        let id = match reuse {
            Some(f) => { self.nodes[f] = node; f }
            None => { self.nodes.push(node); self.nodes.len() - 1 }
        };
        self.nodes[parent].children.push(id);
        id
    }

    // ---- reading -------------------------------------------------------
    pub fn node(&self, id: NodeId) -> &Node { &self.nodes[id] }
    pub fn parent(&self, id: NodeId) -> Option<NodeId> { self.nodes[id].parent }
    pub fn children(&self, id: NodeId) -> &[NodeId] { &self.nodes[id].children }

    pub fn element_children(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        self.nodes[id].children.iter().copied().filter(move |&c| matches!(self.nodes[c].kind, Kind::Element { .. }))
    }

    pub fn tag(&self, id: NodeId) -> Option<&str> {
        match &self.nodes[id].kind { Kind::Element { tag, .. } => Some(tag), _ => None }
    }

    /// Create a detached element node (not linked into the tree until `adopt`).
    pub fn new_element(&mut self, tag: &str) -> NodeId {
        self.nodes.push(Node {
            kind: Kind::Element { tag: tag.to_ascii_lowercase(), attrs: vec![] },
            parent: None,
            children: vec![],
        });
        self.nodes.len() - 1
    }

    /// Link `child` under `parent`, detaching it from any previous parent first
    /// (like `Node.appendChild`, which moves rather than copies).
    /// Refuses to create cycles (child is an ancestor of parent).
    pub fn adopt(&mut self, child: NodeId, parent: NodeId) {
        if child == parent || child == ROOT { return; }
        // cycle guard: parent must not be a descendant of child
        let mut p = self.nodes[parent].parent;
        while let Some(pp) = p {
            if pp == child { return; }
            p = self.nodes[pp].parent;
        }
        if let Some(old) = self.nodes[child].parent {
            self.nodes[old].children.retain(|&c| c != child);
        }
        self.nodes[child].parent = Some(parent);
        self.nodes[parent].children.push(child);
    }

    pub fn attr(&self, id: NodeId, name: &str) -> Option<&str> {
        let name = name.to_ascii_lowercase();
        match &self.nodes[id].kind {
            Kind::Element { attrs, .. } => attrs.iter().find(|(k, _)| k == &name).map(|(_, v)| v.as_str()),
            _ => None,
        }
    }

    pub fn text_content(&self, id: NodeId) -> String {
        let mut s = String::new();
        self.collect_text(id, &mut s);
        s
    }
    fn collect_text(&self, id: NodeId, out: &mut String) {
        match &self.nodes[id].kind {
            Kind::Text(t) => out.push_str(t),
            _ => for &c in &self.nodes[id].children { self.collect_text(c, out) },
        }
    }

    pub fn descendants(&self, id: NodeId) -> Vec<NodeId> {
        let mut out = vec![];
        let mut stack: Vec<NodeId> = self.nodes[id].children.iter().rev().copied().collect();
        while let Some(n) = stack.pop() {
            out.push(n);
            stack.extend(self.nodes[n].children.iter().rev());
        }
        out
    }

    pub fn find_tag(&self, from: NodeId, name: &str) -> Option<NodeId> {
        self.descendants(from).into_iter().find(|&n| self.tag(n) == Some(name))
    }

    pub fn query(&self, from: NodeId, sels: &[Complex], all: bool) -> Vec<NodeId> {
        let mut out = vec![];
        let mut stack: Vec<NodeId> = self.nodes[from].children.iter().rev().copied().collect();
        while let Some(n) = stack.pop() {
            if matches!(self.nodes[n].kind, Kind::Element { .. }) && sels.iter().any(|s| s.matches(self, n)) {
                out.push(n);
                if !all { break; }
            }
            stack.extend(self.nodes[n].children.iter().rev());
        }
        out
    }

    // ---- mutation ------------------------------------------------------
    pub fn set_attr(&mut self, id: NodeId, name: &str, val: &str) {
        let name = name.to_ascii_lowercase();
        if let Kind::Element { attrs, .. } = &mut self.nodes[id].kind {
            match attrs.iter_mut().find(|(k, _)| *k == name) {
                Some((_, v)) => { if v != val { *v = val.to_string(); } }
                None => attrs.push((name, val.to_string())),
            }
        }
    }

    pub fn remove_attr(&mut self, id: NodeId, name: &str) {
        let name = name.to_ascii_lowercase();
        if let Kind::Element { attrs, .. } = &mut self.nodes[id].kind { attrs.retain(|(k, _)| *k != name); }
    }

    pub fn set_style_prop(&mut self, id: NodeId, prop: &str, val: &str) {
        let cur = self.attr(id, "style").unwrap_or("").to_string();
        let mut decls: Vec<(String, String)> = cur.split(';').filter_map(|d| {
            let (k, v) = d.split_once(':')?;
            Some((k.trim().to_string(), v.trim().to_string()))
        }).collect();
        decls.retain(|(k, _)| !k.eq_ignore_ascii_case(prop));
        if !val.is_empty() { decls.push((prop.to_ascii_lowercase(), val.to_string())); }
        let s = decls.iter().map(|(k, v)| format!("{k}:{v}")).collect::<Vec<_>>().join(";");
        self.set_attr(id, "style", &s);
    }

    pub fn set_text(&mut self, id: NodeId, text: &str) {
        if self.nodes[id].children.len() == 1 {
            let only = self.nodes[id].children[0];
            if let Kind::Text(t) = &mut self.nodes[only].kind {
                if t != text { *t = text.to_string(); }
                return;
            }
        }
        self.clear_children(id);
        self.push(Kind::Text(text.to_string()), id);
    }

    pub fn set_inner_html(&mut self, id: NodeId, html: &str) {
        self.clear_children(id);
        self.append_html(id, html);
    }

    pub fn clear_children(&mut self, id: NodeId) {
        for k in std::mem::take(&mut self.nodes[id].children) { self.release(k); }
    }

    pub fn remove(&mut self, id: NodeId) {
        if id == ROOT { return; }
        if let Some(p) = self.nodes[id].parent {
            self.nodes[p].children.retain(|&c| c != id);
        }
        self.release(id);
    }

    fn release(&mut self, id: NodeId) {
        self.nodes[id].parent = None;
        for c in std::mem::take(&mut self.nodes[id].children) { self.release(c); }
        if let Kind::Text(t) = &mut self.nodes[id].kind {
            t.clear();
            self.free_text.push(id);
        }
    }
}

/// `<style>`/`<script>` bodies may contain `<`/`>` (e.g. `ul > li`); escape them
/// so the HTML parser sees plain text. `decode` restores them.
fn escape_raw_blocks(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let mut out = String::with_capacity(html.len());
    let mut i = 0;
    while i < html.len() {
        let rest = &lower[i..];
        // Match only real tag starts: `<style` followed by whitespace, `/` or `>`
        let mut next: Option<(usize, &str)> = None;
        for t in ["<style", "<script"] {
            let mut from = 0;
            while let Some(p) = rest[from..].find(t) {
                let at = from + p;
                let after = rest[at + t.len()..].chars().next();
                if after.map_or(false, |c| c.is_whitespace() || c == '>' || c == '/') {
                    if next.map_or(true, |(bp, _)| at < bp) { next = Some((at, t)); }
                    break;
                }
                from = at + t.len();
            }
        }
        let Some((p, tag)) = next else { out.push_str(&html[i..]); break };
        let open = i + p;
        let Some(gt) = lower[open..].find('>') else { out.push_str(&html[i..]); break };
        let body_start = open + gt + 1;
        out.push_str(&html[i..body_start]);
        let close = format!("</{}", &tag[1..]);
        let body_end = lower[body_start..].find(&close).map(|q| body_start + q).unwrap_or(html.len());
        for ch in html[body_start..body_end].chars() {
            match ch { '&' => out.push_str("&amp;"), '<' => out.push_str("&lt;"), '>' => out.push_str("&gt;"), c => out.push(c) }
        }
        i = body_end;
    }
    out
}

pub fn decode(s: &str) -> String {    if !s.contains('&') { return s.to_string(); }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(p) = rest.find('&') {
        out.push_str(&rest[..p]);
        rest = &rest[p..];
        let end = rest.find(';').filter(|&e| e <= 10);
        let rep = end.and_then(|e| {
            let name = &rest[1..e];
            let c = match name {
                "amp" => '&', "lt" => '<', "gt" => '>', "quot" => '"', "apos" => '\'',
                "nbsp" => '\u{a0}', "copy" => '©', "ndash" => '–', "mdash" => '—', "hellip" => '…',
                n if n.starts_with("#x") || n.starts_with("#X") => char::from_u32(u32::from_str_radix(&n[2..], 16).ok()?)?,
                n if n.starts_with('#') => char::from_u32(n[1..].parse().ok()?)?,
                _ => return None,
            };
            Some((c, e))
        });
        match rep {
            Some((c, e)) => { out.push(c); rest = &rest[e + 1..]; }
            None => { out.push('&'); rest = &rest[1..]; }
        }
    }
    out.push_str(rest);
    out
}

/// Serialize a node's children back to HTML (used by `Element::inner_html()`).
pub fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
    out
}

impl Dom {
    pub fn serialize_children(&self, id: NodeId, out: &mut String) {
        for &c in &self.nodes[id].children {
            self.serialize_node(c, out);
        }
    }

    fn serialize_node(&self, id: NodeId, out: &mut String) {
        match &self.nodes[id].kind {
            Kind::Text(t) => out.push_str(&encode(t)),
            Kind::Element { tag, attrs } => {
                out.push('<');
                out.push_str(tag);
                for (k, v) in attrs {
                    out.push(' ');
                    out.push_str(k);
                    out.push_str("=\"");
                    out.push_str(&encode(v));
                    out.push('"');
                }
                out.push('>');
                self.serialize_children(id, out);
                out.push_str("</");
                out.push_str(tag);
                out.push('>');
            }
        }
    }
}
