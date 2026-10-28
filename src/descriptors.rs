use std::collections::HashSet;
use std::path::Path;

use rnix::ast::{Attr, AttrpathValue, Expr, HasEntry, InterpolPart};
use rowan::ast::AstNode;
use serde::Deserialize;

use crate::dsl::{OptionSpec, WidgetKind};
use crate::model::{self, MenuModel, Record};
use crate::scan::ScanResult;

/// Environment variable holding a path to a descriptor file, used when the
/// `--descriptors` flag is absent.
pub const ENV_VAR: &str = "NSG_DESCRIPTORS";

/// Visibility tier of a descriptor, mapped onto the `advanced` and `hidden`
/// directives of an `@option` comment.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    #[default]
    Normal,
    Advanced,
    Hidden,
}

/// One entry of a descriptor sidecar. Every field an `@option` comment can set
/// is representable; unknown fields are rejected so typos surface early.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Descriptor {
    /// Full option path, e.g. `desktop.ojuny.dux.settings.clockDateFormat`.
    pub path: String,
    /// Widget name, using the same names as `@option widget = "...";`.
    #[serde(default)]
    pub widget: Option<String>,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub group: Option<String>,
    #[serde(default)]
    pub order: Option<i64>,
    #[serde(default)]
    pub choices: Option<Vec<String>>,
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
    #[serde(default)]
    pub step: Option<f64>,
    #[serde(default)]
    pub tier: Tier,
    #[serde(default, rename = "applyCommand", alias = "apply_command")]
    pub apply_command: Option<String>,
}

impl Descriptor {
    /// Convert the descriptor to the annotation type the rest of the app
    /// already understands.
    pub fn spec(&self) -> OptionSpec {
        let mut spec = OptionSpec {
            widget: self.widget.as_deref().and_then(WidgetKind::parse),
            label: (!self.label.trim().is_empty()).then(|| self.label.clone()),
            description: (!self.description.trim().is_empty()).then(|| self.description.clone()),
            choices: self.choices.clone(),
            min: self.min,
            max: self.max,
            step: self.step,
            group: self.group.clone(),
            order: self.order,
            apply_command: self.apply_command.clone(),
            ..OptionSpec::default()
        };
        match self.tier {
            Tier::Normal => {}
            Tier::Advanced => spec.advanced = true,
            Tier::Hidden => spec.hidden = true,
        }
        spec
    }

    fn record(&self, location: &AssignmentLocation) -> Record {
        Record {
            segments: location.segments.clone(),
            option_path: Some(
                model::normalize_namespace(&self.path)
                    .split('.')
                    .map(|segment| segment.to_string())
                    .collect(),
            ),
            file: location.file.clone(),
            line: location.line,
            col: location.col,
            byte_start: location.byte_start,
            byte_end: location.byte_end,
            value: location.value.clone(),
            wrapper: location.wrapper.clone(),
            origin: String::new(),
            spec: self.spec(),
            comment_span: None,
        }
    }
}

/// The assignment a descriptor resolves to. Spans index the original file text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssignmentLocation {
    pub file: String,
    pub line: u32,
    pub col: u32,
    pub byte_start: usize,
    pub byte_end: usize,
    pub value: String,
    pub wrapper: Option<String>,
    pub segments: Vec<String>,
}

/// What a merge changed. Descriptors that lose to an annotation or cannot be
/// located are reported as skipped rather than failing the whole scan.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MergeReport {
    pub added: usize,
    pub updated: usize,
    pub skipped: Vec<String>,
}

/// Parse and validate descriptor JSON.
pub fn parse(text: &str) -> Result<Vec<Descriptor>, String> {
    let descriptors: Vec<Descriptor> =
        serde_json::from_str(text).map_err(|error| format!("invalid descriptor JSON: {error}"))?;
    validate(descriptors)
}

/// Read and validate a descriptor file.
pub fn load(path: &str) -> Result<Vec<Descriptor>, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("cannot read descriptor file {path}: {error}"))?;
    parse(&text).map_err(|error| format!("{path}: {error}"))
}

fn validate(descriptors: Vec<Descriptor>) -> Result<Vec<Descriptor>, String> {
    let mut seen = HashSet::new();
    for (index, descriptor) in descriptors.iter().enumerate() {
        let context = format!("descriptor {index} ({})", descriptor.path);
        if descriptor.path.trim().is_empty() {
            return Err(format!("descriptor {index}: path is required"));
        }
        if descriptor
            .path
            .split('.')
            .any(|segment| segment.trim().is_empty())
        {
            return Err(format!("{context}: path has an empty segment"));
        }
        let Some(widget) = descriptor.widget.as_deref() else {
            return Err(format!("{context}: widget is required"));
        };
        if WidgetKind::parse(widget).is_none() {
            return Err(format!("{context}: unknown widget {widget:?}"));
        }
        if let (Some(min), Some(max)) = (descriptor.min, descriptor.max) {
            if min > max {
                return Err(format!(
                    "{context}: min ({min}) is greater than max ({max})"
                ));
            }
        }
        if let Some(step) = descriptor.step {
            if step <= 0.0 {
                return Err(format!("{context}: step must be positive"));
            }
        }
        if let Some(choices) = &descriptor.choices {
            if choices.is_empty() {
                return Err(format!("{context}: choices must not be empty"));
            }
        }
        if !seen.insert(model::normalize_namespace(&descriptor.path)) {
            return Err(format!("{context}: duplicate path"));
        }
    }
    Ok(descriptors)
}

/// Merge descriptors into a completed scan.
///
/// An `@option` annotation always wins. A descriptor for a path without an
/// annotation is located on disk and added as a normal record, so it edits and
/// dispatches exactly like an annotated one. `locator` is the metadata-backed
/// lookup; the scanned files are the fallback.
pub fn merge(
    scan: &mut ScanResult,
    descriptors: &[Descriptor],
    locator: &dyn Fn(&str) -> Result<AssignmentLocation, String>,
) -> MergeReport {
    let mut report = MergeReport::default();
    if descriptors.is_empty() {
        return report;
    }
    let search_files = scanned_files(scan);
    for descriptor in descriptors {
        let key = model::normalize_namespace(&descriptor.path);
        if let Some((menu_index, record_index)) = find_record(scan, &key) {
            if scan.menus[menu_index].records[record_index]
                .comment_span
                .is_some()
            {
                continue;
            }
            scan.menus[menu_index].records[record_index].spec = descriptor.spec();
            report.updated += 1;
            continue;
        }

        let located = locator(&descriptor.path).or_else(|metadata_error| {
            find_in_files(&search_files, &descriptor.path).map_err(|scan_error| {
                format!("metadata lookup failed: {metadata_error}; {scan_error}")
            })
        });
        let location = match located {
            Ok(location) => location,
            Err(error) => {
                report.skipped.push(format!("{}: {error}", descriptor.path));
                continue;
            }
        };
        if !Path::new(&location.file).is_file() {
            report.skipped.push(format!(
                "{}: assignment file does not exist: {}",
                descriptor.path, location.file
            ));
            continue;
        }
        if let Some((menu_index, record_index)) = find_record_by_span(scan, &location) {
            if scan.menus[menu_index].records[record_index]
                .comment_span
                .is_some()
            {
                continue;
            }
            scan.menus[menu_index].records[record_index].spec = descriptor.spec();
            report.updated += 1;
            continue;
        }
        attach(scan, descriptor, location);
        report.added += 1;
    }
    report
}

/// Resolve a descriptor path through the option registry. Freeform settings
/// expose no option for their leaves, so progressively shorter parent paths are
/// tried until one has a definition or declaration file. The assignment is then
/// located inside that file by attribute path.
pub fn locate_via_metadata(
    flake_path: &str,
    target: &str,
    path: &str,
) -> Result<AssignmentLocation, String> {
    let target = crate::metadata::Target::parse(target)
        .ok_or_else(|| format!("invalid option target {target:?}"))?;
    let mut segments: Vec<String> = model::normalize_namespace(path)
        .split('.')
        .map(str::to_string)
        .collect();
    let mut errors = Vec::new();
    while !segments.is_empty() {
        match crate::metadata::detail(flake_path, &target, &segments) {
            Ok(detail) => match locate_in_files(&candidate_files(&detail), path) {
                Ok(location) => return Ok(location),
                Err(LookupError::Ambiguous(message)) => return Err(message),
                Err(LookupError::Failed(message)) => errors.push(message),
            },
            Err(error) => errors.push(error),
        }
        segments.pop();
    }
    if errors.is_empty() {
        Err(format!("no option definition or declaration covers {path}"))
    } else {
        Err(errors.join("; "))
    }
}

/// Every file the registry reports for an option, definitions before
/// declarations, without duplicates.
fn candidate_files(detail: &crate::metadata::OptionDetail) -> Vec<String> {
    let mut files = Vec::new();
    for file in detail
        .definitions
        .iter()
        .map(|definition| &definition.file)
        .chain(
            detail
                .declarations
                .iter()
                .map(|declaration| &declaration.file),
        )
    {
        if !file.is_empty() && !files.contains(file) {
            files.push(file.clone());
        }
    }
    files
}

/// Why locating an assignment failed. Ambiguity is definitive: several
/// candidates match, so the descriptor must be skipped rather than retried.
enum LookupError {
    Ambiguous(String),
    Failed(String),
}

impl LookupError {
    fn message(self) -> String {
        match self {
            LookupError::Ambiguous(message) | LookupError::Failed(message) => message,
        }
    }
}

/// How closely a located assignment matched the wanted path.
enum Located {
    Exact(AssignmentLocation),
    Suffix(AssignmentLocation),
}

/// Find the assignment for `path` in one Nix file. Nested attribute sets and
/// module lambdas are flattened, so a descriptor can address a freeform leaf
/// such as `settings.clockDateFormat`.
pub fn find_assignment_in_file(file: &str, path: &str) -> Result<AssignmentLocation, String> {
    match locate_in_file(file, path) {
        Ok(Located::Exact(location) | Located::Suffix(location)) => Ok(location),
        Err(error) => Err(error.message()),
    }
}

fn locate_in_file(file: &str, path: &str) -> Result<Located, LookupError> {
    let text = std::fs::read_to_string(file).map_err(|error| {
        LookupError::Failed(format!("cannot read assignment file {file}: {error}"))
    })?;
    let mut located = find_assignment(&text, path).map_err(|error| match error {
        LookupError::Ambiguous(message) => LookupError::Ambiguous(format!("{file}: {message}")),
        LookupError::Failed(message) => LookupError::Failed(format!("{file}: {message}")),
    })?;
    match &mut located {
        Located::Exact(location) | Located::Suffix(location) => location.file = file.to_string(),
    }
    Ok(located)
}

/// Try every candidate file, preferring an exact path over a unique suffix.
fn locate_in_files(files: &[String], path: &str) -> Result<AssignmentLocation, LookupError> {
    let mut errors = Vec::new();
    let mut ambiguous = None;
    let mut suffix: Option<(AssignmentLocation, usize)> = None;
    for file in files {
        match locate_in_file(file, path) {
            Ok(Located::Exact(location)) => return Ok(location),
            Ok(Located::Suffix(location)) => {
                suffix = Some(match suffix {
                    Some((first, count)) => (first, count + 1),
                    None => (location, 1),
                });
            }
            Err(LookupError::Ambiguous(message)) => ambiguous = Some(message),
            Err(LookupError::Failed(message)) => errors.push(message),
        }
    }
    if let Some(message) = ambiguous {
        return Err(LookupError::Ambiguous(message));
    }
    if let Some((location, count)) = suffix {
        if count > 1 {
            return Err(LookupError::Ambiguous(format!(
                "ambiguous assignment for {path}: {count} candidate files match by suffix"
            )));
        }
        return Ok(location);
    }
    if errors.is_empty() {
        return Err(LookupError::Failed(format!(
            "no assignment found for {path}"
        )));
    }
    Err(LookupError::Failed(errors.join("; ")))
}

fn find_assignment(text: &str, path: &str) -> Result<Located, LookupError> {
    let parsed = rnix::Root::parse(text);
    let root = rnix::Root::cast(parsed.syntax())
        .ok_or_else(|| LookupError::Failed("file did not parse as Nix".to_string()))?;
    let Some(expr) = root.expr() else {
        return Err(LookupError::Failed("file has no expression".to_string()));
    };
    let mut candidates = Vec::new();
    collect_assignments(&expr, &[], &mut candidates);
    let wanted = model::normalize_namespace(path);

    let mut exact: Option<(&Vec<String>, &AttrpathValue)> = None;
    let mut suffix: Vec<(&Vec<String>, &AttrpathValue)> = Vec::new();
    for (segments, binding) in &candidates {
        let full = model::normalize_namespace(&segments.join("."));
        if full == wanted {
            if exact.is_none() {
                exact = Some((segments, binding));
            }
        } else if full.ends_with(&format!(".{wanted}")) || wanted.ends_with(&format!(".{full}")) {
            suffix.push((segments, binding));
        }
    }
    let (is_exact, (segments, binding)) = match exact {
        Some(candidate) => (true, candidate),
        None => match suffix.as_slice() {
            [] => {
                return Err(LookupError::Failed(format!(
                    "no assignment found for {path}"
                )))
            }
            [only] => (false, *only),
            _ => {
                return Err(LookupError::Ambiguous(format!(
                    "ambiguous assignment for {path}: {} candidates match by suffix",
                    suffix.len()
                )))
            }
        },
    };
    let Some(value) = binding.value() else {
        return Err(LookupError::Failed(format!(
            "assignment for {path} has no value"
        )));
    };
    let value_start = usize::from(value.syntax().text_range().start());
    let value_text = value.syntax().text().to_string();
    let inner = peel_value(&value).unwrap_or_else(|| value.clone());
    let inner_start = usize::from(inner.syntax().text_range().start());
    let inner_end = usize::from(inner.syntax().text_range().end());
    let head = value_text
        .get(..inner_start.saturating_sub(value_start))
        .unwrap_or("")
        .trim();
    let wrapper = (!head.is_empty()).then(|| head.to_string());
    let (line, col) = crate::scan::line_col(text, inner_start);
    let location = AssignmentLocation {
        file: String::new(),
        line,
        col,
        byte_start: inner_start,
        byte_end: inner_end,
        value: inner.syntax().text().to_string(),
        wrapper,
        segments: segments.clone(),
    };
    Ok(if is_exact {
        Located::Exact(location)
    } else {
        Located::Suffix(location)
    })
}

fn scanned_files(scan: &ScanResult) -> Vec<String> {
    let mut files = Vec::new();
    for menu in &scan.menus {
        if !menu.file.is_empty() && !files.contains(&menu.file) {
            files.push(menu.file.clone());
        }
        for record in &menu.records {
            if !record.file.is_empty() && !files.contains(&record.file) {
                files.push(record.file.clone());
            }
        }
    }
    files
}

fn find_in_files(files: &[String], path: &str) -> Result<AssignmentLocation, String> {
    locate_in_files(files, path).map_err(LookupError::message)
}

fn find_record(scan: &ScanResult, key: &str) -> Option<(usize, usize)> {
    for (menu_index, menu) in scan.menus.iter().enumerate() {
        for (record_index, record) in menu.records.iter().enumerate() {
            if model::normalize_namespace(&record.path()) == key {
                return Some((menu_index, record_index));
            }
        }
    }
    None
}

fn find_record_by_span(scan: &ScanResult, location: &AssignmentLocation) -> Option<(usize, usize)> {
    for (menu_index, menu) in scan.menus.iter().enumerate() {
        for (record_index, record) in menu.records.iter().enumerate() {
            if record.file == location.file
                && record.byte_start == location.byte_start
                && record.byte_end == location.byte_end
            {
                return Some((menu_index, record_index));
            }
        }
    }
    None
}

fn attach(scan: &mut ScanResult, descriptor: &Descriptor, location: AssignmentLocation) {
    let record = descriptor.record(&location);
    if let Some(menu) = scan
        .menus
        .iter_mut()
        .find(|menu| menu.file == location.file)
    {
        menu.has_menu = true;
        menu.records.push(record);
        return;
    }
    let mut menu = MenuModel {
        file: location.file.clone(),
        has_menu: true,
        ..MenuModel::default()
    };
    menu.spec.displayname = Some(
        descriptor
            .group
            .clone()
            .filter(|group| !group.trim().is_empty())
            .unwrap_or_else(|| "Dux Shell".to_string()),
    );
    menu.spec.order = descriptor.order;
    menu.records.push(record);
    scan.menus.push(menu);
}

fn collect_assignments(
    expr: &Expr,
    prefix: &[String],
    out: &mut Vec<(Vec<String>, AttrpathValue)>,
) {
    match expr {
        Expr::AttrSet(set) => {
            for binding in set.attrpath_values() {
                let segments = binding
                    .attrpath()
                    .map(|path| {
                        path.attrs()
                            .filter_map(|attr| attr_component(&attr))
                            .collect::<Vec<String>>()
                    })
                    .unwrap_or_default();
                if segments.is_empty() {
                    continue;
                }
                let full = [prefix, &segments[..]].concat();
                out.push((full.clone(), binding.clone()));
                if let Some(value) = binding.value() {
                    collect_assignments(&value, &full, out);
                }
            }
        }
        Expr::Lambda(lambda) => {
            if let Some(body) = lambda.body() {
                collect_assignments(&body, prefix, out);
            }
        }
        Expr::LetIn(let_in) => {
            if let Some(body) = let_in.body() {
                collect_assignments(&body, prefix, out);
            }
        }
        Expr::With(with) => {
            if let Some(body) = with.body() {
                collect_assignments(&body, prefix, out);
            }
        }
        Expr::Paren(paren) => {
            if let Some(inner) = paren.expr() {
                collect_assignments(&inner, prefix, out);
            }
        }
        Expr::BinOp(op) => {
            if let Some(lhs) = op.lhs() {
                collect_assignments(&lhs, prefix, out);
            }
            if let Some(rhs) = op.rhs() {
                collect_assignments(&rhs, prefix, out);
            }
        }
        Expr::IfElse(if_else) => {
            if let Some(body) = if_else.body() {
                collect_assignments(&body, prefix, out);
            }
            if let Some(else_body) = if_else.else_body() {
                collect_assignments(&else_body, prefix, out);
            }
        }
        _ => {}
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

fn attr_component(attr: &Attr) -> Option<String> {
    match attr {
        Attr::Ident(ident) => Some(ident.syntax().text().to_string()),
        Attr::Str(string) => match string.normalized_parts().as_slice() {
            [InterpolPart::Literal(text)] => Some(text.clone()),
            _ => None,
        },
        Attr::Dynamic(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl::MenuSpec;

    fn temp_file(name: &str, contents: &str) -> String {
        let path = std::env::temp_dir().join(format!(
            "nix-settings-gui-descriptors-{}-{name}.nix",
            std::process::id()
        ));
        std::fs::write(&path, contents).expect("temp file");
        path.display().to_string()
    }

    fn scan_with(file: &str, record: Option<Record>) -> ScanResult {
        let mut menu = MenuModel {
            file: file.to_string(),
            spec: MenuSpec::default(),
            has_menu: true,
            ..MenuModel::default()
        };
        if let Some(record) = record {
            menu.records.push(record);
        }
        ScanResult {
            menus: vec![menu],
            ..ScanResult::default()
        }
    }

    fn annotated(path: &str, label: &str) -> Record {
        Record {
            segments: path.split('.').map(str::to_string).collect(),
            spec: OptionSpec {
                widget: Some(WidgetKind::Switch),
                label: Some(label.to_string()),
                ..OptionSpec::default()
            },
            comment_span: Some((0, 1)),
            ..Record::default()
        }
    }

    fn descriptor(path: &str, label: &str) -> Descriptor {
        Descriptor {
            path: path.to_string(),
            widget: Some("text".to_string()),
            label: label.to_string(),
            description: String::new(),
            group: None,
            order: None,
            choices: None,
            min: None,
            max: None,
            step: None,
            tier: Tier::Normal,
            apply_command: None,
        }
    }

    #[test]
    fn annotation_wins_over_descriptor() {
        let file = temp_file("precedence", "");
        let mut scan = scan_with(
            &file,
            Some(annotated(
                "desktop.ojuny.dux.settings.clockDateFormat",
                "Annotated",
            )),
        );
        let descriptors = vec![descriptor(
            "desktop.ojuny.dux.settings.clockDateFormat",
            "Descriptor",
        )];
        let report = merge(&mut scan, &descriptors, &|_: &str| -> Result<
            AssignmentLocation,
            String,
        > {
            Err("locator should not run".to_string())
        });
        assert_eq!(report.added, 0);
        assert!(report.skipped.is_empty());
        assert_eq!(
            scan.menus[0].records[0].spec.label.as_deref(),
            Some("Annotated")
        );
    }

    #[test]
    fn descriptor_replaces_an_unannotated_record_spec() {
        let file = temp_file("unannotated", "");
        let mut record = annotated("desktop.ojuny.dux.settings.clockDateFormat", "Old");
        record.comment_span = None;
        let mut scan = scan_with(&file, Some(record));
        let descriptors = vec![descriptor(
            "desktop.ojuny.dux.settings.clockDateFormat",
            "Descriptor",
        )];
        let report = merge(&mut scan, &descriptors, &|_: &str| -> Result<
            AssignmentLocation,
            String,
        > {
            Err("locator should not run".to_string())
        });
        assert_eq!(report.updated, 1);
        assert_eq!(
            scan.menus[0].records[0].spec.label.as_deref(),
            Some("Descriptor")
        );
    }

    #[test]
    fn legacy_namespace_matches_current_namespace() {
        let file = temp_file("legacy", "");
        let mut scan = scan_with(
            &file,
            Some(annotated(
                "desktop.ojuny.dux.settings.clockDateFormat",
                "Annotated",
            )),
        );
        let descriptors = vec![descriptor(
            "desktop.ojuny.dms.settings.clockDateFormat",
            "Descriptor",
        )];
        let report = merge(&mut scan, &descriptors, &|_: &str| -> Result<
            AssignmentLocation,
            String,
        > {
            Err("locator should not run".to_string())
        });
        assert_eq!(report.added, 0);
    }

    #[test]
    fn descriptor_without_annotation_creates_an_editable_record() {
        let file = temp_file("add", "{ a = 1; }");
        let mut scan = scan_with(&file, None);
        let descriptors = vec![descriptor(
            "desktop.ojuny.dux.settings.clockDateFormat",
            "Clock format",
        )];
        let report = merge(&mut scan, &descriptors, &|path: &str| -> Result<
            AssignmentLocation,
            String,
        > {
            assert_eq!(path, "desktop.ojuny.dux.settings.clockDateFormat");
            Ok(AssignmentLocation {
                file: file.clone(),
                line: 1,
                col: 5,
                byte_start: 6,
                byte_end: 7,
                value: "1".to_string(),
                wrapper: None,
                segments: vec!["a".to_string()],
            })
        });
        assert_eq!(report.added, 1);
        assert_eq!(scan.menus[0].records.len(), 1);
        let record = &scan.menus[0].records[0];
        assert_eq!(record.spec.widget, Some(WidgetKind::Text));
        assert_eq!(record.spec.label.as_deref(), Some("Clock format"));
        assert_eq!(record.byte_start, 6);
        assert_eq!(record.segments, vec!["a".to_string()]);
        assert_eq!(record.path(), "desktop.ojuny.dux.settings.clockDateFormat");
    }

    #[test]
    fn annotated_assignment_span_wins_over_descriptor() {
        let file = temp_file("span", "{ a = 1; }");
        let mut record = annotated("desktop.ojuny.dux.settings.clockDateFormat", "Annotated");
        record.file = file.clone();
        record.byte_start = 6;
        record.byte_end = 7;
        let mut scan = scan_with(&file, Some(record));
        let descriptors = vec![descriptor(
            "desktop.ojuny.dux.settings.clockDateFormat.alias",
            "Descriptor",
        )];
        let report = merge(&mut scan, &descriptors, &|_: &str| -> Result<
            AssignmentLocation,
            String,
        > {
            Ok(AssignmentLocation {
                file: file.clone(),
                line: 1,
                col: 5,
                byte_start: 6,
                byte_end: 7,
                value: "1".to_string(),
                wrapper: None,
                segments: vec!["a".to_string()],
            })
        });
        assert_eq!(report.added, 0);
        assert!(report.skipped.is_empty());
        assert_eq!(
            scan.menus[0].records[0].spec.label.as_deref(),
            Some("Annotated")
        );
    }

    #[test]
    fn descriptor_pointing_at_a_missing_file_is_skipped() {
        let file = temp_file("missing", "");
        let mut scan = scan_with(&file, None);
        let descriptors = vec![descriptor(
            "desktop.ojuny.dux.settings.clockDateFormat",
            "Clock format",
        )];
        let report = merge(&mut scan, &descriptors, &|_: &str| -> Result<
            AssignmentLocation,
            String,
        > {
            Ok(AssignmentLocation {
                file: "/nonexistent/nix-settings-gui/absent.nix".to_string(),
                line: 1,
                col: 1,
                byte_start: 0,
                byte_end: 1,
                value: "1".to_string(),
                wrapper: None,
                segments: Vec::new(),
            })
        });
        assert_eq!(report.added, 0);
        assert_eq!(report.skipped.len(), 1);
        assert!(scan.menus[0].records.is_empty());
    }

    #[test]
    fn invalid_descriptors_are_rejected() {
        assert!(parse(r#"[{"path": "a.b"}]"#).is_err());
        assert!(parse(r#"[{"path": "a.b", "widget": "nope"}]"#).is_err());
        assert!(parse(r#"[{"path": "a.b", "widget": "text", "tier": "sometimes"}]"#).is_err());
        assert!(parse(r#"[{"path": "a.b", "widget": "spin", "min": 10, "max": 1}]"#).is_err());
        assert!(
            parse(r#"[{"path": "a.b", "widget": "text"}, {"path": "a.b", "widget": "text"}]"#)
                .is_err()
        );
        assert!(parse(r#"[{"path": "a.b", "widget": "text", "extra": 1}]"#).is_err());
        assert!(load("/nonexistent/nix-settings-gui/descriptors.json").is_err());
    }

    #[test]
    fn tier_maps_to_advanced_and_hidden() {
        let mut advanced = descriptor("a.b", "Advanced");
        advanced.tier = Tier::Advanced;
        assert!(advanced.spec().advanced);
        assert!(!advanced.spec().hidden);

        let mut hidden = descriptor("a.b", "Hidden");
        hidden.tier = Tier::Hidden;
        assert!(hidden.spec().hidden);
    }

    #[test]
    fn finds_freeform_assignment_in_nested_sets() {
        let file = temp_file(
            "locate",
            "{\n  desktop.ojuny.dux.settings = { clockDateFormat = \"%H\"; };\n}\n",
        );
        let location =
            find_assignment_in_file(&file, "desktop.ojuny.dux.settings.clockDateFormat").unwrap();
        assert_eq!(location.value, "\"%H\"");
        assert_eq!(location.segments.len(), 5);

        let location =
            find_assignment_in_file(&file, "desktop.ojuny.dms.settings.clockDateFormat").unwrap();
        assert_eq!(location.value, "\"%H\"");
    }

    #[test]
    fn exact_path_beats_a_suffix_candidate() {
        let file = temp_file(
            "exact",
            "{\n  desktop.ojuny.dux.settings = { clockDateFormat = \"%H\"; };\n  other.desktop.ojuny.dux.settings.clockDateFormat = \"%I\";\n}\n",
        );
        let location =
            find_assignment_in_file(&file, "desktop.ojuny.dux.settings.clockDateFormat").unwrap();
        assert_eq!(location.value, "\"%H\"");
    }

    #[test]
    fn ambiguous_suffix_is_rejected() {
        let file = temp_file(
            "ambiguous",
            "{\n  one.desktop.ojuny.dux.settings.clockDateFormat = \"%H\";\n  two.desktop.ojuny.dux.settings.clockDateFormat = \"%I\";\n}\n",
        );
        let error = find_assignment_in_file(&file, "desktop.ojuny.dux.settings.clockDateFormat")
            .unwrap_err();
        assert!(error.contains("ambiguous"), "{error}");
    }
}
