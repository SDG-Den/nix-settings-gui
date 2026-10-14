use std::collections::HashSet;
use std::path::{Path, PathBuf};

use rnix::ast::{Attr, AttrSet, AttrpathValue, Expr, HasEntry, InterpolPart};
use rnix::{NodeOrToken, SyntaxKind, SyntaxNode};
use rowan::ast::AstNode;

use crate::dsl::{self, MarkerKind, MenuSpec, OptionSpec};
use crate::model::{MenuModel, Record};

/// Everything the scanner learns from one file.
#[derive(Debug, Clone, Default)]
pub struct FileScan {
    pub file: String,
    pub spec: MenuSpec,
    pub records: Vec<Record>,
    pub targets: Vec<String>,
}

/// The whole scan of a config tree.
#[derive(Debug, Clone, Default)]
pub struct ScanResult {
    pub menus: Vec<MenuModel>,
    pub targets: Vec<String>,
}

impl ScanResult {
    /// Menu filenames restricted to one host. A shared menu (no `systems`)
    /// always shows; a host directory name acts as a fallback when `systems`
    /// is absent but the file sits under `hosts/<host>/`.
    pub fn menus_for(&self, host: &str) -> Vec<&MenuModel> {
        self.menus
            .iter()
            .filter(|menu| {
                menu.spec.systems.is_empty() || menu.spec.systems.iter().any(|s| s == host)
            })
            .filter(|menu| {
                if !menu.spec.systems.is_empty() {
                    return true;
                }
                match menu_host_dir(&menu.file) {
                    Some(dir) => dir == host,
                    None => true,
                }
            })
            .collect()
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
        result.targets.extend(scan.targets);
        result.menus.push(MenuModel {
            file: scan.file,
            spec: scan.spec,
            records: scan.records,
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

    let Ok(root) = rnix::Root::parse(text).ok() else {
        return scan;
    };
    scan.spec = find_menu(root.syntax());

    let Some(expr) = root.expr() else {
        return scan;
    };

    if let Some(set) = top_attrset(&expr) {
        if has_flake_container(&set) {
            scan_dendritic(text, &file, &set, &mut scan.records, &mut scan.targets);
            return scan;
        }
    }

    walk_body(text, &file, "", &expr, &mut scan.records);
    scan
}

/// Scan a value literal on its own, e.g. the inner attribute set of a
/// submodule control. Returned spans are relative to `text`, so a caller adds
/// the parent value's file offset to turn them into whole-file spans.
pub fn scan_value(text: &str) -> Vec<Record> {
    let Ok(root) = rnix::Root::parse(text).ok() else {
        return Vec::new();
    };
    let Some(expr) = root.expr() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    walk_body(text, "", "", &expr, &mut out);
    out
}

/// Byte offset just after the opening brace of the module body selected by
/// `origin`: the top-level body for a plain file, or the
/// `flake.modules.<kind>.<name>` body for a dendritic file.
pub fn body_insert_offset(text: &str, origin: &str) -> Option<usize> {
    let root = rnix::Root::parse(text).ok().ok()?;
    let expr = root.expr()?;

    if origin.is_empty() {
        return match peel_body(&expr)? {
            Expr::AttrSet(set) => brace_offset(&set),
            _ => None,
        };
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
            if let Expr::AttrSet(set) = peel_body(&value)? {
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

/// The first `@menu` comment anywhere in the file is the menu header.
fn find_menu(root: &SyntaxNode) -> MenuSpec {
    for element in root.descendants_with_tokens() {
        if let Some(token) = element.into_token() {
            if token.kind() == SyntaxKind::TOKEN_COMMENT {
                if let Some((MarkerKind::Menu, directives)) = dsl::parse_comment(token.text()) {
                    return MenuSpec::from_directives(&directives);
                }
            }
        }
    }
    MenuSpec::default()
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
fn scan_dendritic(
    text: &str,
    file: &str,
    set: &AttrSet,
    out: &mut Vec<Record>,
    targets: &mut Vec<String>,
) {
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
            let Some(value) = binding.value() else {
                continue;
            };
            if let Some(Expr::AttrSet(body)) = peel_body(&value) {
                collect_container(text, file, &origin, body.syntax(), &[], out);
            }
            continue;
        }

        match comps[1].as_str() {
            "nixosConfigurations" => targets.push(format!("nixos:{}", comps[2])),
            "homeConfigurations" => targets.push(format!("home:{}", comps[2])),
            "darwinConfigurations" => targets.push(format!("darwin:{}", comps[2])),
            _ => {}
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
/// bindings carry their own comments, so they are collected before the body.
fn walk_body(text: &str, file: &str, origin: &str, expr: &Expr, out: &mut Vec<Record>) {
    match expr {
        Expr::Lambda(l) => {
            if let Some(body) = l.body() {
                walk_body(text, file, origin, &body, out);
            }
        }
        Expr::LetIn(l) => {
            collect_container(text, file, origin, l.syntax(), &[], out);
            if let Some(body) = l.body() {
                walk_body(text, file, origin, &body, out);
            }
        }
        Expr::With(w) => {
            if let Some(body) = w.body() {
                walk_body(text, file, origin, &body, out);
            }
        }
        Expr::Paren(p) => {
            if let Some(inner) = p.expr() {
                walk_body(text, file, origin, &inner, out);
            }
        }
        Expr::AttrSet(set) => collect_container(text, file, origin, set.syntax(), &[], out),
        _ => {}
    }
}

/// Walk the direct children of a container (an attribute set or a `let` block),
/// tracking the most recent comment so it can bind to the next assignment.
fn collect_container(
    text: &str,
    file: &str,
    origin: &str,
    container: &SyntaxNode,
    prefix: &[String],
    out: &mut Vec<Record>,
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
                collect_binding(text, file, origin, &binding, prefix, comment, out);
            }
        }
    }
}

fn collect_binding(
    text: &str,
    file: &str,
    origin: &str,
    binding: &AttrpathValue,
    prefix: &[String],
    comment: Option<(String, usize, usize)>,
    out: &mut Vec<Record>,
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
    if comps.is_empty() || comps[0] == "imports" {
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
        if !annotated {
            collect_container(text, file, origin, nested.syntax(), &segments, out);
            return;
        }
    }

    let node = inner.syntax();
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
        assert_eq!(
            paths,
            vec!["networking.hostName", "services.openssh.enable"]
        );
        assert_eq!(scan.records[0].value, "\"demo\"");
        assert!(scan.records[0].comment_span.is_some());
        assert!(scan.records[1].comment_span.is_none());
    }

    #[test]
    fn flattens_unannotated_attrsets() {
        let text = r#"{
  networking = {
    hostName = "demo";
    firewall.enable = true;
  };
}
"#;
        let scan = scan(text);
        let paths: Vec<String> = scan.records.iter().map(Record::path).collect();
        assert_eq!(
            paths,
            vec!["networking.hostName", "networking.firewall.enable"]
        );
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
        let text = "{\n  environment.systemPackages = with pkgs; [ git vim ];\n}\n";
        let wrapped = scan(text);
        assert_eq!(wrapped.records.len(), 1);
        assert_eq!(wrapped.records[0].wrapper.as_deref(), Some("with pkgs;"));
        let plain = scan("{ a = [ 1 2 ];\n}\n");
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
    fn records_host_targets() {
        let text = r#"{ self, inputs, ... }: {
  flake.nixosConfigurations.alpha = inputs.nixpkgs.lib.nixosSystem {
    modules = [ self.modules.nixos.common ];
  };
  flake.modules.nixos.common = { pkgs, ... }: {
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
        assert_eq!(paths, vec!["extraPackages", "environment.systemPackages"]);
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
    }

    #[test]
    fn scan_value_reads_inner_bindings() {
        let records = scan_value("{ enable = true; allowedTCPPorts = [ 22 ]; }");
        let paths: Vec<String> = records.iter().map(Record::path).collect();
        assert_eq!(paths, vec!["enable", "allowedTCPPorts"]);
        assert_eq!(records[0].value, "true");
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
}
