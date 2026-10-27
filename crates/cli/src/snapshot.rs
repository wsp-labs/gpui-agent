//! Turns a frame record into the outline an agent reads, with a ref for each line.

use gpui_agent_protocol::{RawNode, RawTree, Rect};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// Identifies an element across frames: its id path, where it was built, its type, and its
/// position among elements that share all three.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Key {
    pub path: String,
    pub source: Option<String>,
    #[serde(rename = "type")]
    pub type_name: String,
    pub ordinal: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    #[serde(rename = "ref")]
    pub reference: String,
    pub depth: usize,
    pub role: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub states: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub visible: Rect,
    pub key: Key,
    #[serde(skip)]
    pub node: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub window: u64,
    pub title: String,
    pub width: f32,
    pub height: f32,
    pub prompt: bool,
    pub entries: Vec<Entry>,
}

#[derive(Debug, Clone, Default)]
pub struct Options {
    pub interactive: bool,
    pub all: bool,
    pub depth: Option<usize>,
    pub scope: Option<Key>,
}

/// Facts about a frame record that every command needs.
pub struct Frame<'a> {
    pub tree: &'a RawTree,
    pub children: Vec<Vec<usize>>,
    pub roots: Vec<usize>,
    pub keys: Vec<Key>,
    texts: Vec<Vec<usize>>,
    interactive_below: Vec<bool>,
    text_inputs: HashSet<&'a str>,
}

impl<'a> Frame<'a> {
    pub fn new(tree: &'a RawTree) -> Self {
        let n = tree.nodes.len();
        let mut children = vec![Vec::new(); n];
        let mut roots = Vec::new();
        let mut depth = vec![0; n];
        for (i, node) in tree.nodes.iter().enumerate() {
            match node.parent {
                Some(parent) if parent < i => {
                    children[parent].push(i);
                    depth[i] = depth[parent] + 1;
                }
                _ => roots.push(i),
            }
        }

        let mut paths = vec![String::new(); n];
        for (i, node) in tree.nodes.iter().enumerate() {
            let parent_path = node.parent.filter(|p| *p < i).map(|p| paths[p].clone());
            paths[i] = match (&node.id, parent_path) {
                (Some(id), Some(parent)) if !parent.is_empty() => format!("{parent}/{id}"),
                (Some(id), _) => id.clone(),
                (None, Some(parent)) => parent,
                (None, None) => String::new(),
            };
        }
        let mut seen: HashMap<(String, Option<String>, String), usize> = HashMap::new();
        let keys = tree
            .nodes
            .iter()
            .enumerate()
            .map(|(i, node)| {
                let base = (
                    paths[i].clone(),
                    node.source.clone(),
                    node.type_name.clone(),
                );
                let ordinal = seen.entry(base.clone()).or_insert(0);
                let key = Key {
                    path: base.0,
                    source: base.1,
                    type_name: base.2,
                    ordinal: *ordinal,
                };
                *ordinal += 1;
                key
            })
            .collect();

        let mut texts = vec![Vec::new(); n];
        for (t, text) in tree.texts.iter().enumerate() {
            if text.text.trim().is_empty() {
                continue;
            }
            let owner = tree
                .nodes
                .iter()
                .enumerate()
                .filter(|(_, node)| node.bounds.contains_rect(&text.bounds))
                .max_by_key(|(i, _)| (depth[*i], *i))
                .map(|(i, _)| i)
                .or_else(|| roots.first().copied());
            if let Some(owner) = owner {
                texts[owner].push(t);
            }
        }

        let mut interactive_below = vec![false; n];
        for i in (0..n).rev() {
            if let Some(parent) = tree.nodes[i].parent.filter(|p| *p < i)
                && (interactive_below[i] || tree.nodes[i].flags.interactive())
            {
                interactive_below[parent] = true;
            }
        }

        Frame {
            tree,
            children,
            roots,
            keys,
            texts,
            interactive_below,
            text_inputs: tree.text_inputs.iter().map(String::as_str).collect(),
        }
    }

    pub fn find(&self, key: &Key) -> Option<usize> {
        self.keys.iter().position(|candidate| candidate == key)
    }

    pub fn is_text_input(&self, i: usize) -> bool {
        self.tree.nodes[i]
            .focus
            .as_deref()
            .is_some_and(|focus| self.text_inputs.contains(focus))
    }

    pub fn is_ancestor(&self, ancestor: usize, mut node: usize) -> bool {
        while let Some(parent) = self.tree.nodes[node].parent {
            if parent == ancestor {
                return true;
            }
            if parent >= node {
                return false;
            }
            node = parent;
        }
        false
    }

    /// The element that receives the mouse at a point: the last drawn element there that
    /// listens for the mouse or blocks it.
    pub fn hit(&self, x: f32, y: f32) -> Option<usize> {
        self.tree
            .nodes
            .iter()
            .enumerate()
            .rev()
            .find_map(|(i, node)| {
                (node.visible.contains(x, y) && (node.flags.pointer() || node.flags.blocks))
                    .then_some(i)
            })
    }

    pub fn nearest_scroll_ancestor(&self, mut node: usize) -> Option<usize> {
        while let Some(parent) = self.tree.nodes[node].parent {
            if parent >= node {
                return None;
            }
            if self.tree.nodes[parent].flags.scroll {
                return Some(parent);
            }
            node = parent;
        }
        None
    }

    /// All text in a subtree, in reading order.
    pub fn text_below(&self, i: usize, all: bool) -> String {
        let mut found = Vec::new();
        let mut stack = vec![i];
        while let Some(node) = stack.pop() {
            for &t in &self.texts[node] {
                let text = &self.tree.texts[t];
                if all || !text.visible.is_empty() {
                    found.push(text);
                }
            }
            stack.extend(self.children[node].iter().copied());
        }
        found.sort_by(|a, b| {
            (a.bounds.y, a.bounds.x)
                .partial_cmp(&(b.bounds.y, b.bounds.x))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let joined = found
            .iter()
            .map(|text| text.text.trim())
            .collect::<Vec<_>>()
            .join(" ");
        joined.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// What a line calls an element: the text inside a leaf, or a container's id or key context.
    pub fn name(&self, i: usize) -> String {
        let node = &self.tree.nodes[i];
        if self.interactive_below[i] {
            return node
                .id
                .clone()
                .or_else(|| node.key_context.clone())
                .unwrap_or_default();
        }
        let text = self.text_below(i, true);
        if text.is_empty() {
            node.id.clone().unwrap_or_default()
        } else {
            text
        }
    }

    fn own_text(&self, i: usize, all: bool) -> String {
        self.texts[i]
            .iter()
            .map(|&t| &self.tree.texts[t])
            .filter(|text| all || !text.visible.is_empty())
            .map(|text| text.text.trim())
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub fn role(&self, i: usize) -> String {
        let node = &self.tree.nodes[i];
        if self.is_text_input(i) {
            return "textbox".into();
        }
        if let Some(role) = &node.role {
            return role.to_lowercase();
        }
        let flags = &node.flags;
        if flags.click {
            "button"
        } else if flags.scroll {
            "scroll"
        } else if flags.drag {
            "draggable"
        } else if flags.drop {
            "droptarget"
        } else if flags.focusable || flags.mouse || flags.keys {
            "group"
        } else if flags.blocks {
            "overlay"
        } else {
            "text"
        }
        .into()
    }

    fn states(&self, i: usize) -> Vec<String> {
        let node = &self.tree.nodes[i];
        let mut states = Vec::new();
        if node.flags.focused {
            states.push("focused".into());
        }
        if node.visible.area() + 0.5 < node.bounds.area() {
            states.push("clipped".into());
        }
        states
    }

    fn structural(node: &RawNode) -> bool {
        node.type_name.contains("AnyView") || node.type_name.ends_with("View>")
    }
}

pub fn build(tree: &RawTree, options: &Options) -> Snapshot {
    let frame = Frame::new(tree);
    let mut entries = Vec::new();
    let starts: Vec<usize> = match &options.scope {
        Some(key) => frame.find(key).into_iter().collect(),
        None => frame.roots.clone(),
    };
    for start in starts {
        emit(&frame, options, start, 0, &mut entries);
    }
    for (n, entry) in entries.iter_mut().enumerate() {
        entry.reference = format!("e{}", n + 1);
    }
    Snapshot {
        window: tree.window,
        title: tree.title.clone(),
        width: tree.width,
        height: tree.height,
        prompt: tree.prompt,
        entries,
    }
}

fn emit(frame: &Frame, options: &Options, i: usize, depth: usize, out: &mut Vec<Entry>) {
    let node = &frame.tree.nodes[i];
    let visible = options.all || !node.visible.is_empty();
    let within_depth = options.depth.is_none_or(|limit| depth <= limit);
    let interactive = node.flags.interactive() || frame.is_text_input(i);
    let leaf = interactive && !frame.interactive_below[i];

    let push = |role: String, name: String, out: &mut Vec<Entry>| {
        out.push(Entry {
            reference: String::new(),
            depth,
            role,
            name,
            states: frame.states(i),
            context: node.key_context.clone(),
            source: node.source.clone(),
            visible: node.visible,
            key: frame.keys[i].clone(),
            node: i,
        });
    };

    if leaf {
        if visible && within_depth {
            let mut name = frame.text_below(i, options.all);
            if name.is_empty() {
                name = node.id.clone().unwrap_or_default();
            }
            push(frame.role(i), name, out);
        }
        return;
    }

    let mut child_depth = depth;
    if (interactive || node.role.is_some()) && !Frame::structural(node) {
        if visible && within_depth {
            push(frame.role(i), node.id.clone().unwrap_or_default(), out);
        }
        child_depth = depth + 1;
    } else if !options.interactive {
        let text = frame.own_text(i, options.all);
        if !text.is_empty() && visible && within_depth {
            push("text".into(), text, out);
        }
    }
    for &child in &frame.children[i] {
        emit(frame, options, child, child_depth, out);
    }
}

pub fn render(snapshot: &Snapshot, show_source: bool) -> String {
    let mut out = format!(
        "window {} \"{}\" {}x{}{}\n",
        snapshot.window,
        snapshot.title,
        snapshot.width,
        snapshot.height,
        if snapshot.prompt {
            " [prompt open]"
        } else {
            ""
        }
    );
    for entry in &snapshot.entries {
        out.push_str(&"  ".repeat(entry.depth + 1));
        out.push('@');
        out.push_str(&entry.reference);
        out.push(' ');
        out.push_str(&entry.role);
        if !entry.name.is_empty() {
            out.push_str(&format!(" {:?}", truncate(&entry.name, 100)));
        }
        if let Some(context) = &entry.context {
            out.push_str(&format!(" ctx={context}"));
        }
        for state in &entry.states {
            out.push_str(&format!(" [{state}]"));
        }
        if show_source && let Some(source) = &entry.source {
            out.push_str("  ");
            out.push_str(source);
        }
        out.push('\n');
    }
    out
}

fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        text.to_string()
    } else {
        let mut cut: String = text.chars().take(limit).collect();
        cut.push_str("...");
        cut
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_agent_protocol::{Flags, RawText};

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }

    fn node(parent: Option<usize>, bounds: Rect, id: Option<&str>, flags: Flags) -> RawNode {
        RawNode {
            parent,
            bounds,
            visible: bounds,
            id: id.map(Into::into),
            type_name: "gpui::Div".into(),
            source: Some("app.rs:1".into()),
            role: None,
            flags,
            key_context: None,
            focus: None,
        }
    }

    fn text(bounds: Rect, text: &str) -> RawText {
        RawText {
            bounds,
            visible: bounds,
            text: text.into(),
        }
    }

    fn sample() -> RawTree {
        let click = Flags {
            click: true,
            ..Default::default()
        };
        let scroll = Flags {
            scroll: true,
            ..Default::default()
        };
        let mut clipped = node(Some(3), rect(0., 100., 100., 20.), Some("row-2"), click);
        clipped.visible = rect(0., 100., 0., 0.);
        let mut field = node(
            Some(0),
            rect(0., 140., 200., 30.),
            None,
            Flags {
                focusable: true,
                mouse: true,
                ..Default::default()
            },
        );
        field.focus = Some("FocusId(1)".into());
        RawTree {
            window: 1,
            title: "t".into(),
            width: 400.,
            height: 300.,
            scale: 1.,
            nodes: vec![
                node(None, rect(0., 0., 400., 300.), None, Flags::default()),
                node(Some(0), rect(0., 0., 100., 30.), Some("inc"), click),
                node(Some(0), rect(0., 40., 200., 20.), None, Flags::default()),
                node(Some(0), rect(0., 60., 100., 40.), Some("rows"), scroll),
                node(Some(3), rect(0., 60., 100., 20.), Some("row-1"), click),
                clipped,
                field,
            ],
            texts: vec![
                text(rect(0., 0., 60., 20.), "Increment"),
                text(rect(0., 40., 60., 20.), "count: 0"),
                text(rect(0., 60., 40., 20.), "row 1"),
                text(rect(0., 100., 40., 20.), "row 2"),
                text(rect(0., 140., 80., 20.), "Your name"),
            ],
            text_inputs: vec!["FocusId(1)".into()],
            prompt: false,
        }
    }

    #[test]
    fn outline_names_buttons_by_their_text_and_skips_clipped_rows() {
        let snapshot = build(&sample(), &Options::default());
        let lines: Vec<(String, String, usize)> = snapshot
            .entries
            .iter()
            .map(|e| (e.role.clone(), e.name.clone(), e.depth))
            .collect();
        assert_eq!(
            lines,
            vec![
                ("button".into(), "Increment".into(), 0),
                ("text".into(), "count: 0".into(), 0),
                ("scroll".into(), "rows".into(), 0),
                ("button".into(), "row 1".into(), 1),
                ("textbox".into(), "Your name".into(), 0),
            ]
        );
        assert_eq!(snapshot.entries[0].reference, "e1");
    }

    #[test]
    fn interactive_only_drops_plain_text() {
        let options = Options {
            interactive: true,
            ..Default::default()
        };
        let snapshot = build(&sample(), &options);
        assert!(snapshot.entries.iter().all(|e| e.role != "text"));
        assert_eq!(snapshot.entries.len(), 4);
    }

    #[test]
    fn all_includes_clipped_rows() {
        let options = Options {
            all: true,
            ..Default::default()
        };
        let snapshot = build(&sample(), &options);
        assert!(snapshot.entries.iter().any(|e| e.name == "row 2"));
    }

    #[test]
    fn keys_survive_a_text_change() {
        let before = sample();
        let mut after = sample();
        after.texts[1].text = "count: 1".into();
        let a = Frame::new(&before);
        let b = Frame::new(&after);
        assert_eq!(a.keys, b.keys);
    }

    #[test]
    fn hit_finds_the_topmost_blocking_element() {
        let mut tree = sample();
        let mut overlay = node(
            Some(0),
            rect(0., 0., 150., 50.),
            Some("overlay"),
            Flags {
                blocks: true,
                ..Default::default()
            },
        );
        overlay.type_name = "gpui::Div".into();
        tree.nodes.push(overlay);
        let frame = Frame::new(&tree);
        assert_eq!(frame.hit(10., 10.), Some(7));
        assert_eq!(frame.hit(10., 65.), Some(4));
    }
}
