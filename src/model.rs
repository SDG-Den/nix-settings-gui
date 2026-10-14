use std::collections::HashMap;
use std::path::Path;

use crate::dsl::{MenuSpec, OptionSpec};

/// One config assignment found by the scanner, tied to its `@option`
/// annotation. Spans index the original file text so the edit engine can
/// replace the value in place.
#[derive(Debug, Clone, Default)]
pub struct Record {
    /// Option attribute path relative to the module body, e.g.
    /// `["services", "pipewire", "enable"]`.
    pub segments: Vec<String>,
    pub file: String,
    pub line: u32,
    pub col: u32,
    /// Span of the (peeled) value expression.
    pub byte_start: usize,
    pub byte_end: usize,
    /// Source text of the value expression.
    pub value: String,
    /// Leading `with ...;` wrapper peeled off the value, e.g. `with pkgs;`,
    /// preserved on write so the inner list is rewritten in place.
    pub wrapper: Option<String>,
    /// Flake output path this record came from, e.g. `modules.nixos.audio`.
    /// Empty when the assignment is written directly in the file body.
    pub origin: String,
    pub spec: OptionSpec,
    /// Span of the `@option` comment itself, when present.
    pub comment_span: Option<(usize, usize)>,
}

impl Record {
    pub fn path(&self) -> String {
        self.segments.join(".")
    }
}

/// A node in the flattened option tree: a namespace (folder) or a key (leaf).
#[derive(Debug, Clone, Default)]
pub struct Node {
    pub name: String,
    pub path: String,
    pub segments: Vec<String>,
    pub ty: String,
    pub value: Option<String>,
    pub records: Vec<Record>,
    pub spec: OptionSpec,
}

impl Node {
    pub fn is_folder(&self) -> bool {
        self.ty == "namespace"
    }

    pub fn short_type(&self) -> &str {
        if self.is_folder() {
            "namespace"
        } else {
            "key"
        }
    }
}

/// One `.nix` file rendered as a settings menu.
#[derive(Debug, Clone, Default)]
pub struct MenuModel {
    pub file: String,
    pub spec: MenuSpec,
    pub records: Vec<Record>,
}

impl MenuModel {
    pub fn display_name(&self) -> String {
        if let Some(name) = &self.spec.displayname {
            return name.clone();
        }
        Path::new(&self.file)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.file.clone())
    }

    pub fn order(&self) -> i64 {
        self.spec.order.unwrap_or(1000)
    }

    /// A menu with no `systems` restriction is shared by every host.
    pub fn shared(&self) -> bool {
        self.spec.systems.is_empty()
    }
}

/// Group records into parent-path -> children nodes. Namespaces are the
/// dotted-path prefixes of every defined key; keys are the leaf assignments.
pub fn build_levels(records: &[Record]) -> HashMap<String, Vec<Node>> {
    let mut levels: HashMap<String, Vec<Node>> = HashMap::new();

    fn merge(nodes: &mut Vec<Node>, node: Node) {
        match nodes.iter_mut().find(|n| n.path == node.path) {
            Some(existing) => {
                if node.ty == "namespace" && existing.ty == "key" {
                    existing.ty = "namespace".to_string();
                } else if node.ty == "key" && existing.ty == "key" {
                    existing.records.extend(node.records);
                    if existing.value.is_none() {
                        existing.value = node.value;
                    }
                }
            }
            None => nodes.push(node),
        }
    }

    for rec in records {
        let segs = &rec.segments;
        for depth in 1..=segs.len() {
            let parent = &segs[..depth - 1];
            let parent_key = parent.join(".");
            let path = segs[..depth].join(".");
            let name = segs[depth - 1].clone();
            let segments = segs[..depth].to_vec();

            if depth < segs.len() {
                let node = Node {
                    ty: "namespace".to_string(),
                    name,
                    path,
                    segments,
                    value: None,
                    records: Vec::new(),
                    spec: OptionSpec::default(),
                };
                merge(levels.entry(parent_key).or_default(), node);
            } else {
                let node = Node {
                    ty: "key".to_string(),
                    value: Some(rec.value.clone()),
                    records: vec![rec.clone()],
                    name,
                    path,
                    segments,
                    spec: rec.spec.clone(),
                };
                merge(levels.entry(parent_key).or_default(), node);
            }
        }
    }

    levels
}
