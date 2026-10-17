use std::collections::HashMap;
use std::path::Path;

use crate::dsl::{MenuSpec, OptionSpec, SnippetSpec};

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

/// A `@insert` marker in a file: the point where a snippet's instances are
/// inserted, and the group the add button belongs to.
#[derive(Debug, Clone, Default)]
pub struct InsertMarker {
    pub name: String,
    pub group: Option<String>,
    pub order: Option<i64>,
    pub label: Option<String>,
    /// Byte offset of the start of the line holding the marker comment, so an
    /// insert lands above the marker and shifts it down.
    pub offset: usize,
    /// 1-based line of the marker, for diagnostics.
    pub line: u32,
}

impl InsertMarker {
    pub fn from_spec(spec: &crate::dsl::InsertSpec, offset: usize, line: u32) -> Self {
        Self {
            name: spec.name.clone(),
            group: spec.group.clone(),
            order: spec.order,
            label: spec.label.clone(),
            offset,
            line,
        }
    }
}

/// One `.nix` file rendered as a settings menu.
#[derive(Debug, Clone, Default)]
pub struct MenuModel {
    pub file: String,
    pub spec: MenuSpec,
    pub records: Vec<Record>,
    /// Snippets defined by `@snippet` markers in the file.
    pub snippets: Vec<SnippetSpec>,
    /// `@insert` markers in the file, in source order.
    pub inserts: Vec<InsertMarker>,
    /// Module origins this file defines, used to filter by host import graph.
    pub origins: Vec<String>,
    /// Whether the file carries an `@menu` block. Files without one are never
    /// shown, regardless of the visibility toggles.
    pub has_menu: bool,
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

/// Rank the groups of a list of records. Groups with an explicit `groupOrder`
/// come first, ordered by that value; the rest follow in order of first source
/// appearance. The empty (ungrouped) group always ranks first.
pub fn group_ranks(records: &[Record]) -> HashMap<String, usize> {
    let mut info: HashMap<String, (Option<i64>, usize)> = HashMap::new();
    for (index, record) in records.iter().enumerate() {
        let group = record.spec.group.clone().unwrap_or_default();
        if group.is_empty() {
            continue;
        }
        let entry = info.entry(group).or_insert((None, index));
        if let Some(order) = record.spec.group_order {
            entry.0 = Some(entry.0.map_or(order, |existing| existing.min(order)));
        }
    }
    let mut groups: Vec<(String, Option<i64>, usize)> = info
        .into_iter()
        .map(|(group, (order, index))| (group, order, index))
        .collect();
    groups.sort_by(|a, b| match (a.1, b.1) {
        (Some(x), Some(y)) => x.cmp(&y).then(a.2.cmp(&b.2)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.2.cmp(&b.2),
    });
    let mut ranks = HashMap::new();
    ranks.insert(String::new(), 0);
    for (rank, (group, _, _)) in groups.into_iter().enumerate() {
        ranks.insert(group, rank + 1);
    }
    ranks
}

/// Order two records by group rank, then explicit `order`, then source index.
/// Records with an explicit `order` sort before records without one.
pub fn record_order(
    a: &Record,
    a_index: usize,
    b: &Record,
    b_index: usize,
    ranks: &HashMap<String, usize>,
) -> std::cmp::Ordering {
    let ag = a.spec.group.clone().unwrap_or_default();
    let bg = b.spec.group.clone().unwrap_or_default();
    let ar = ranks.get(&ag).copied().unwrap_or(usize::MAX);
    let br = ranks.get(&bg).copied().unwrap_or(usize::MAX);
    let order = match (a.spec.order, b.spec.order) {
        (Some(ao), Some(bo)) => ao.cmp(&bo),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    };
    ar.cmp(&br)
        .then_with(|| order)
        .then_with(|| a_index.cmp(&b_index))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::scan_value;

    fn spec_record(group: Option<&str>, order: Option<i64>, group_order: Option<i64>) -> Record {
        let mut record = scan_value("{ x = 1; }").pop().unwrap();
        record.spec.group = group.map(str::to_string);
        record.spec.order = order;
        record.spec.group_order = group_order;
        record
    }

    #[test]
    fn group_ranks_order_by_group_order_then_source() {
        let records = vec![
            spec_record(Some("Zeta"), None, None),
            spec_record(Some("Alpha"), None, Some(5)),
            spec_record(None, None, None),
        ];
        let ranks = group_ranks(&records);
        assert_eq!(ranks.get(""), Some(&0));
        assert_eq!(ranks.get("Alpha"), Some(&1));
        assert_eq!(ranks.get("Zeta"), Some(&2));
    }

    #[test]
    fn group_ranks_use_min_group_order_and_tiebreak_by_source() {
        let records = vec![
            spec_record(Some("B"), None, Some(5)),
            spec_record(Some("A"), None, Some(5)),
            spec_record(Some("C"), None, Some(1)),
        ];
        let ranks = group_ranks(&records);
        assert_eq!(ranks.get("C"), Some(&1));
        assert_eq!(ranks.get("B"), Some(&2));
        assert_eq!(ranks.get("A"), Some(&3));
    }

    #[test]
    fn record_order_ranks_groups_then_order_then_source() {
        let mut ranks = HashMap::new();
        ranks.insert(String::new(), 0);
        ranks.insert("A".to_string(), 1);
        ranks.insert("B".to_string(), 2);

        let a_unordered = spec_record(Some("A"), None, None);
        let a_first = spec_record(Some("A"), Some(1), None);
        let a_second = spec_record(Some("A"), Some(2), None);
        let b = spec_record(Some("B"), None, None);
        let later = spec_record(Some("A"), None, None);

        assert_eq!(
            record_order(&a_unordered, 0, &b, 0, &ranks),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            record_order(&a_second, 0, &a_unordered, 1, &ranks),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            record_order(&a_first, 0, &a_second, 0, &ranks),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            record_order(&a_unordered, 3, &later, 1, &ranks),
            std::cmp::Ordering::Greater
        );
    }
}
