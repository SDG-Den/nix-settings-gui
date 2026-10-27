use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::path::Path;
use std::process::Command;

use rnix::ast::{Attr, AttrSet, AttrpathValue, Expr, HasEntry, InterpolPart};
use rnix::{NodeOrToken, SyntaxKind};
use rowan::ast::AstNode;
use serde_json::Value;

use crate::dsl::{self, MarkerKind};
use crate::edit::PendingEdit;

/// One top-level flake input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlakeInput {
    pub name: String,
    pub url: Option<String>,
    pub flake: bool,
    pub submodules: bool,
    pub lfs: bool,
    /// Nested input name -> the current flake's input it follows.
    pub follows: BTreeMap<String, String>,
    pub description: Option<String>,
}

impl FlakeInput {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            url: None,
            flake: true,
            submodules: false,
            lfs: false,
            follows: BTreeMap::new(),
            description: None,
        }
    }
}

/// The parsed `inputs` set of one `flake.nix`, plus the source spans needed to
/// edit it.
pub struct InputDocument {
    pub path: String,
    pub inputs: Vec<FlakeInput>,
    groups: BTreeMap<String, InputGroup>,
    insert_offset: usize,
    wrap_inputs: bool,
}

/// One `@inputs` declaration found at the top of a repository file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredInput {
    pub name: String,
    pub url: String,
    pub file: String,
    pub line: usize,
}

#[derive(Default)]
struct InputGroup {
    bindings: Vec<Range<usize>>,
    description_span: Option<Range<usize>>,
    description: Option<String>,
}

pub fn load(path: &str) -> Result<InputDocument, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    let root = rnix::Root::parse(&text)
        .ok()
        .map_err(|e| format!("cannot parse {path}: {e}"))?;
    let expr = root
        .expr()
        .ok_or_else(|| format!("{path} has no expression"))?;
    let top = peel_body(&expr).ok_or_else(|| format!("{path} is not an attribute set"))?;
    let Expr::AttrSet(top) = top else {
        return Err(format!("{path} is not an attribute set"));
    };

    let mut inputs_set: Option<AttrSet> = None;
    for binding in top.attrpath_values() {
        let Some(path) = binding.attrpath() else {
            continue;
        };
        let comps = attr_components(&path);
        if comps.first().map(String::as_str) != Some("inputs") {
            continue;
        }
        if comps.len() != 1 {
            return Err(format!("{path}: inputs is not a literal attribute set"));
        }
        let Some(value) = binding.value() else {
            continue;
        };
        let Some(Expr::AttrSet(set)) = peel_value(&value) else {
            return Err(format!("{path}: inputs is not a literal attribute set"));
        };
        inputs_set = Some(set);
        break;
    }
    let (insert_offset, wrap_inputs) = match &inputs_set {
        Some(set) => {
            let offset = set
                .r_curly_token()
                .map(|token| usize::from(token.text_range().start()))
                .ok_or_else(|| format!("{path}: cannot locate the end of the inputs set"))?;
            (offset, false)
        }
        None => {
            let offset = top
                .l_curly_token()
                .map(|token| usize::from(token.text_range().end()))
                .ok_or_else(|| format!("{path}: cannot locate the top-level attribute set"))?;
            (offset, true)
        }
    };

    let mut groups: BTreeMap<String, InputGroup> = BTreeMap::new();
    let mut inputs: BTreeMap<String, FlakeInput> = BTreeMap::new();
    if let Some(set) = &inputs_set {
        let mut pending: Option<(String, Range<usize>)> = None;
        for element in set.syntax().children_with_tokens() {
            match element {
                NodeOrToken::Token(token) => match token.kind() {
                    SyntaxKind::TOKEN_COMMENT => {
                        let range = token.text_range();
                        pending = Some((
                            token.text().to_string(),
                            usize::from(range.start())..usize::from(range.end()),
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
                    let Some(attrpath) = binding.attrpath() else {
                        continue;
                    };
                    let comps = attr_components(&attrpath);
                    let Some(name) = comps.first().cloned() else {
                        continue;
                    };
                    if name == "self" {
                        continue;
                    }
                    let range = binding.syntax().text_range();
                    let group = groups.entry(name.clone()).or_default();
                    if group.bindings.is_empty() {
                        if let Some((text, span)) = comment {
                            if let Some(description) = parse_input_description(&text) {
                                group.description = Some(description);
                                group.description_span = Some(span);
                            }
                        }
                    }
                    group
                        .bindings
                        .push(usize::from(range.start())..usize::from(range.end()));

                    let input = inputs
                        .entry(name.clone())
                        .or_insert_with(|| FlakeInput::new(name));
                    if let Some(value) = binding.value() {
                        apply_path(input, &comps[1..], &value);
                    }
                }
            }
        }
    }

    for (name, group) in &groups {
        if let Some(input) = inputs.get_mut(name) {
            input.description = group.description.clone();
        }
    }
    for input in inputs.values_mut() {
        if let Some(url) = input.url.as_deref() {
            input.submodules |= url_flag(url, "submodules");
            input.lfs |= url_flag(url, "lfs");
        }
    }

    Ok(InputDocument {
        path: path.to_string(),
        inputs: inputs.into_values().collect(),
        groups,
        insert_offset,
        wrap_inputs,
    })
}

impl InputDocument {
    /// Edits that add `input` or replace the existing entry with the same name.
    pub fn edits_for_upsert(&self, input: &FlakeInput) -> Result<Vec<PendingEdit>, String> {
        if input.name.trim().is_empty() {
            return Err("input name cannot be empty".to_string());
        }
        if input.url.as_deref().unwrap_or("").trim().is_empty() {
            return Err("input URL cannot be empty".to_string());
        }
        let marker = render_description(input.description.as_deref());
        let block = render_block(input);

        let Some(group) = self.groups.get(&input.name) else {
            let mut rendered = String::new();
            if let Some(marker) = &marker {
                rendered.push_str(marker);
                rendered.push('\n');
            }
            rendered.push_str(&block);
            return Ok(self.insert_blocks(vec![rendered]));
        };

        let Some(first) = group.bindings.first().cloned() else {
            return Err(format!("input `{}` has no source binding", input.name));
        };
        let mut edits = Vec::new();
        if let Some(span) = &group.description_span {
            edits.push(pending(
                &self.path,
                span.clone(),
                marker.clone().unwrap_or_default(),
            ));
        }
        let first_value = if group.description_span.is_some() {
            block
        } else {
            match &marker {
                Some(marker) => format!("{marker}\n{block}"),
                None => block,
            }
        };
        edits.push(pending(&self.path, first, first_value));
        for span in group.bindings.iter().skip(1) {
            edits.push(pending(&self.path, span.clone(), String::new()));
        }
        Ok(edits)
    }

    /// Edits that add every input not already declared in the file. All new
    /// entries are inserted as one block.
    pub fn edits_for_add_all(&self, inputs: &[FlakeInput]) -> Result<Vec<PendingEdit>, String> {
        let mut blocks = Vec::new();
        let mut seen = BTreeSet::new();
        for input in inputs {
            if input.name.trim().is_empty()
                || input.url.as_deref().unwrap_or("").trim().is_empty()
                || self.groups.contains_key(&input.name)
                || !seen.insert(input.name.clone())
            {
                continue;
            }
            let mut rendered = String::new();
            if let Some(marker) = render_description(input.description.as_deref()) {
                rendered.push_str(&marker);
                rendered.push('\n');
            }
            rendered.push_str(&render_block(input));
            blocks.push(rendered);
        }
        if blocks.is_empty() {
            return Err("no missing inputs to add".to_string());
        }
        Ok(self.insert_blocks(blocks))
    }

    fn insert_blocks(&self, blocks: Vec<String>) -> Vec<PendingEdit> {
        let body = blocks.join("");
        let value = if self.wrap_inputs {
            format!("\ninputs = {{\n{body}}};\n")
        } else {
            format!("\n{body}\n")
        };
        vec![pending(
            &self.path,
            self.insert_offset..self.insert_offset,
            value,
        )]
    }

    /// Edits that remove every binding and the description marker for `name`.
    pub fn edits_for_delete(&self, name: &str) -> Result<Vec<PendingEdit>, String> {
        let Some(group) = self.groups.get(name) else {
            return Err(format!("input `{name}` was not found"));
        };
        let mut edits = Vec::new();
        if let Some(span) = &group.description_span {
            edits.push(pending(&self.path, span.clone(), String::new()));
        }
        for span in &group.bindings {
            edits.push(pending(&self.path, span.clone(), String::new()));
        }
        Ok(edits)
    }
}

pub(crate) fn declared_inputs_from_syntax(
    text: &str,
    syntax: &rnix::SyntaxNode,
    file: &str,
) -> Vec<DeclaredInput> {
    let mut out = Vec::new();
    for element in syntax.descendants_with_tokens() {
        let Some(token) = element.into_token() else {
            continue;
        };
        if token.kind() != SyntaxKind::TOKEN_COMMENT {
            continue;
        }
        let Some((MarkerKind::Inputs, directives)) = dsl::parse_comment(token.text()) else {
            continue;
        };
        let Some(name) = directives.string("name") else {
            continue;
        };
        let Some(url) = directives.string("url") else {
            continue;
        };
        if name.trim().is_empty() || url.trim().is_empty() {
            continue;
        }
        let offset = usize::from(token.text_range().start());
        let line = text[..offset].bytes().filter(|byte| *byte == b'\n').count() + 1;
        out.push(DeclaredInput {
            name: name.trim().to_string(),
            url: url.trim().to_string(),
            file: file.to_string(),
            line,
        });
    }
    out
}

/// Declared inputs that have no matching name in the flake's current inputs.
pub fn missing_declared_inputs(
    declared: &[DeclaredInput],
    existing: &[FlakeInput],
) -> Vec<DeclaredInput> {
    let existing: BTreeSet<&str> = existing.iter().map(|input| input.name.as_str()).collect();
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for input in declared {
        if input.name == "self"
            || existing.contains(input.name.as_str())
            || !seen.insert(input.name.clone())
        {
            continue;
        }
        out.push(input.clone());
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn pending(file: &str, span: Range<usize>, value: String) -> PendingEdit {
    PendingEdit {
        file: file.to_string(),
        start: span.start,
        end: span.end,
        value,
    }
}

fn render_block(input: &FlakeInput) -> String {
    let mut out = format!("{} = {{\n", attr_key(&input.name));
    if let Some(url) = input.url.as_deref() {
        let url = set_url_flag(
            &set_url_flag(url, "submodules", input.submodules),
            "lfs",
            input.lfs,
        );
        out.push_str(&format!("  url = \"{}\";\n", escape_nix_string(&url)));
    }
    if !input.flake {
        out.push_str("  flake = false;\n");
    }
    for (external, local) in &input.follows {
        if local.is_empty() {
            continue;
        }
        out.push_str(&format!(
            "  inputs.{}.follows = \"{}\";\n",
            attr_key(external),
            escape_nix_string(local)
        ));
    }
    out.push_str("};\n");
    out
}

fn render_description(description: Option<&str>) -> Option<String> {
    let description = description?.trim();
    if description.is_empty() {
        return None;
    }
    Some(format!(
        "/* @input description = \"{}\"; */",
        escape_nix_string(description)
    ))
}

fn parse_input_description(comment: &str) -> Option<String> {
    let inner = comment
        .trim()
        .strip_prefix("/*")?
        .strip_suffix("*/")?
        .trim();
    let rest = inner.strip_prefix("@input")?.trim();
    let rest = rest.strip_prefix("description")?.trim();
    let rest = rest.strip_prefix('=')?.trim();
    let rest = rest.strip_suffix(';')?.trim();
    parse_nix_string(rest)
}

fn apply_path(input: &mut FlakeInput, path: &[String], value: &Expr) {
    if path.is_empty() {
        if let Some(Expr::AttrSet(set)) = peel_value(value) {
            for binding in set.attrpath_values() {
                let Some(attrpath) = binding.attrpath() else {
                    continue;
                };
                let comps = attr_components(&attrpath);
                if let Some(value) = binding.value() {
                    apply_path(input, &comps, &value);
                }
            }
        }
        return;
    }
    match path {
        [key] if key == "url" => {
            if let Some(url) = string_expr(value) {
                input.url = Some(url);
            }
        }
        [key] if key == "flake" => {
            if let Some(flake) = bool_expr(value) {
                input.flake = flake;
            }
        }
        [key] if key == "submodules" => {
            if let Some(enabled) = bool_expr(value) {
                input.submodules = enabled;
            }
        }
        [key] if key == "lfs" => {
            if let Some(enabled) = bool_expr(value) {
                input.lfs = enabled;
            }
        }
        [inputs, external, follows] if inputs == "inputs" && follows == "follows" => {
            if let Some(local) = string_expr(value) {
                input.follows.insert(external.clone(), local);
            }
        }
        [inputs] if inputs == "inputs" => {
            if let Some(Expr::AttrSet(set)) = peel_value(value) {
                for binding in set.attrpath_values() {
                    let Some(attrpath) = binding.attrpath() else {
                        continue;
                    };
                    let comps = attr_components(&attrpath);
                    if let Some(value) = binding.value() {
                        apply_path(input, &comps, &value);
                    }
                }
            }
        }
        _ => {}
    }
}

fn attr_components(attrpath: &rnix::ast::Attrpath) -> Vec<String> {
    attrpath
        .attrs()
        .filter_map(|attr| attr_component(&attr))
        .collect()
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

fn string_expr(expr: &Expr) -> Option<String> {
    let Expr::Str(value) = expr else {
        return None;
    };
    match value.normalized_parts().as_slice() {
        [InterpolPart::Literal(text)] => Some(text.clone()),
        _ => None,
    }
}

fn bool_expr(expr: &Expr) -> Option<bool> {
    let Expr::Ident(ident) = expr else {
        return None;
    };
    match ident.syntax().text().to_string().as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

fn parse_nix_string(text: &str) -> Option<String> {
    let root = rnix::Root::parse(text).ok().ok()?;
    string_expr(&root.expr()?)
}

fn peel_body(expr: &Expr) -> Option<Expr> {
    let mut current = expr.clone();
    loop {
        current = match current {
            Expr::Lambda(lambda) => lambda.body()?,
            Expr::LetIn(let_in) => let_in.body()?,
            Expr::With(with) => with.body()?,
            Expr::Paren(paren) => paren.expr()?,
            other => return Some(other),
        };
    }
}

fn peel_value(expr: &Expr) -> Option<Expr> {
    let mut current = expr.clone();
    loop {
        current = match current {
            Expr::With(with) => with.body()?,
            Expr::LetIn(let_in) => let_in.body()?,
            Expr::Paren(paren) => paren.expr()?,
            other => return Some(other),
        };
    }
}

fn attr_key(name: &str) -> String {
    let mut chars = name.chars();
    let valid_start = chars
        .next()
        .map(|c| c.is_ascii_alphabetic() || c == '_')
        .unwrap_or(false);
    let valid_rest = name
        .chars()
        .skip(1)
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '\''));
    if valid_start && valid_rest {
        name.to_string()
    } else {
        format!("\"{}\"", escape_nix_string(name))
    }
}

fn escape_nix_string(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace("${", "\\${")
}

/// Whether a flake URL carries `flag=1` in its query string.
pub fn url_flag(url: &str, flag: &str) -> bool {
    let Some(query) = url
        .split('#')
        .next()
        .and_then(|url| url.split_once('?').map(|(_, query)| query))
    else {
        return false;
    };
    query.split('&').any(|parameter| {
        let mut parts = parameter.splitn(2, '=');
        if parts.next() != Some(flag) {
            return false;
        }
        matches!(parts.next(), None | Some("1") | Some("true"))
    })
}

/// Add or remove `flag=1` in a flake URL's query string.
pub fn set_url_flag(url: &str, flag: &str, enabled: bool) -> String {
    let (without_fragment, fragment) = match url.split_once('#') {
        Some((url, fragment)) => (url, Some(fragment)),
        None => (url, None),
    };
    let (base, query) = match without_fragment.split_once('?') {
        Some((base, query)) => (base, Some(query)),
        None => (without_fragment, None),
    };
    let mut parameters: Vec<String> = query
        .map(|query| {
            query
                .split('&')
                .filter(|parameter| {
                    !parameter.is_empty()
                        && parameter.split('=').next().unwrap_or(parameter) != flag
                })
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    if enabled {
        parameters.push(format!("{flag}=1"));
    }
    let mut out = base.to_string();
    if !parameters.is_empty() {
        out.push('?');
        out.push_str(&parameters.join("&"));
    }
    if let Some(fragment) = fragment {
        out.push('#');
        out.push_str(fragment);
    }
    out
}

/// Fetch the direct input names of the flake at `url`. Runs `nix` and returns
/// its stderr on failure.
pub fn fetch_nested_inputs(base: &str, url: &str) -> Result<Vec<String>, String> {
    if url.trim().is_empty() {
        return Err("input URL is empty".to_string());
    }
    let mut command = Command::new("nix");
    command.args([
        "flake",
        "metadata",
        "--json",
        "--no-write-lock-file",
        url.trim(),
    ]);
    if !base.is_empty() {
        command.current_dir(Path::new(base));
    }
    let output = command
        .output()
        .map_err(|e| format!("cannot run nix flake metadata: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("nix flake metadata failed");
        return Err(detail.trim().to_string());
    }
    let value: Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("cannot parse nix flake metadata output: {e}"))?;
    nested_input_names(&value)
}

fn nested_input_names(value: &Value) -> Result<Vec<String>, String> {
    let locks = value
        .get("locks")
        .ok_or_else(|| "flake metadata has no lock data".to_string())?;
    let root = locks.get("root").and_then(Value::as_str).unwrap_or("root");
    let nodes = locks
        .get("nodes")
        .ok_or_else(|| "flake metadata has no lock nodes".to_string())?;
    let node = nodes
        .get(root)
        .ok_or_else(|| "flake metadata has no root node".to_string())?;
    let mut names: Vec<String> = node
        .get("inputs")
        .and_then(Value::as_object)
        .map(|inputs| inputs.keys().cloned().collect())
        .unwrap_or_default();
    names.sort();
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FLAKE: &str = r#"{
  description = "test";
  inputs = {
    /* @input description = "Nix packages"; */
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";

    home-manager = {
      url = "github:nix-community/home-manager";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    disko.inputs.nixpkgs.follows = "nixpkgs";
    disko.url = "github:nix-community/disko";
  };
  outputs = { ... }: {};
}"#;

    fn document(name: &str, text: &str) -> InputDocument {
        let dir = std::env::temp_dir().join(format!("nsg-inputs-{}-{}", std::process::id(), name));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("flake.nix");
        std::fs::write(&path, text).unwrap();
        load(path.to_str().unwrap()).unwrap()
    }

    fn patch(text: &str, edits: &[PendingEdit]) -> String {
        let mut ordered = edits.to_vec();
        ordered.sort_by_key(|edit| std::cmp::Reverse(edit.start));
        let mut out = text.to_string();
        for edit in ordered {
            out.replace_range(edit.start..edit.end, &edit.value);
        }
        out
    }

    fn input<'a>(doc: &'a InputDocument, name: &str) -> &'a FlakeInput {
        doc.inputs.iter().find(|input| input.name == name).unwrap()
    }

    #[test]
    fn parses_block_and_dotted_inputs() {
        let doc = document("parse", FLAKE);
        let nixpkgs = input(&doc, "nixpkgs");
        assert_eq!(
            nixpkgs.url.as_deref(),
            Some("github:nixos/nixpkgs/nixos-unstable")
        );
        assert_eq!(nixpkgs.description.as_deref(), Some("Nix packages"));
        let home = input(&doc, "home-manager");
        assert_eq!(
            home.follows.get("nixpkgs").map(String::as_str),
            Some("nixpkgs")
        );
        let disko = input(&doc, "disko");
        assert_eq!(disko.url.as_deref(), Some("github:nix-community/disko"));
        assert_eq!(
            disko.follows.get("nixpkgs").map(String::as_str),
            Some("nixpkgs")
        );
    }

    #[test]
    fn updates_an_existing_input_in_place() {
        let doc = document("update", FLAKE);
        let mut updated = input(&doc, "home-manager").clone();
        updated.url = Some("github:new/home-manager".to_string());
        updated.description = Some("Home manager".to_string());
        updated
            .follows
            .insert("nixpkgs".to_string(), "nixpkgs".to_string());
        let edits = doc.edits_for_upsert(&updated).unwrap();
        let patched = patch(FLAKE, &edits);
        assert!(rnix::Root::parse(&patched).ok().is_ok());
        assert!(patched.contains("github:new/home-manager"));
        assert!(patched.contains("@input description = \"Home manager\""));
        assert!(patched.contains("inputs.nixpkgs.follows = \"nixpkgs\""));
    }

    #[test]
    fn adds_and_deletes_an_input() {
        let doc = document("add-delete", FLAKE);
        let mut added = FlakeInput::new("foo");
        added.url = Some("github:owner/foo?lfs=1".to_string());
        added.submodules = true;
        added.lfs = true;
        let edits = doc.edits_for_upsert(&added).unwrap();
        let patched = patch(FLAKE, &edits);
        assert!(rnix::Root::parse(&patched).ok().is_ok());
        assert!(patched.contains("foo = {"));
        assert!(patched.contains("submodules=1"));
        assert!(patched.contains("lfs=1"));

        let doc = document("add-delete-2", &patched);
        let edits = doc.edits_for_delete("disko").unwrap();
        let patched = patch(&patched, &edits);
        assert!(rnix::Root::parse(&patched).ok().is_ok());
        assert!(!patched.contains("disko"));
    }

    #[test]
    fn toggles_url_flags_without_touching_other_parameters() {
        let url = "github:owner/repo?dir=pkgs&submodules=1#frag";
        let updated = set_url_flag(url, "submodules", false);
        assert_eq!(updated, "github:owner/repo?dir=pkgs#frag");
        let updated = set_url_flag(&updated, "lfs", true);
        assert_eq!(updated, "github:owner/repo?dir=pkgs&lfs=1#frag");
        assert!(url_flag(&updated, "lfs"));
        assert!(!url_flag(&updated, "submodules"));
    }

    #[test]
    fn reads_declared_inputs_from_comments_anywhere() {
        let text = r#"{ ... }: {
  # @inputs name = "foo"; url = "https://example.com/foo";
  note = ''
    # @inputs name = "inside"; url = "https://example.com/nope";
  '';
  /* @inputs name = "bar"; url = "github:owner/bar"; */
  value = 1;
}
"#;
        let parsed = rnix::Root::parse(text);
        let declared = declared_inputs_from_syntax(text, &parsed.syntax(), "module.nix");
        let names: Vec<&str> = declared.iter().map(|input| input.name.as_str()).collect();
        assert_eq!(names, vec!["foo", "bar"]);
        assert_eq!(declared[0].line, 2);
    }

    #[test]
    fn finds_only_missing_declared_inputs() {
        let declared = vec![
            DeclaredInput {
                name: "nixpkgs".to_string(),
                url: "github:nixos/nixpkgs".to_string(),
                file: "a.nix".to_string(),
                line: 1,
            },
            DeclaredInput {
                name: "foo".to_string(),
                url: "https://example.com/foo".to_string(),
                file: "b.nix".to_string(),
                line: 1,
            },
            DeclaredInput {
                name: "foo".to_string(),
                url: "https://example.com/other".to_string(),
                file: "c.nix".to_string(),
                line: 1,
            },
            DeclaredInput {
                name: "bar".to_string(),
                url: "https://example.com/bar".to_string(),
                file: "d.nix".to_string(),
                line: 1,
            },
        ];
        let missing = missing_declared_inputs(&declared, &[FlakeInput::new("nixpkgs")]);
        let names: Vec<&str> = missing.iter().map(|input| input.name.as_str()).collect();
        assert_eq!(names, vec!["bar", "foo"]);
    }

    #[test]
    fn adds_all_missing_inputs_in_one_edit() {
        let doc = document("add-all", FLAKE);
        let mut foo = FlakeInput::new("foo");
        foo.url = Some("https://example.com/foo".to_string());
        let mut bar = FlakeInput::new("bar");
        bar.url = Some("https://example.com/bar".to_string());
        let mut inputs = vec![foo, bar, input(&doc, "nixpkgs").clone()];
        let edits = doc.edits_for_add_all(&inputs).unwrap();
        assert_eq!(edits.len(), 1);
        let patched = patch(FLAKE, &edits);
        assert!(rnix::Root::parse(&patched).ok().is_ok());
        assert!(patched.contains("foo = {"));
        assert!(patched.contains("bar = {"));
        inputs.clear();
    }

    #[test]
    fn creates_inputs_set_when_missing() {
        let text = "{\n  outputs = { ... }: {};\n}\n";
        let doc = document("wrap", text);
        assert!(doc.inputs.is_empty());
        let mut foo = FlakeInput::new("foo");
        foo.url = Some("https://example.com/foo".to_string());
        let edits = doc.edits_for_upsert(&foo).unwrap();
        let patched = patch(text, &edits);
        assert!(rnix::Root::parse(&patched).ok().is_ok());
        assert!(patched.contains("inputs = {"));
        assert!(patched.contains("https://example.com/foo"));
    }

    #[test]
    fn reads_direct_inputs_from_metadata() {
        let value: Value = serde_json::from_str(
            r#"{
              "locks": {
                "root": "root",
                "nodes": {
                  "root": {"inputs": {"nixpkgs": "nixpkgs", "foo": ["bar"]}}
                }
              }
            }"#,
        )
        .unwrap();
        assert_eq!(
            nested_input_names(&value).unwrap(),
            vec!["foo".to_string(), "nixpkgs".to_string()]
        );
    }
}
