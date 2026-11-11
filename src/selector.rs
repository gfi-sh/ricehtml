use crate::dom::{Dom, Kind, NodeId};
#[allow(unused_imports)]
use crate::dom::ROOT;

#[derive(Clone, Copy, Debug)]
pub enum Comb { Descendant, Child }

#[derive(Clone, Debug)]
enum AttrOp { Exists, Eq, Contains, Prefix, Suffix, Word }

#[derive(Clone, Debug, Default)]
pub struct Compound {
    tag: Option<String>,
    id: Option<String>,
    classes: Vec<String>,
    attrs: Vec<(String, AttrOp, String)>,
    first_child: bool,
    last_child: bool,
    hover: bool,   // :hover — matches when the pointer is over this element
    checked: bool, // :checked — matches when the `checked` attribute is present
    never: bool,   // unsupported pseudo-class (e.g. :focus, ::before)
}

#[derive(Clone, Debug)]
pub struct Complex { parts: Vec<(Comb, Compound)> }

pub enum Key { Id(String), Class(String), Tag(String), Any }

impl Compound {
    fn matches(&self, dom: &Dom, n: NodeId, hovered: &dyn Fn(NodeId) -> bool) -> bool {
        let Kind::Element { tag, .. } = &dom.node(n).kind else { return false };
        if self.never { return false; }
        if self.hover && !hovered(n) { return false; }
        if self.checked && dom.attr(n, "checked").is_none() { return false; }
        if let Some(t) = &self.tag { if t != tag { return false; } }
        if let Some(i) = &self.id { if dom.attr(n, "id") != Some(i.as_str()) { return false; } }
        if !self.classes.is_empty() {
            let Some(c) = dom.attr(n, "class") else { return false };
            if !self.classes.iter().all(|k| c.split_whitespace().any(|w| w == k)) { return false; }
        }
        for (name, op, val) in &self.attrs {
            let Some(v) = dom.attr(n, name) else { return false };
            let ok = match op {
                AttrOp::Exists => true,
                AttrOp::Eq => v == val,
                AttrOp::Contains => v.contains(val.as_str()),
                AttrOp::Prefix => v.starts_with(val.as_str()),
                AttrOp::Suffix => v.ends_with(val.as_str()),
                AttrOp::Word => v.split_whitespace().any(|w| w == val),
            };
            if !ok { return false; }
        }
        if self.first_child || self.last_child {
            let Some(p) = dom.parent(n) else { return false };
            let mut it = dom.element_children(p);
            if self.first_child && it.next() != Some(n) { return false; }
            if self.last_child && dom.element_children(p).last() != Some(n) { return false; }
        }
        true
    }
}

impl Complex {
    pub fn matches(&self, dom: &Dom, n: NodeId) -> bool { self.matches_dyn(dom, n, &|_| false) }

    /// `hovered` reports which node the pointer is currently over (for `:hover`).
    pub fn matches_dyn(&self, dom: &Dom, n: NodeId, hovered: &dyn Fn(NodeId) -> bool) -> bool {
        self.match_from(dom, self.parts.len() - 1, n, hovered)
    }

    fn match_from(&self, dom: &Dom, i: usize, n: NodeId, hovered: &dyn Fn(NodeId) -> bool) -> bool {
        let (comb, comp) = &self.parts[i];
        if !comp.matches(dom, n, hovered) { return false; }
        if i == 0 { return true; }
        match comb {
            Comb::Child => dom.parent(n).map_or(false, |p| self.match_from(dom, i - 1, p, hovered)),
            Comb::Descendant => {
                let mut p = dom.parent(n);
                while let Some(pp) = p {
                    if self.match_from(dom, i - 1, pp, hovered) { return true; }
                    p = dom.parent(pp);
                }
                false
            }
        }
    }

    pub fn specificity(&self) -> u32 {
        self.parts.iter().map(|(_, c)| {
            c.id.is_some() as u32 * 10_000
                + (c.classes.len() + c.attrs.len() + c.first_child as usize + c.last_child as usize
                    + c.hover as usize + c.checked as usize) as u32 * 100
                + c.tag.is_some() as u32
        }).sum()
    }

    /// Rightmost compound's most selective key, used to bucket rules.
    pub fn key(&self) -> Key {
        let Some(c) = self.parts.last().map(|(_, c)| c) else { return Key::Any };
        if c.never { return Key::Tag("\u{0}never".into()); }
        if let Some(i) = &c.id { return Key::Id(i.clone()); }
        if let Some(k) = c.classes.first() { return Key::Class(k.clone()); }
        if let Some(t) = &c.tag { return Key::Tag(t.clone()); }
        Key::Any
    }
}

pub fn parse_list(s: &str) -> Vec<Complex> {
    s.split(',').filter_map(|p| parse_complex(p.trim())).collect()
}

fn parse_complex(s: &str) -> Option<Complex> {
    let ch: Vec<char> = s.chars().collect();
    let (mut i, mut comb, mut parts) = (0, Comb::Descendant, Vec::new());
    while i < ch.len() {
        match ch[i] {
            c if c.is_whitespace() => i += 1,
            '>' => { comb = Comb::Child; i += 1; }
            '+' | '~' => return None,
            _ => {
                let (c, ni) = parse_compound(&ch, i)?;
                i = ni;
                parts.push((comb, c));
                comb = Comb::Descendant;
            }
        }
    }
    if parts.is_empty() { None } else { Some(Complex { parts }) }
}

fn ident(ch: &[char], mut i: usize) -> (String, usize) {
    let s = i;
    while i < ch.len() && (ch[i].is_alphanumeric() || ch[i] == '-' || ch[i] == '_') { i += 1; }
    (ch[s..i].iter().collect(), i)
}

fn parse_compound(ch: &[char], mut i: usize) -> Option<(Compound, usize)> {
    let start = i;
    let mut c = Compound::default();
    while i < ch.len() && !ch[i].is_whitespace() && ch[i] != '>' {
        match ch[i] {
            '*' => i += 1,
            '#' => { let (s, ni) = ident(ch, i + 1); c.id = Some(s); i = ni; }
            '.' => { let (s, ni) = ident(ch, i + 1); c.classes.push(s); i = ni; }
            '[' => {
                let end = ch[i..].iter().position(|&x| x == ']')? + i;
                let body: String = ch[i + 1..end].iter().collect();
                c.attrs.push(parse_attr(&body)?);
                i = end + 1;
            }
            ':' => {
                i += 1;
                if i < ch.len() && ch[i] == ':' { i += 1; }
                let (name, ni) = ident(ch, i);
                i = ni;
                if i < ch.len() && ch[i] == '(' {
                    while i < ch.len() && ch[i] != ')' { i += 1; }
                    i += 1;
                    c.never = true;
                } else {
                    match name.as_str() {
                        "first-child" => c.first_child = true,
                        "last-child" => c.last_child = true,
                        "hover" => c.hover = true,
                        "checked" => c.checked = true,
                        _ => c.never = true,
                    }
                }
            }
            x if x.is_alphabetic() => { let (s, ni) = ident(ch, i); c.tag = Some(s.to_ascii_lowercase()); i = ni; }
            _ => return None,
        }
    }
    if i == start { None } else { Some((c, i)) }
}

fn parse_attr(b: &str) -> Option<(String, AttrOp, String)> {
    for (tok, op) in [("*=", AttrOp::Contains), ("^=", AttrOp::Prefix), ("$=", AttrOp::Suffix), ("~=", AttrOp::Word), ("=", AttrOp::Eq)] {
        if let Some((k, v)) = b.split_once(tok) {
            return Some((k.trim().to_ascii_lowercase(), op, v.trim().trim_matches(|c| c == '"' || c == '\'').to_string()));
        }
    }
    Some((b.trim().to_ascii_lowercase(), AttrOp::Exists, String::new()))
}
