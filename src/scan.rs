use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use rnix::ast::{Attr, AttrSet, AttrpathValue, Expr, HasEntry, InterpolPart, List, Select, With};
use rnix::{NodeOrToken, SyntaxKind, SyntaxNode};
use rowan::ast::AstNode;

use crate::dsl::{
    self, InsertSpec, MarkerKind, MenuSpec, OptionSpec, PresetFieldSpec, SnippetSpec,
    WidgetPresetSpec,
};
use crate::model::{InsertMarker, MenuModel, Record};

/// Everything the scanner learns from one file.
#[derive(Debug, Clone, Default)]
pub struct FileScan {
    pub file: String,
    pub spec: MenuSpec,
    /// Whether an `@menu` block was present in the file.
    pub has_menu: bool,
    pub records: Vec<Record>,
    /// `@snippet` definitions in the file.
    pub snippets: Vec<SnippetSpec>,
    /// `@insert` markers in the file, in source order.
    pub inserts: Vec<InsertMarker>,
    pub widget_presets: Vec<(WidgetPresetSpec, Vec<PresetFieldSpec>)>,
    pub targets: Vec<String>,
    /// Module origins this file defines, e.g. `modules.nixos.audio`.
    pub origins: Vec<String>,
    /// Module origins this file references, e.g. `modules.nixos.common`.
    pub imports: Vec<String>,
    /// Per-host direct module references, keyed by target, e.g.
    /// `nixos:alpha`.
    pub target_imports: HashMap<String, Vec<String>>,
    /// Every `@inputs` declaration found in this file.
    pub declared_inputs: Vec<crate::flake_inputs::DeclaredInput>,
}

/// The whole scan of a config tree.
#[derive(Debug, Clone, Default)]
pub struct ScanResult {
    pub menus: Vec<MenuModel>,
    pub targets: Vec<String>,
    /// Origin to the menu file that defines it.
    pub origin_menu: HashMap<String, String>,
    /// Origin to the origins it imports.
    pub module_imports: HashMap<String, Vec<String>>,
    /// Target to the origins its configuration pulls in directly.
    pub target_imports: HashMap<String, Vec<String>>,
    /// Every `@inputs` declaration found in the tree, including files that do
    /// not become menus.
    pub declared_inputs: Vec<crate::flake_inputs::DeclaredInput>,
}

impl ScanResult {
    /// Menu filenames restricted to one host. A `systems` list overrides the
    /// import graph; otherwise a menu shows when one of its origins is reachable
    /// from the host's configuration. Menus with no origin fall back to the
    /// `hosts/<host>/` directory name, and any file with neither restriction is
    /// shared.
    pub fn menus_for(&self, host: &str) -> Vec<&MenuModel> {
        let resolved = self.resolve_host(&format!("nixos:{host}"));
        self.menus
            .iter()
            .filter(|menu| self.menu_visible(menu, host, resolved.as_ref()))
            .collect()
    }

    fn menu_visible(
        &self,
        menu: &MenuModel,
        host: &str,
        resolved: Option<&HashSet<String>>,
    ) -> bool {
        if !menu.spec.systems.is_empty() {
            return menu.spec.systems.iter().any(|s| s == "all" || s == host);
        }
        if !menu.origins.is_empty() {
            if let Some(reachable) = resolved {
                return menu.origins.iter().any(|o| reachable.contains(o));
            }
        }
        match menu_host_dir(&menu.file) {
            Some(dir) => dir == host,
            None => true,
        }
    }

    /// The origins reachable from a target by following module imports
    /// transitively, or `None` when the target has no resolvable imports.
    pub fn resolve_host(&self, target: &str) -> Option<HashSet<String>> {
        let direct = self.target_imports.get(target)?;
        let mut reachable = HashSet::new();
        let mut stack = direct.clone();
        while let Some(origin) = stack.pop() {
            if !reachable.insert(origin.clone()) {
                continue;
            }
            if let Some(next) = self.module_imports.get(&origin) {
                for import in next {
                    if !reachable.contains(import) {
                        stack.push(import.clone());
                    }
                }
            }
        }
        if reachable.is_empty() {
            None
        } else {
            Some(reachable)
        }
    }
}

/// Scan every `.nix` file under `root` into a menu model plus the discovered
/// system targets.
pub fn scan_root(root: &str) -> Result<ScanResult, String> {
    let mut files = Vec::new();
    collect_nix_files(Path::new(root), &mut files)?;
    files.sort();

    let mut result = ScanResult::default();
    for file in &files {
        let text = std::fs::read_to_string(file)
            .map_err(|e| format!("cannot read {}: {e}", file.display()))?;
        let scan = scan_file(file, &text);
        result.declared_inputs.extend(scan.declared_inputs.clone());
        result.targets.extend(scan.targets.clone());
        for origin in &scan.origins {
            result
                .origin_menu
                .entry(origin.clone())
                .or_insert_with(|| scan.file.clone());
            let entry = result.module_imports.entry(origin.clone()).or_default();
            for import in &scan.imports {
                if !entry.contains(import) {
                    entry.push(import.clone());
                }
            }
        }
        for (target, imports) in &scan.target_imports {
            let entry = result.target_imports.entry(target.clone()).or_default();
            for import in imports {
                if !entry.contains(import) {
                    entry.push(import.clone());
                }
            }
        }
        result.menus.push(MenuModel {
            file: scan.file,
            spec: scan.spec,
            records: scan.records,
            snippets: scan.snippets,
            inserts: scan.inserts,
            widget_presets: scan.widget_presets.clone(),
            origins: scan.origins,
            has_menu: scan.has_menu,
        });
    }

    result.targets.sort();
    result.targets.dedup();
    result.menus.sort_by(|a, b| {
        a.order()
            .cmp(&b.order())
            .then_with(|| a.display_name().cmp(&b.display_name()))
    });
    Ok(result)
}

fn collect_nix_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries =
        std::fs::read_dir(dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy().into_owned();
        if name == "flake.nix" || name.starts_with('_') {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            if name.starts_with('.') || matches!(name.as_str(), "result" | "target") {
                continue;
            }
            collect_nix_files(&path, out)?;
        } else if path.extension().map(|e| e == "nix").unwrap_or(false) {
            out.push(path);
        }
    }
    Ok(())
}

/// Parse one file: find the menu header, then collect every option assignment
/// with the `@option` comment that sits directly above it.
pub fn scan_file(path: &Path, text: &str) -> FileScan {
    let file = path.display().to_string();
    let mut scan = FileScan {
        file: file.clone(),
        ..FileScan::default()
    };

    let parsed = rnix::Root::parse(text);
    scan.declared_inputs =
        crate::flake_inputs::declared_inputs_from_syntax(text, &parsed.syntax(), &file);
    let Some(root) = rnix::Root::cast(parsed.syntax()) else {
        return scan;
    };
    if let Some(spec) = find_menu(root.syntax()) {
        scan.spec = spec;
        scan.has_menu = true;
    }
    scan.snippets = find_snippets(root.syntax());
    scan.inserts = find_inserts(text, root.syntax());
    scan.widget_presets = find_widget_presets(text, root.syntax());

    let Some(expr) = root.expr() else {
        return scan;
    };

    if let Some(set) = top_attrset(&expr) {
        if has_flake_container(&set) {
            scan_dendritic(text, &set, &mut scan);
            return scan;
        }
    }

    walk_body(text, &file, "", &expr, &mut scan.records, true, true);
    scan
}

/// Scan a value literal on its own, e.g. the inner attribute set of a
/// submodule control. Returned spans are relative to `text`, so a caller adds
/// the parent value's file offset to turn them into whole-file spans.
pub fn scan_value(text: &str) -> Vec<Record> {
    let parsed = rnix::Root::parse(text);
    let Some(root) = rnix::Root::cast(parsed.syntax()) else {
        return Vec::new();
    };
    let Some(expr) = root.expr() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    walk_body(text, "", "", &expr, &mut out, false, true);
    out
}

/// Scan a value literal's direct bindings without flattening nested attribute
/// sets. A card collection needs one record per entry, not one per leaf.
pub fn scan_value_direct(text: &str) -> Vec<Record> {
    let parsed = rnix::Root::parse(text);
    let Some(root) = rnix::Root::cast(parsed.syntax()) else {
        return Vec::new();
    };
    let Some(expr) = root.expr() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    walk_body(text, "", "", &expr, &mut out, false, false);
    out
}

/// One row of a `table-attrset` / `table-list` value.
#[derive(Debug, Clone, Default)]
pub struct TableRowScan {
    /// Parent attribute name for `table-attrset`, empty for `table-list`.
    pub key: String,
    /// Field name -> source text of the field value.
    pub fields: Vec<(String, String)>,
    /// Per-row settings from a leading `@table-item` comment.
    pub spec: OptionSpec,
}

/// Read a table value into rows. `list` selects `table-list` (`[ { ... } ]`)
/// over `table-attrset` (`{ key = { ... }; }`).
pub fn scan_table(value: &str, list: bool) -> Vec<TableRowScan> {
    let Ok(root) = rnix::Root::parse(value).ok() else {
        return Vec::new();
    };
    let Some(expr) = root.expr() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if list {
        let Expr::List(list_node) = expr else {
            return Vec::new();
        };
        let mut pending: Option<OptionSpec> = None;
        for element in list_node.syntax().children_with_tokens() {
            match element {
                NodeOrToken::Token(token) => match token.kind() {
                    SyntaxKind::TOKEN_COMMENT => pending = table_item_spec(token.text()),
                    SyntaxKind::TOKEN_WHITESPACE => {}
                    _ => pending = None,
                },
                NodeOrToken::Node(node) => {
                    if Expr::cast(node.clone()).is_none() {
                        continue;
                    }
                    let spec = pending.take().unwrap_or_default();
                    if let Some(fields) = row_fields(&node) {
                        out.push(TableRowScan {
                            key: String::new(),
                            fields,
                            spec,
                        });
                    }
                }
            }
        }
    } else {
        let Expr::AttrSet(set) = expr else {
            return Vec::new();
        };
        let mut pending: Option<OptionSpec> = None;
        for element in set.syntax().children_with_tokens() {
            match element {
                NodeOrToken::Token(token) => match token.kind() {
                    SyntaxKind::TOKEN_COMMENT => pending = table_item_spec(token.text()),
                    SyntaxKind::TOKEN_WHITESPACE => {}
                    _ => pending = None,
                },
                NodeOrToken::Node(node) => {
                    if node.kind() != SyntaxKind::NODE_ATTRPATH_VALUE {
                        continue;
                    }
                    let Some(binding) = AttrpathValue::cast(node) else {
                        continue;
                    };
                    let spec = pending.take().unwrap_or_default();
                    let key = binding
                        .attrpath()
                        .and_then(|path| {
                            let comps: Vec<String> =
                                path.attrs().filter_map(|a| attr_component(&a)).collect();
                            (comps.len() == 1).then(|| comps[0].clone())
                        })
                        .unwrap_or_default();
                    let fields = binding
                        .value()
                        .and_then(|v| row_fields(v.syntax()))
                        .unwrap_or_default();
                    out.push(TableRowScan { key, fields, spec });
                }
            }
        }
    }
    out
}

/// The `key = value` fields of a row's attribute set.
fn row_fields(node: &SyntaxNode) -> Option<Vec<(String, String)>> {
    let Expr::AttrSet(set) = Expr::cast(node.clone())? else {
        return None;
    };
    let mut fields = Vec::new();
    for binding in set.attrpath_values() {
        let Some(path) = binding.attrpath() else {
            continue;
        };
        let comps: Vec<String> = path.attrs().filter_map(|a| attr_component(&a)).collect();
        if comps.len() != 1 {
            continue;
        }
        let Some(value) = binding.value() else {
            continue;
        };
        fields.push((comps[0].clone(), value.syntax().text().to_string()));
    }
    Some(fields)
}

/// Parse a leading `@table-item` comment into a per-row spec.
fn table_item_spec(text: &str) -> Option<OptionSpec> {
    match dsl::parse_comment(text) {
        Some((MarkerKind::TableItem, directives)) => Some(OptionSpec::from_directives(&directives)),
        _ => None,
    }
}

/// Byte offset just after the opening brace of the module body selected by
/// `origin`: the top-level body for a plain file, or the
/// `flake.modules.<kind>.<name>` body for a dendritic file.
pub fn body_insert_offset(text: &str, origin: &str) -> Option<usize> {
    let root = rnix::Root::parse(text).ok().ok()?;
    let expr = root.expr()?;

    if origin.is_empty() {
        return insert_body(&expr).and_then(|set| brace_offset(&set));
    }

    let comps: Vec<&str> = origin.split('.').collect();
    if comps.len() != 3 || comps[0] != "modules" {
        return None;
    }
    let top = top_attrset(&expr)?;
    for binding in top.attrpath_values() {
        let Some(path) = binding.attrpath() else {
            continue;
        };
        let binding_comps: Vec<String> = path.attrs().filter_map(|a| attr_component(&a)).collect();
        let expected = ["flake", "modules", comps[1], comps[2]];
        if binding_comps.len() == 4
            && binding_comps
                .iter()
                .map(String::as_str)
                .eq(expected.iter().copied())
        {
            let Some(value) = binding.value() else {
                continue;
            };
            if let Some(set) = insert_body(&value) {
                return brace_offset(&set);
            }
        }
    }
    None
}

fn brace_offset(set: &AttrSet) -> Option<usize> {
    set.syntax()
        .children_with_tokens()
        .filter_map(|element| element.into_token())
        .find(|token| token.kind() == SyntaxKind::TOKEN_L_BRACE)
        .map(|token| usize::from(token.text_range().end()))
}

/// The attribute set an inserted assignment belongs to: unwrap wrappers and
/// descend through merges (`//`) and conditionals (`if`) to the first set.
fn insert_body(expr: &Expr) -> Option<AttrSet> {
    match peel_body(expr)? {
        Expr::AttrSet(set) => Some(set),
        Expr::BinOp(op) => op
            .lhs()
            .and_then(|lhs| insert_body(&lhs))
            .or_else(|| op.rhs().and_then(|rhs| insert_body(&rhs))),
        Expr::IfElse(cond) => cond
            .body()
            .and_then(|body| insert_body(&body))
            .or_else(|| cond.else_body().and_then(|body| insert_body(&body))),
        _ => None,
    }
}

/// Find all widget presets and their field specs in a file. Field specs are
/// matched to their preset by id after the whole file has been scanned, so a
/// field marker does not have to immediately follow its preset.
pub fn find_widget_presets(
    _text: &str,
    root: &SyntaxNode,
) -> Vec<(WidgetPresetSpec, Vec<PresetFieldSpec>)> {
    let mut presets: Vec<WidgetPresetSpec> = Vec::new();
    let mut fields: Vec<PresetFieldSpec> = Vec::new();
    for element in root.descendants_with_tokens() {
        let Some(token) = element.into_token() else {
            continue;
        };
        if token.kind() != SyntaxKind::TOKEN_COMMENT {
            continue;
        }
        if let Some((MarkerKind::WidgetPreset, directives)) = dsl::parse_comment(token.text()) {
            let spec = WidgetPresetSpec::from_directives(&directives);
            if !spec.id.is_empty() {
                presets.push(spec);
            }
            continue;
        }
        if let Some((MarkerKind::WidgetPresetField, directives)) = dsl::parse_comment(token.text())
        {
            let spec = PresetFieldSpec::from_directives(&directives);
            if !spec.preset.is_empty() && !spec.name.is_empty() {
                fields.push(spec);
            }
        }
    }
    presets
        .into_iter()
        .map(|preset| {
            let own: Vec<PresetFieldSpec> = fields
                .iter()
                .filter(|f| f.preset == preset.id)
                .cloned()
                .collect();
            (preset, own)
        })
        .collect()
}

/// The first `@menu` comment anywhere in the file is the menu header.
fn find_menu(root: &SyntaxNode) -> Option<MenuSpec> {
    for element in root.descendants_with_tokens() {
        if let Some(token) = element.into_token() {
            if token.kind() == SyntaxKind::TOKEN_COMMENT {
                if let Some((MarkerKind::Menu, directives)) = dsl::parse_comment(token.text()) {
                    return Some(MenuSpec::from_directives(&directives));
                }
            }
        }
    }
    None
}

/// Every `@snippet` marker in the file, in source order.
fn find_snippets(root: &SyntaxNode) -> Vec<SnippetSpec> {
    let mut out = Vec::new();
    for element in root.descendants_with_tokens() {
        let Some(token) = element.into_token() else {
            continue;
        };
        if token.kind() != SyntaxKind::TOKEN_COMMENT {
            continue;
        }
        if let Some((MarkerKind::Snippet, directives)) = dsl::parse_comment(token.text()) {
            let spec = SnippetSpec::from_directives(&directives);
            if !spec.name.is_empty() {
                out.push(spec);
            }
        }
    }
    out
}

/// Every `@insert` marker in the file, in source order, with the byte offset of
/// the start of its line.
fn find_inserts(text: &str, root: &SyntaxNode) -> Vec<InsertMarker> {
    let mut out = Vec::new();
    for element in root.descendants_with_tokens() {
        let Some(token) = element.into_token() else {
            continue;
        };
        if token.kind() != SyntaxKind::TOKEN_COMMENT {
            continue;
        }
        let Some((MarkerKind::Insert, directives)) = dsl::parse_comment(token.text()) else {
            continue;
        };
        let comment_start = usize::from(token.text_range().start());
        let offset = text[..comment_start]
            .rfind('\n')
            .map(|i| i + 1)
            .unwrap_or(0);
        let (line, _) = line_col(text, offset);
        let spec = InsertSpec::from_directives(&directives);
        if !spec.name.is_empty() {
            out.push(InsertMarker::from_spec(&spec, offset, line));
        }
    }
    out
}

fn top_attrset(expr: &Expr) -> Option<AttrSet> {
    match peel_body(expr)? {
        Expr::AttrSet(set) => Some(set),
        _ => None,
    }
}

fn has_flake_container(set: &AttrSet) -> bool {
    set.attrpath_values().any(|binding| {
        binding
            .attrpath()
            .and_then(|p| p.attrs().next())
            .and_then(|a| attr_component(&a))
            .as_deref()
            == Some("flake")
    })
}

/// Dendritic files express each module as `flake.modules.<kind>.<name>` and
/// each host as `flake.nixosConfigurations.<host>`.
fn scan_dendritic(text: &str, set: &AttrSet, scan: &mut FileScan) {
    let file = scan.file.clone();
    for binding in set.attrpath_values() {
        let Some(path) = binding.attrpath() else {
            continue;
        };
        let comps: Vec<String> = path.attrs().filter_map(|a| attr_component(&a)).collect();
        if comps.first().map(String::as_str) != Some("flake") || comps.len() < 3 {
            continue;
        }

        if comps[1] == "modules" && comps.len() >= 4 {
            let origin = format!("modules.{}.{}", comps[2], comps[3]);
            scan.origins.push(origin.clone());
            let Some(value) = binding.value() else {
                continue;
            };
            walk_body(text, &file, &origin, &value, &mut scan.records, true, true);
            collect_module_refs(value.syntax(), &mut scan.imports);
            continue;
        }

        let target = match comps[1].as_str() {
            "nixosConfigurations" => Some(format!("nixos:{}", comps[2])),
            "homeConfigurations" => Some(format!("home:{}", comps[2])),
            "darwinConfigurations" => Some(format!("darwin:{}", comps[2])),
            _ => None,
        };
        if let Some(target) = target {
            scan.targets.push(target.clone());
            let Some(value) = binding.value() else {
                continue;
            };
            let mut imports = Vec::new();
            collect_module_refs(value.syntax(), &mut imports);
            if !imports.is_empty() {
                scan.target_imports.insert(target, imports);
            }
        }
    }
}

/// Peel lambda / let / with / paren wrappers around a module body.
fn peel_body(expr: &Expr) -> Option<Expr> {
    let mut current = expr.clone();
    loop {
        current = match current {
            Expr::Lambda(l) => l.body()?,
            Expr::LetIn(l) => l.body()?,
            Expr::With(w) => w.body()?,
            Expr::Paren(p) => p.expr()?,
            other => return Some(other),
        };
    }
}

/// Walk a module body, unwrapping wrappers, and collect assignments. `let`
/// bindings carry their own comments, so they are collected before the body;
/// they always require an `@option` comment so helper variables stay hidden.
/// `annotated_only` hides bindings without an `@option` comment; submodule
/// scans pass `false` so their inner fields always render.
fn walk_body(
    text: &str,
    file: &str,
    origin: &str,
    expr: &Expr,
    out: &mut Vec<Record>,
    annotated_only: bool,
    flatten: bool,
) {
    match expr {
        Expr::Lambda(l) => {
            if let Some(body) = l.body() {
                walk_body(text, file, origin, &body, out, annotated_only, flatten);
            }
        }
        Expr::LetIn(l) => {
            collect_container(text, file, origin, l.syntax(), &[], out, true, flatten);
            if let Some(body) = l.body() {
                walk_body(text, file, origin, &body, out, annotated_only, flatten);
            }
        }
        Expr::With(w) => {
            if let Some(body) = w.body() {
                walk_body(text, file, origin, &body, out, annotated_only, flatten);
            }
        }
        Expr::Paren(p) => {
            if let Some(inner) = p.expr() {
                walk_body(text, file, origin, &inner, out, annotated_only, flatten);
            }
        }
        Expr::AttrSet(set) => collect_container(
            text,
            file,
            origin,
            set.syntax(),
            &[],
            out,
            annotated_only,
            flatten,
        ),
        Expr::BinOp(op) => {
            if let Some(lhs) = op.lhs() {
                walk_body(text, file, origin, &lhs, out, annotated_only, flatten);
            }
            if let Some(rhs) = op.rhs() {
                walk_body(text, file, origin, &rhs, out, annotated_only, flatten);
            }
        }
        Expr::IfElse(cond) => {
            if let Some(body) = cond.body() {
                walk_body(text, file, origin, &body, out, annotated_only, flatten);
            }
            if let Some(else_body) = cond.else_body() {
                walk_body(text, file, origin, &else_body, out, annotated_only, flatten);
            }
        }
        _ => {}
    }
}

/// Walk the direct children of a container (an attribute set or a `let` block),
/// tracking the most recent comment so it can bind to the next assignment.
#[allow(clippy::too_many_arguments)]
fn collect_container(
    text: &str,
    file: &str,
    origin: &str,
    container: &SyntaxNode,
    prefix: &[String],
    out: &mut Vec<Record>,
    annotated_only: bool,
    flatten: bool,
) {
    let mut pending: Option<(String, usize, usize)> = None;
    for element in container.children_with_tokens() {
        match element {
            NodeOrToken::Token(token) => match token.kind() {
                SyntaxKind::TOKEN_COMMENT => {
                    let range = token.text_range();
                    pending = Some((
                        token.text().to_string(),
                        usize::from(range.start()),
                        usize::from(range.end()),
                    ));
                }
                SyntaxKind::TOKEN_WHITESPACE => {}
                _ => pending = None,
            },
            NodeOrToken::Node(node) => {
                let comment = pending.take();
                if node.kind() != SyntaxKind::NODE_ATTRPATH_VALUE {
                    continue;
                }
                let Some(binding) = AttrpathValue::cast(node) else {
                    continue;
                };
                collect_binding(
                    text,
                    file,
                    origin,
                    &binding,
                    prefix,
                    comment,
                    out,
                    annotated_only,
                    flatten,
                );
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn collect_binding(
    text: &str,
    file: &str,
    origin: &str,
    binding: &AttrpathValue,
    prefix: &[String],
    comment: Option<(String, usize, usize)>,
    out: &mut Vec<Record>,
    annotated_only: bool,
    flatten: bool,
) {
    let Some(path) = binding.attrpath() else {
        return;
    };
    let mut comps = Vec::new();
    for attr in path.attrs() {
        match attr_component(&attr) {
            Some(component) => comps.push(component),
            None => return,
        }
    }
    if comps.is_empty() {
        return;
    }
    let mut segments = prefix.to_vec();
    segments.extend(comps);

    let mut comment_span = None;
    let mut spec = OptionSpec::default();
    let mut annotated = false;
    if let Some((body, start, end)) = &comment {
        if let Some((MarkerKind::Option, directives)) = dsl::parse_comment(body) {
            spec = OptionSpec::from_directives(&directives);
            comment_span = Some((*start, *end));
            annotated = true;
        }
    }

    let Some(value) = binding.value() else {
        return;
    };
    let value_start = usize::from(value.syntax().text_range().start());
    let value_text = value.syntax().text().to_string();
    let Some(inner) = peel_value(&value) else {
        return;
    };
    let inner_start = usize::from(inner.syntax().text_range().start());
    let wrapper = {
        let head = value_text
            .get(..inner_start.saturating_sub(value_start))
            .unwrap_or("")
            .trim();
        if head.is_empty() {
            None
        } else {
            Some(head.to_string())
        }
    };

    if let Expr::AttrSet(nested) = &inner {
        if !annotated && flatten {
            collect_container(
                text,
                file,
                origin,
                nested.syntax(),
                &segments,
                out,
                annotated_only,
                flatten,
            );
            return;
        }
    }

    if annotated_only && !annotated {
        return;
    }

    let node = if annotated && matches!(inner, Expr::AttrSet(_)) {
        value.syntax()
    } else {
        inner.syntax()
    };
    let range = node.text_range();
    let byte_start = usize::from(range.start());
    let (line, col) = line_col(text, byte_start);
    out.push(Record {
        segments,
        file: file.to_string(),
        line,
        col,
        byte_start,
        byte_end: usize::from(range.end()),
        value: node.text().to_string(),
        wrapper,
        origin: origin.to_string(),
        spec,
        comment_span,
    });
}

/// Peel `with` / `let` / paren wrappers so the value span points at the inner
/// expression that will be replaced on write.
fn peel_value(expr: &Expr) -> Option<Expr> {
    let mut current = expr.clone();
    loop {
        current = match current {
            Expr::With(w) => w.body()?,
            Expr::LetIn(l) => l.body()?,
            Expr::Paren(p) => p.expr()?,
            other => return Some(other),
        };
    }
}

fn attr_component(attr: &Attr) -> Option<String> {
    match attr {
        Attr::Ident(ident) => Some(ident.syntax().text().to_string()),
        Attr::Str(s) => match s.normalized_parts().as_slice() {
            [InterpolPart::Literal(text)] => Some(text.clone()),
            _ => None,
        },
        Attr::Dynamic(_) => None,
    }
}

/// Collect every `self.modules.<kind>.<name>` (or `flake.modules...`)
/// reference appearing anywhere in `node`, normalized to `modules.<kind>.<name>`.
/// Inside a `with self.modules.<kind>; [ a b ]` scope, the bare list items are
/// module names, so they are normalized too.
fn collect_module_refs(node: &SyntaxNode, out: &mut Vec<String>) {
    for descendant in node.descendants() {
        if descendant.kind() == SyntaxKind::NODE_SELECT {
            let Some(select) = Select::cast(descendant) else {
                continue;
            };
            if let Some(path) = dotted_path(&select) {
                if let Some(origin) = module_origin_from_path(&path) {
                    push_unique(out, origin);
                }
            }
        } else if descendant.kind() == SyntaxKind::NODE_WITH {
            collect_with_refs(&descendant, out);
        }
    }
}

/// The bare module names a `with self.modules.<kind>; [...]` scope brings in.
fn collect_with_refs(node: &SyntaxNode, out: &mut Vec<String>) {
    let Some(with) = With::cast(node.clone()) else {
        return;
    };
    let Some(namespace) = with.namespace() else {
        return;
    };
    let Some(path) = select_base_path(&namespace) else {
        return;
    };
    let Some(kind) = module_kind_from_path(&path) else {
        return;
    };
    let Some(body) = with.body() else {
        return;
    };
    for descendant in body.syntax().descendants() {
        if descendant.kind() != SyntaxKind::NODE_LIST {
            continue;
        }
        let Some(list) = List::cast(descendant) else {
            continue;
        };
        for item in list.items() {
            if let Some(name) = bare_ident(&item) {
                push_unique(out, format!("modules.{kind}.{name}"));
            }
        }
    }
}

/// A list item that is just a bare name (optionally parenthesized).
fn bare_ident(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Ident(ident) => Some(ident.syntax().text().to_string()),
        Expr::Paren(paren) => paren.expr().and_then(|inner| bare_ident(&inner)),
        _ => None,
    }
}

/// The module kind of a `with` namespace path such as `self.modules.nixos`.
fn module_kind_from_path(path: &[String]) -> Option<String> {
    let comps: Vec<&str> = path.iter().map(String::as_str).collect();
    match comps.as_slice() {
        [base, "modules", kind] if *base == "self" || *base == "flake" => Some((*kind).to_string()),
        ["modules", kind] => Some((*kind).to_string()),
        _ => None,
    }
}

fn push_unique(out: &mut Vec<String>, origin: String) {
    if !out.contains(&origin) {
        out.push(origin);
    }
}

/// Rebuild the dotted attribute path of a selection chain, e.g.
/// `self.modules.nixos.audio` -> `["self","modules","nixos","audio"]`.
fn dotted_path(select: &Select) -> Option<Vec<String>> {
    let mut path = select_base_path(&select.expr()?)?;
    for attr in select.attrpath()?.attrs() {
        path.push(attr_component(&attr)?);
    }
    Some(path)
}

fn select_base_path(expr: &Expr) -> Option<Vec<String>> {
    match expr {
        Expr::Ident(ident) => Some(vec![ident.syntax().text().to_string()]),
        Expr::Select(select) => dotted_path(select),
        _ => None,
    }
}

/// Normalize a dotted path to a `modules.<kind>.<name>` origin when it names a
/// flake module, ignoring anything else.
fn module_origin_from_path(path: &[String]) -> Option<String> {
    let comps: Vec<&str> = path.iter().map(String::as_str).collect();
    let (kind, name) = match comps.as_slice() {
        [base, "modules", kind, name] if *base == "self" || *base == "flake" => (*kind, *name),
        ["modules", kind, name] => (*kind, *name),
        _ => return None,
    };
    Some(format!("modules.{kind}.{name}"))
}

fn line_col(text: &str, offset: usize) -> (u32, u32) {
    let mut line = 1;
    let mut col = 1;
    for (i, ch) in text.char_indices() {
        if i >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}

/// The host directory a menu file sits in, if any: `<root>/hosts/<host>/...`.
fn menu_host_dir(file: &str) -> Option<String> {
    let parts: Vec<&str> = file.split('/').collect();
    parts
        .iter()
        .position(|p| *p == "hosts")
        .and_then(|idx| parts.get(idx + 1))
        .map(|s| s.to_string())
}

/// The set of every host name referenced by a `nixos:` target.
pub fn hosts_from_targets(targets: &[String]) -> Vec<String> {
    let mut hosts = Vec::new();
    let mut seen = HashSet::new();
    for target in targets {
        if let Some(host) = target.strip_prefix("nixos:") {
            if seen.insert(host.to_string()) {
                hosts.push(host.to_string());
            }
        }
    }
    hosts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(text: &str) -> FileScan {
        scan_file(Path::new("test.nix"), text)
    }

    #[test]
    fn scans_plain_module() {
        let text = r#"/* @menu displayname = "System"; order = 10; */

{ config, pkgs, ... }:

{
  /* @option widget = "text"; label = "Hostname"; */
  networking.hostName = "demo";

  services.openssh.enable = true;
}
"#;
        let scan = scan(text);
        assert_eq!(scan.spec.displayname.as_deref(), Some("System"));
        let paths: Vec<String> = scan.records.iter().map(Record::path).collect();
        assert_eq!(paths, vec!["networking.hostName"]);
        assert_eq!(scan.records[0].value, "\"demo\"");
        assert!(scan.records[0].comment_span.is_some());
    }

    #[test]
    fn marks_files_without_menu() {
        let with = scan("/* @menu displayname = \"System\"; */\n{ a = 1; }\n");
        assert!(with.has_menu);
        let without = scan("{ a = 1; }\n");
        assert!(!without.has_menu);
    }

    #[test]
    fn collects_inputs_markers_from_any_comment() {
        let text = r#"{ ... }: {
  # @inputs name = "nixvim"; url = "github:nix-community/nixvim";
  value = 1;
  /* @inputs name = "foo"; url = "https://example.com/foo"; */
}
"#;
        let scan = scan(text);
        let names: Vec<&str> = scan
            .declared_inputs
            .iter()
            .map(|input| input.name.as_str())
            .collect();
        assert_eq!(names, vec!["nixvim", "foo"]);
    }

    #[test]
    fn recovers_menu_and_records_from_malformed_file() {
        let text = r#"/* @menu displayname = "User"; */

{ config, ... }:

{
  config = {
    /* @option widget = "text"; label = "Description"; */
    description = "Primary account";
  };
"#;
        let scan = scan(text);
        assert!(scan.has_menu);
        assert_eq!(scan.spec.displayname.as_deref(), Some("User"));
        let paths: Vec<String> = scan.records.iter().map(Record::path).collect();
        assert_eq!(paths, vec!["config.description"]);
    }

    #[test]
    fn valid_file_scan_is_unchanged_by_best_effort_parse() {
        let text = r#"/* @menu displayname = "System"; */

{ config, ... }:

{
  /* @option widget = "text"; label = "Hostname"; */
  networking.hostName = "demo";
}
"#;
        let scan = scan(text);
        assert!(scan.has_menu);
        let paths: Vec<String> = scan.records.iter().map(Record::path).collect();
        assert_eq!(paths, vec!["networking.hostName"]);
    }

    #[test]
    fn flattens_containers_to_annotated_leaves() {
        let text = r#"{
  networking = {
    /* @option widget = "text"; label = "Hostname"; */
    hostName = "demo";
    firewall.enable = true;
  };
}
"#;
        let scan = scan(text);
        let paths: Vec<String> = scan.records.iter().map(Record::path).collect();
        assert_eq!(paths, vec!["networking.hostName"]);
    }

    #[test]
    fn keeps_annotated_subtree_whole() {
        let text = r#"{
  /* @option widget = "submodule"; label = "Firewall"; */
  networking.firewall = {
    enable = true;
    allowedTCPPorts = [ 22 80 ];
  };
}
"#;
        let scan = scan(text);
        assert_eq!(scan.records.len(), 1);
        assert_eq!(scan.records[0].path(), "networking.firewall");
        assert!(scan.records[0].value.contains("allowedTCPPorts"));
    }

    #[test]
    fn value_span_peels_with() {
        let text = r#"{
  /* @option widget = "packages"; */
  environment.systemPackages = with pkgs; [ git vim ];
}
"#;
        let scan = scan(text);
        assert_eq!(scan.records.len(), 1);
        assert_eq!(scan.records[0].value, "[ git vim ]");
        let span = &text[scan.records[0].byte_start..scan.records[0].byte_end];
        assert_eq!(span, "[ git vim ]");
    }

    #[test]
    fn captures_with_wrapper() {
        let text =
            "{\n  /* @option widget = \"packages\"; */\n  environment.systemPackages = with pkgs; [ git vim ];\n}\n";
        let wrapped = scan(text);
        assert_eq!(wrapped.records.len(), 1);
        assert_eq!(wrapped.records[0].wrapper.as_deref(), Some("with pkgs;"));
        let plain = scan("{\n  /* @option widget = \"list\"; */\n  a = [ 1 2 ];\n}\n");
        assert_eq!(plain.records[0].wrapper, None);
    }

    #[test]
    fn scans_dendritic_module() {
        let text = r#"{ self, inputs, ... }: {

  /* @menu displayname = "Audio"; order = 40; systems = [ "alpha" ]; */

  flake.modules.nixos.audio = { config, pkgs, ... }: {

    /* @option widget = "switch"; label = "Enable PipeWire"; */
    services.pipewire.enable = true;
  };
}
"#;
        let scan = scan(text);
        assert_eq!(scan.spec.displayname.as_deref(), Some("Audio"));
        assert_eq!(scan.spec.systems, vec!["alpha"]);
        assert_eq!(scan.records.len(), 1);
        assert_eq!(scan.records[0].path(), "services.pipewire.enable");
        assert_eq!(scan.records[0].origin, "modules.nixos.audio");
    }

    #[test]
    fn scans_dendritic_let_and_merge_body() {
        let text = r#"{ self, ... }: {
  flake.modules.nixos.mixed = { pkgs, ... }:
    let
      /* @option widget = "text"; label = "Extra"; */
      extra = "value";
      enabled = true;
    in
    {
      /* @option widget = "switch"; label = "Base"; */
      services.base.enable = true;
    } // (if enabled then {
      /* @option widget = "text"; label = "Theme"; */
      theme.name = "dark";
    } else { });
}
"#;
        let scan = scan(text);
        let paths: Vec<String> = scan.records.iter().map(Record::path).collect();
        assert!(paths.contains(&"extra".to_string()));
        assert!(paths.contains(&"services.base.enable".to_string()));
        assert!(paths.contains(&"theme.name".to_string()));
        assert!(!paths.contains(&"enabled".to_string()));
        assert!(scan
            .records
            .iter()
            .all(|r| r.origin == "modules.nixos.merge" || r.origin == "modules.nixos.mixed"));
    }

    #[test]
    fn submodule_value_with_let() {
        let text = r#"{ self, ... }: {
  flake.modules.nixos.sub = {
    /* @option widget = "submodule"; label = "Group"; */
    foo = let
      hidden = 1;
      /* @option widget = "switch"; label = "Nested"; */
      bar = true;
    in {
      baz = 2;
    };
  };
}
"#;
        let scan = scan(text);
        let foo = scan.records.iter().find(|r| r.path() == "foo").unwrap();
        let names: Vec<String> = scan_value(&foo.value).iter().map(Record::path).collect();
        assert!(names.contains(&"bar".to_string()));
        assert!(names.contains(&"baz".to_string()));
        assert!(!names.contains(&"hidden".to_string()));
    }

    #[test]
    fn records_host_targets() {
        let text = r#"{ self, inputs, ... }: {
  flake.nixosConfigurations.alpha = inputs.nixpkgs.lib.nixosSystem {
    modules = [ self.modules.nixos.common ];
  };
  flake.modules.nixos.common = { pkgs, ... }: {
    /* @option widget = "dropdown"; */
    time.timeZone = "Europe/Amsterdam";
  };
}
"#;
        let scan = scan(text);
        assert_eq!(scan.targets, vec!["nixos:alpha".to_string()]);
        assert_eq!(scan.records.len(), 1);
        assert_eq!(scan.records[0].path(), "time.timeZone");
        assert_eq!(scan.records[0].origin, "modules.nixos.common");
    }

    #[test]
    fn records_let_binding_comment() {
        let text = r#"{ pkgs, ... }:
let
  /* @option label = "Extra packages"; widget = "packages"; */
  extraPackages = [ pkgs.git pkgs.vim ];
in
{
  environment.systemPackages = extraPackages;
}
"#;
        let scan = scan(text);
        let paths: Vec<String> = scan.records.iter().map(Record::path).collect();
        assert_eq!(paths, vec!["extraPackages"]);
        assert!(scan.records[0].comment_span.is_some());
    }

    #[test]
    fn scans_single_file_fixture() {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/single-file");
        let result = scan_root(root).unwrap();
        assert_eq!(result.menus.len(), 1);
        let menu = &result.menus[0];
        assert_eq!(menu.display_name(), "System");
        let paths: Vec<String> = menu.records.iter().map(Record::path).collect();
        assert!(paths.contains(&"networking.hostName".to_string()));
        assert!(paths.contains(&"environment.systemPackages".to_string()));
        assert!(paths.contains(&"networking.firewall".to_string()));
        assert!(!paths.contains(&"networking.firewall.enable".to_string()));
    }

    #[test]
    fn scans_plain_flake_fixture() {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/plain-flake");
        let result = scan_root(root).unwrap();
        assert_eq!(result.menus.len(), 1);
        let menu = &result.menus[0];
        assert_eq!(menu.display_name(), "System");
        let paths: Vec<String> = menu.records.iter().map(Record::path).collect();
        assert!(paths.contains(&"networking.hostName".to_string()));
        assert!(paths.contains(&"environment.systemPackages".to_string()));
    }

    #[test]
    fn scans_dendritic_fixture() {
        let root = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/dendritic-flake"
        );
        let result = scan_root(root).unwrap();
        assert_eq!(
            result.targets,
            vec!["nixos:alpha".to_string(), "nixos:beta".to_string()]
        );

        let audio = result
            .menus
            .iter()
            .find(|m| m.display_name() == "Audio")
            .unwrap();
        assert_eq!(audio.spec.systems, vec!["alpha"]);
        assert!(audio
            .records
            .iter()
            .any(|r| r.path() == "services.pipewire.enable" && r.origin == "modules.nixos.audio"));

        let shared: Vec<String> = result
            .menus_for("beta")
            .iter()
            .map(|m| m.display_name())
            .collect();
        assert!(shared.contains(&"Common".to_string()));
        assert!(shared.contains(&"Desktop".to_string()));
        assert!(!shared.contains(&"Audio".to_string()));

        let merge = result
            .menus
            .iter()
            .find(|m| m.display_name() == "Merge")
            .unwrap();
        let merge_paths: Vec<String> = merge.records.iter().map(Record::path).collect();
        assert!(merge_paths.contains(&"extra".to_string()));
        assert!(merge_paths.contains(&"services.base.enable".to_string()));
        assert!(merge_paths.contains(&"theme.name".to_string()));
        assert!(merge
            .records
            .iter()
            .all(|r| r.origin == "modules.nixos.merge"));
    }

    #[test]
    fn scans_feature_widgets_fixture() {
        use crate::dsl::WidgetKind;

        let root = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/feature-widgets"
        );
        let result = scan_root(root).unwrap();
        assert_eq!(result.menus.len(), 1);

        // `systems = [ "all" ]` makes the menu visible for any host.
        let any_host: Vec<String> = result
            .menus_for("no-such-host")
            .iter()
            .map(|m| m.display_name())
            .collect();
        assert!(any_host.contains(&"Features".to_string()));

        let menu = &result.menus[0];
        let find = |path: &str| {
            menu.records
                .iter()
                .find(|r| r.path() == path)
                .unwrap_or_else(|| panic!("missing record {path}"))
        };

        let zeta = find("features.zetaFirst");
        assert_eq!(zeta.spec.group.as_deref(), Some("Zeta"));
        assert_eq!(zeta.spec.group_order, None);
        let unordered = find("features.alphaUnordered");
        assert_eq!(unordered.spec.group_order, Some(5));
        assert_eq!(unordered.spec.order, None);

        let sites = find("features.sites");
        assert_eq!(sites.spec.widget, Some(WidgetKind::TableAttrset));
        assert_eq!(
            sites.spec.columns,
            Some(vec!["enable".to_string(), "url".to_string()])
        );
        assert!(sites.spec.first_value_editable);
        assert!(!sites.spec.key_editable);

        let rules = find("features.rules");
        assert_eq!(rules.spec.widget, Some(WidgetKind::TableList));
        assert!(rules.spec.key_editable);

        let cards = find("features.cards");
        assert_eq!(cards.spec.widget, Some(WidgetKind::HorizontalSubmodule));
        assert_eq!(cards.spec.max_columns, Some(2));

        for (path, kind) in [
            ("features.binds.default", WidgetKind::Keybinds),
            ("features.monitor_rule", WidgetKind::Monitors),
            ("features.palette", WidgetKind::Theme),
            ("features.menu", WidgetKind::Menu),
            ("features.plugins", WidgetKind::Plugins),
            ("features.bar.default", WidgetKind::Bar),
            ("features.dockConfig", WidgetKind::Dock),
        ] {
            assert_eq!(find(path).spec.widget, Some(kind), "wrong kind at {path}");
        }

        // The nested submodule is kept whole; its children stay in the value.
        let nested = find("features.nested");
        assert_eq!(nested.spec.widget, Some(WidgetKind::Submodule));
        assert!(nested.value.contains("visibleChild"));
        assert!(nested.value.contains("hiddenChild"));
        assert!(nested.value.contains("deeper"));
    }

    #[test]
    fn scans_table_rows_and_item_specs() {
        let value = r#"{
  /* @table-item label = "Docs"; hidden = true; */
  docs = { enable = true; url = "x"; };
  plain = { enable = false; };
}"#;
        let rows = scan_table(value, false);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].key, "docs");
        assert_eq!(rows[0].spec.label.as_deref(), Some("Docs"));
        assert!(rows[0].spec.hidden);
        assert_eq!(rows[0].fields[0], ("enable".into(), "true".into()));
        assert_eq!(rows[0].fields[1], ("url".into(), "\"x\"".into()));
        assert_eq!(rows[1].key, "plain");
        assert!(rows[1].spec.label.is_none());
        assert!(!rows[1].spec.hidden);

        let list_rows = scan_table("[ { name = \"eDP\"; width = 1920; } ]", true);
        assert_eq!(list_rows.len(), 1);
        assert_eq!(list_rows[0].key, "");
        assert_eq!(list_rows[0].fields.len(), 2);
        assert_eq!(list_rows[0].fields[1], ("width".into(), "1920".into()));
    }

    #[test]
    fn scan_value_reads_inner_bindings() {
        let records = scan_value("{ enable = true; allowedTCPPorts = [ 22 ]; }");
        let paths: Vec<String> = records.iter().map(Record::path).collect();
        assert_eq!(paths, vec!["enable", "allowedTCPPorts"]);
        assert_eq!(records[0].value, "true");
    }

    #[test]
    fn scan_value_direct_keeps_nested_sets_whole() {
        let records = scan_value_direct("{ user = { col = 0; }; extra = 3; }");
        let paths: Vec<String> = records.iter().map(Record::path).collect();
        assert_eq!(paths, vec!["user", "extra"]);
        assert!(records[0].value.contains("col = 0"));
        assert_eq!(records[1].value, "3");
    }

    #[test]
    fn body_insert_offset_plain_module() {
        let text = "{ config, ... }: {\n  a = 1;\n}\n";
        let offset = body_insert_offset(text, "").unwrap();
        assert_eq!(&text[..offset], "{ config, ... }: {");
    }

    #[test]
    fn body_insert_offset_dendritic_module() {
        let text = "{ self, ... }: {\n  flake.modules.nixos.audio = { pkgs, ... }: {\n    a = 1;\n  };\n}\n";
        let offset = body_insert_offset(text, "modules.nixos.audio").unwrap();
        assert!(text[..offset].ends_with("flake.modules.nixos.audio = { pkgs, ... }: {"));
        assert!(text[offset..].starts_with("\n    a = 1;"));
    }

    #[test]
    fn body_insert_offset_merged_body() {
        let text = "{ self, ... }: {\n  flake.modules.nixos.mixed = { pkgs, ... }: let x = 1; in {\n    a = 1;\n  } // (if true then { b = 2; } else { });\n}\n";
        let offset = body_insert_offset(text, "modules.nixos.mixed").unwrap();
        assert!(text[..offset].ends_with(": let x = 1; in {"));
        assert!(text[offset..].starts_with("\n    a = 1;"));
    }

    #[test]
    fn menus_for_filters_shared_and_system() {
        let result = scan_root(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/dendritic-flake"
        ))
        .unwrap();
        let alpha: Vec<String> = result
            .menus_for("alpha")
            .iter()
            .map(|m| m.display_name())
            .collect();
        assert!(alpha.contains(&"Audio".to_string()));
        assert!(alpha.contains(&"Common".to_string()));
        let beta: Vec<String> = result
            .menus_for("beta")
            .iter()
            .map(|m| m.display_name())
            .collect();
        assert!(!beta.contains(&"Audio".to_string()));
        assert!(beta.contains(&"Common".to_string()));
    }

    #[test]
    fn collects_module_origins_and_imports() {
        let text = r#"{ self, ... }: {
  flake.modules.nixos.common = { ... }: {
    /* @option widget = "module-list"; */
    imports = [ self.modules.nixos.audio ];
    time.timeZone = "UTC";
  };
  flake.nixosConfigurations.alpha = {
    modules = [ self.modules.nixos.common ];
  };
}
"#;
        let scan = scan(text);
        assert_eq!(scan.origins, vec!["modules.nixos.common".to_string()]);
        assert_eq!(scan.imports, vec!["modules.nixos.audio".to_string()]);
        assert_eq!(
            scan.target_imports.get("nixos:alpha").cloned().unwrap(),
            vec!["modules.nixos.common".to_string()]
        );

        let imports = scan
            .records
            .iter()
            .find(|r| r.segments == ["imports"])
            .unwrap();
        assert_eq!(imports.origin, "modules.nixos.common");
        assert_eq!(imports.path(), "imports");
    }

    #[test]
    fn collects_modules_from_with_lists() {
        let text = r#"{ self, ... }: {
  flake.modules.nixos.base = { ... }: {
    imports = with self.modules.nixos; [ common audio ];
    home-manager.sharedModules = with self.modules.homeManager; [ desktop ];
  };
  flake.nixosConfigurations.alpha = {
    modules = [ self.modules.nixos.base ];
  };
}
"#;
        let scan = scan(text);
        assert_eq!(
            scan.imports,
            vec![
                "modules.nixos.common".to_string(),
                "modules.nixos.audio".to_string(),
                "modules.homeManager.desktop".to_string(),
            ]
        );
    }

    #[test]
    fn resolves_host_module_graph() {
        let result = scan_root(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/dendritic-flake"
        ))
        .unwrap();

        let alpha = result.resolve_host("nixos:alpha").unwrap();
        assert!(alpha.contains("modules.nixos.common"));
        assert!(alpha.contains("modules.nixos.audio"));
        assert!(alpha.contains("modules.homeManager.desktop"));

        let beta = result.resolve_host("nixos:beta").unwrap();
        assert!(beta.contains("modules.nixos.common"));
        assert!(beta.contains("modules.homeManager.desktop"));
        assert!(!beta.contains("modules.nixos.audio"));

        assert!(result.resolve_host("nixos:missing").is_none());
    }

    #[test]
    fn menus_for_hides_unreachable_origins() {
        let result = scan_root(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/dendritic-flake"
        ))
        .unwrap();
        let alpha: Vec<String> = result
            .menus_for("alpha")
            .iter()
            .map(|m| m.display_name())
            .collect();
        assert!(!alpha.contains(&"Unused".to_string()));
        assert!(alpha.contains(&"Audio".to_string()));
    }

    #[test]
    fn finds_snippet_and_insert_markers() {
        let text = r#"/* @menu displayname = "Keys"; */

/* @snippet
   name = "keybind";
   label = "Add keybind";
   snippet = ''
     # @option widget = "text"; label = "Action";
     keybinds."%%keys%%" = "%%action%%";
   '';
   placeholders.keys.label = "Keys";
*/

{ config, ... }:

{
  /* @insert
     name = "keybind";
     group = "Keybinds";
     order = 10;
  */
}
"#;
        let scan = scan(text);
        assert_eq!(scan.snippets.len(), 1);
        let snippet = &scan.snippets[0];
        assert_eq!(snippet.name, "keybind");
        assert_eq!(snippet.placeholders[0].label, "Keys");
        assert_eq!(scan.inserts.len(), 1);
        let marker = &scan.inserts[0];
        assert_eq!(marker.name, "keybind");
        assert_eq!(marker.group.as_deref(), Some("Keybinds"));
        assert_eq!(marker.order, Some(10));
        let marker_line = text[..marker.offset].matches('\n').count() + 1;
        assert_eq!(marker_line as u32, marker.line);
    }

    #[test]
    fn ignores_incomplete_markers() {
        let text = "/* @snippet label = \"No name\"; */\n{ a = 1; }\n";
        let scan = scan(text);
        assert!(scan.snippets.is_empty());
    }
}
