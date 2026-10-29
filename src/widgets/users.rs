use super::basic::build_raw;
use super::context::{BuildContext, ChangeHook};
use super::editor::Editor;
use super::nix::{attr_key, entry_text, parse_string_literal, quote, string_literal};
use crate::dsl::OptionSpec;
use crate::edit::PendingEdit;
use crate::model::Record;
use crate::scan;
use crate::value;
use gtk::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct UserFields {
    pub name: String,
    pub description: String,
    pub password: String,
    pub groups: Vec<String>,
    pub wheel: bool,
    pub uid: String,
    pub shell: String,
    pub ssh_keys: Vec<String>,
    pub is_normal_user: bool,
    pub is_normal_present: bool,
    pub extras: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct HomeFields {
    pub state_version: String,
    pub extras: String,
}

#[derive(Clone, Debug)]
enum HomeSpans {
    Missing {
        insert_at: usize,
    },
    Block {
        start: usize,
        end: usize,
    },
    Dotted {
        ranges: Vec<(usize, usize)>,
        insert_at: usize,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct HomeUsers {
    entries: Vec<(String, HomeFields)>,
    orig_nix: String,
    spans: HomeSpans,
    default_state_version: String,
}

impl Default for HomeUsers {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            orig_nix: "{ }".to_string(),
            spans: HomeSpans::Missing { insert_at: 0 },
            default_state_version: "25.05".to_string(),
        }
    }
}

impl HomeUsers {
    pub(crate) fn load(record: &Record) -> Self {
        let Ok(text) = std::fs::read_to_string(&record.file) else {
            return Self::default();
        };
        let Some((siblings, insert_at)) =
            scan::sibling_assignments(&text, record.byte_start, record.byte_end)
        else {
            return Self::default();
        };
        if let Some(block) = siblings.iter().find(|assignment| {
            assignment.segments == ["home-manager", "users"]
                && value::classify(&assignment.value) == value::ValueKind::AttrSet
        }) {
            let entries = scan_home_block(&block.value);
            return Self::from_entries(
                entries,
                HomeSpans::Block {
                    start: block.value_start,
                    end: block.value_end,
                },
            );
        }
        let dotted: Vec<&scan::SiblingAssignment> = siblings
            .iter()
            .filter(|assignment| {
                assignment.segments.len() >= 3
                    && assignment.segments[0] == "home-manager"
                    && assignment.segments[1] == "users"
            })
            .collect();
        if !dotted.is_empty() {
            let entries = scan_home_dotted(&dotted);
            let ranges = dotted
                .iter()
                .map(|assignment| (assignment.full_start, assignment.full_end))
                .collect();
            return Self::from_entries(entries, HomeSpans::Dotted { ranges, insert_at });
        }
        Self::from_entries(Vec::new(), HomeSpans::Missing { insert_at })
    }

    fn from_entries(entries: Vec<(String, HomeFields)>, spans: HomeSpans) -> Self {
        let default_state_version = entries
            .iter()
            .map(|(_, home)| home.state_version.trim())
            .find(|value| !value.is_empty())
            .unwrap_or("25.05")
            .to_string();
        let orig_nix = serialize_home(&entries);
        Self {
            entries,
            orig_nix,
            spans,
            default_state_version,
        }
    }

    fn entry(&self, name: &str) -> HomeFields {
        self.entries
            .iter()
            .find(|(entry, _)| entry == name)
            .map(|(_, fields)| fields.clone())
            .unwrap_or_else(|| HomeFields {
                state_version: self.default_state_version.clone(),
                ..HomeFields::default()
            })
    }

    pub(crate) fn default_state_version(&self) -> &str {
        &self.default_state_version
    }
}

impl UserFields {
    fn new(name: String) -> Self {
        Self {
            name,
            is_normal_user: true,
            is_normal_present: true,
            ..Self::default()
        }
    }
}

#[derive(Clone)]
pub(crate) struct UserRow {
    pub(crate) key: gtk::Entry,
    pub(crate) description: gtk::Entry,
    pub(crate) password: gtk::Entry,
    pub(crate) groups: gtk::Entry,
    pub(crate) wheel: gtk::Switch,
    pub(crate) uid: gtk::Entry,
    pub(crate) shell: gtk::Entry,
    pub(crate) ssh_keys: gtk::TextView,
    pub(crate) extras: gtk::TextView,
    pub(crate) home_state_version: Option<String>,
    pub(crate) home_extras: Option<gtk::TextView>,
    pub(crate) remove: gtk::Button,
    pub(crate) expander: gtk::Expander,
    pub(crate) orig: UserFields,
    pub(crate) is_normal_user: bool,
    pub(crate) is_normal_present: bool,
}

impl UserRow {
    pub(crate) fn read_fields(&self) -> UserFields {
        UserFields {
            name: self.key.text().trim().to_string(),
            description: self.description.text().to_string(),
            password: normalize_secret(&self.password.text()),
            groups: parse_groups(&self.groups.text()),
            wheel: self.wheel.is_active(),
            uid: self.uid.text().trim().to_string(),
            shell: self.shell.text().trim().to_string(),
            ssh_keys: parse_lines(&text_of(&self.ssh_keys)),
            is_normal_user: self.is_normal_user,
            is_normal_present: self.is_normal_present,
            extras: text_of(&self.extras),
        }
    }

    pub(crate) fn read_home(&self) -> HomeFields {
        HomeFields {
            state_version: self.home_state_version.clone().unwrap_or_default(),
            extras: self.home_extras.as_ref().map(text_of).unwrap_or_default(),
        }
    }
}

/// Read the direct user entries of a `users.users` attribute set.
pub(crate) fn scan_users(value_text: &str) -> Vec<UserFields> {
    scan::scan_value_direct(value_text)
        .into_iter()
        .filter(|record| record.segments.len() == 1)
        .map(|record| scan_user(&record.segments[0], &record.value))
        .collect()
}

fn scan_user(name: &str, value_text: &str) -> UserFields {
    let mut fields = UserFields {
        name: name.to_string(),
        ..UserFields::default()
    };
    if value::classify(value_text) != value::ValueKind::AttrSet {
        fields.extras = value_text.trim().to_string();
        return fields;
    }

    for record in scan::scan_value_direct(value_text) {
        let path = record.segments.join(".");
        match path.as_str() {
            "description" => fields.description = entry_text(&record.value),
            "hashedPassword" => fields.password = normalize_secret(&record.value),
            "extraGroups" => {
                for item in value::list_items(&record.value).unwrap_or_default() {
                    let group = parse_string_literal(&item);
                    if group == "wheel" {
                        fields.wheel = true;
                    } else if !group.is_empty() {
                        fields.groups.push(group);
                    }
                }
            }
            "uid" => fields.uid = record.value.trim().to_string(),
            "shell" => fields.shell = record.value.trim().to_string(),
            "openssh.authorizedKeys.keys" => {
                fields.ssh_keys = parse_string_list(&record.value);
            }
            "isNormalUser" => {
                fields.is_normal_user = super::nix::truthy(&record.value);
                fields.is_normal_present = true;
            }
            "openssh" => scan_openssh(&record.value, &mut fields),
            _ => fields.extras.push_str(&format!(
                "{} = {}; ",
                dotted_key(&record.segments),
                record.value.trim()
            )),
        }
    }
    fields.extras = fields.extras.trim().to_string();
    fields
}

/// Accept the nested `openssh = { authorizedKeys.keys = [...]; };` spelling
/// without losing the other keys below it.
fn scan_openssh(value_text: &str, fields: &mut UserFields) {
    let leaves = scan::scan_value(value_text);
    if leaves.is_empty() {
        fields
            .extras
            .push_str(&format!("openssh = {}; ", value_text.trim()));
        return;
    }
    for leaf in leaves {
        let path = leaf.segments.join(".");
        if path == "authorizedKeys.keys" {
            fields.ssh_keys = parse_string_list(&leaf.value);
        } else {
            fields.extras.push_str(&format!(
                "openssh.{} = {}; ",
                dotted_key(&leaf.segments),
                leaf.value.trim()
            ));
        }
    }
}

fn scan_home_block(value_text: &str) -> Vec<(String, HomeFields)> {
    scan::scan_value_direct(value_text)
        .into_iter()
        .filter(|record| record.segments.len() == 1)
        .map(|record| (record.segments[0].clone(), scan_home_user(&record.value)))
        .collect()
}

fn scan_home_user(value_text: &str) -> HomeFields {
    let mut fields = HomeFields::default();
    for leaf in scan::scan_value(value_text) {
        apply_home_leaf(&mut fields, &leaf.segments, &leaf.value);
    }
    fields.extras = fields.extras.trim().to_string();
    fields
}

fn scan_home_dotted(assignments: &[&scan::SiblingAssignment]) -> Vec<(String, HomeFields)> {
    let mut entries: Vec<(String, HomeFields)> = Vec::new();
    for assignment in assignments {
        let Some(name) = assignment.segments.get(2).cloned() else {
            continue;
        };
        let index = match entries.iter().position(|(entry, _)| entry == &name) {
            Some(index) => index,
            None => {
                entries.push((name, HomeFields::default()));
                entries.len() - 1
            }
        };
        if assignment.segments.len() == 3 {
            if value::classify(&assignment.value) == value::ValueKind::AttrSet {
                entries[index].1 = scan_home_user(&assignment.value);
            }
            continue;
        }
        apply_home_leaf(
            &mut entries[index].1,
            &assignment.segments[3..],
            &assignment.value,
        );
    }
    for (_, fields) in &mut entries {
        fields.extras = fields.extras.trim().to_string();
    }
    entries
}

fn apply_home_leaf(fields: &mut HomeFields, segments: &[String], value_text: &str) {
    match segments.join(".").as_str() {
        "home.stateVersion" => fields.state_version = entry_text(value_text),
        "home.username" | "home.homeDirectory" => {}
        _ => fields.extras.push_str(&format!(
            "{} = {}; ",
            dotted_key(segments),
            value_text.trim()
        )),
    }
}

fn serialize_home(entries: &[(String, HomeFields)]) -> String {
    let mut parts = Vec::new();
    for (name, home) in entries {
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        let mut fields = Vec::new();
        if !home.state_version.trim().is_empty() {
            fields.push(format!(
                "home.stateVersion = {};",
                string_literal(&home.state_version)
            ));
        }
        let extras = home.extras.trim();
        if !extras.is_empty() {
            fields.push(extras.to_string());
        }
        parts.push(format!("{} = {{ {} }};", attr_key(name), fields.join(" ")));
    }
    format!("{{ {} }}", parts.join(" "))
}

fn normalize_secret(value: &str) -> String {
    if value.trim() == "null" {
        String::new()
    } else {
        entry_text(value)
    }
}

fn parse_string_list(value_text: &str) -> Vec<String> {
    value::list_items(value_text)
        .unwrap_or_default()
        .into_iter()
        .map(|item| parse_string_literal(&item))
        .filter(|item| !item.is_empty())
        .collect()
}

fn parse_groups(text: &str) -> Vec<String> {
    text.split(',')
        .map(|group| group.trim().to_string())
        .filter(|group| !group.is_empty() && group != "wheel")
        .collect()
}

fn parse_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
        .collect()
}

fn dotted_key(segments: &[String]) -> String {
    segments
        .iter()
        .map(|segment| attr_key(segment))
        .collect::<Vec<_>>()
        .join(".")
}

fn text_of(view: &gtk::TextView) -> String {
    let buffer = view.buffer();
    buffer
        .text(&buffer.start_iter(), &buffer.end_iter(), false)
        .to_string()
}

fn string_list(items: &[String]) -> String {
    let quoted: Vec<String> = items.iter().map(|item| quote(item)).collect();
    format!("[ {} ]", quoted.join(" "))
}

/// Render the whole editor back to a `users.users` attribute set.
pub(crate) fn serialize_users(users: &[UserFields]) -> String {
    let mut entries = Vec::new();
    for user in users {
        let mut fields = Vec::new();
        if user.is_normal_user || user.is_normal_present {
            fields.push(format!("isNormalUser = {};", user.is_normal_user));
        }
        if !user.description.trim().is_empty() {
            fields.push(format!(
                "description = {};",
                string_literal(&user.description)
            ));
        }
        if !user.password.trim().is_empty() {
            fields.push(format!(
                "hashedPassword = {};",
                string_literal(&user.password)
            ));
        }
        let mut groups: Vec<String> = user
            .groups
            .iter()
            .map(|group| group.trim().to_string())
            .filter(|group| !group.is_empty() && group != "wheel")
            .map(|group| quote(&group))
            .collect();
        if user.wheel {
            groups.push(quote("wheel"));
        }
        if !groups.is_empty() {
            fields.push(format!("extraGroups = [ {} ];", groups.join(" ")));
        }
        if !user.uid.trim().is_empty() {
            fields.push(format!("uid = {};", user.uid.trim()));
        }
        if !user.shell.trim().is_empty() {
            fields.push(format!("shell = {};", user.shell.trim()));
        }
        if !user.ssh_keys.is_empty() {
            fields.push(format!(
                "openssh.authorizedKeys.keys = {};",
                string_list(&user.ssh_keys)
            ));
        }
        let extras = user.extras.trim();
        if !extras.is_empty() {
            fields.push(extras.to_string());
        }
        entries.push(format!(
            "{} = {{ {} }};",
            attr_key(user.name.trim()),
            fields.join(" ")
        ));
    }
    format!("{{ {} }}", entries.join(" "))
}

pub(crate) fn users_nix(editor: &Editor) -> String {
    let Editor::Users { rows, .. } = editor else {
        return "{ }".to_string();
    };
    let users: Vec<UserFields> = rows.borrow().iter().map(UserRow::read_fields).collect();
    serialize_users(&users)
}

pub(crate) fn users_unchanged(
    rows: &Rc<RefCell<Vec<UserRow>>>,
    orig_count: usize,
    home_manager: bool,
    home: Option<&HomeUsers>,
) -> bool {
    let unchanged_users = {
        let rows = rows.borrow();
        rows.len() == orig_count && rows.iter().all(|row| row.read_fields() == row.orig)
    };
    if !unchanged_users || !home_manager {
        return unchanged_users;
    }
    let Some(home) = home else {
        return true;
    };
    let entries = home_entries(rows);
    serialize_home(&entries) == home.orig_nix
}

pub(crate) fn users_validation_error(
    rows: &Rc<RefCell<Vec<UserRow>>>,
    _home_manager: bool,
) -> Option<String> {
    let users: Vec<UserFields> = rows.borrow().iter().map(UserRow::read_fields).collect();
    validation_error(&users)
}

fn home_entries(rows: &Rc<RefCell<Vec<UserRow>>>) -> Vec<(String, HomeFields)> {
    rows.borrow()
        .iter()
        .map(|row| (row.key.text().trim().to_string(), row.read_home()))
        .collect()
}

/// The edits for the user list: the `users.users` value plus, when enabled,
/// the sibling `home-manager.users` block.
pub(crate) fn users_edits(editor: &Editor, record: &Record) -> Vec<PendingEdit> {
    let Editor::Users {
        rows,
        home,
        home_manager,
        orig_count,
        ..
    } = editor
    else {
        return Vec::new();
    };
    let users_changed = !users_unchanged(rows, *orig_count, false, None);
    let mut edits = Vec::new();
    if users_changed {
        edits.push(PendingEdit {
            file: record.file.clone(),
            start: record.byte_start,
            end: record.byte_end,
            value: users_nix(editor),
        });
    }
    if !home_manager {
        return edits;
    }
    let Some(home) = home else {
        return edits;
    };
    let entries = home_entries(rows);
    let nix = serialize_home(&entries);
    if nix == home.orig_nix {
        return edits;
    }
    match &home.spans {
        HomeSpans::Block { start, end } => edits.push(PendingEdit {
            file: record.file.clone(),
            start: *start,
            end: *end,
            value: nix,
        }),
        HomeSpans::Dotted { ranges, insert_at } => {
            for (start, end) in ranges {
                edits.push(PendingEdit {
                    file: record.file.clone(),
                    start: *start,
                    end: *end,
                    value: String::new(),
                });
            }
            edits.push(PendingEdit {
                file: record.file.clone(),
                start: *insert_at,
                end: *insert_at,
                value: format!("\n\nhome-manager.users = {nix};"),
            });
        }
        HomeSpans::Missing { insert_at } => edits.push(PendingEdit {
            file: record.file.clone(),
            start: *insert_at,
            end: *insert_at,
            value: format!("\n\nhome-manager.users = {nix};"),
        }),
    }
    edits
}

pub(crate) fn password_is_set(password: &str) -> bool {
    let text = password.trim();
    !text.is_empty() && !matches!(text, "!" | "*" | "!*" | "!!")
}

pub(crate) fn is_valid_admin(user: &UserFields) -> bool {
    !user.name.trim().is_empty()
        && user.is_normal_user
        && user.wheel
        && password_is_set(&user.password)
}

fn valid_account_name(name: &str) -> bool {
    let mut characters = name.chars();
    match characters.next() {
        Some(character) if character.is_ascii_alphabetic() || character == '_' => {}
        _ => return false,
    }
    name != "."
        && name != ".."
        && characters.all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.')
        })
}

fn valid_group_name(group: &str) -> bool {
    valid_account_name(group)
}

fn valid_shell(shell: &str) -> bool {
    let Some(path) = shell.trim().strip_prefix("pkgs.") else {
        return false;
    };
    !path.is_empty()
        && path.split('.').all(|segment| {
            let mut characters = segment.chars();
            match characters.next() {
                Some(character) if character.is_ascii_alphabetic() || character == '_' => {}
                _ => return false,
            }
            characters.all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '_' | '\'')
            })
        })
}

pub(crate) fn validation_error(users: &[UserFields]) -> Option<String> {
    if users.is_empty() {
        return Some("At least one user is required.".to_string());
    }
    let mut names = std::collections::HashSet::new();
    let mut uids = std::collections::HashMap::new();
    for user in users {
        let name = user.name.trim();
        if name.is_empty() {
            return Some("Every user needs a name.".to_string());
        }
        if !valid_account_name(name) {
            return Some(format!(
                "User name {name:?} is invalid. Use letters, digits, underscores, dots, and dashes, starting with a letter or underscore."
            ));
        }
        if !names.insert(name.to_string()) {
            return Some(format!("User names must be unique; {name} is repeated."));
        }
        for group in &user.groups {
            let group = group.trim();
            if !valid_group_name(group) {
                return Some(format!(
                    "Group {group:?} for {name} is invalid. Use letters, digits, underscores, dots, and dashes, starting with a letter or underscore."
                ));
            }
        }
        if !user.uid.trim().is_empty() {
            let uid = match user.uid.trim().parse::<u32>() {
                Ok(0) | Err(_) => {
                    return Some(format!("UID for {name} must be a positive number."));
                }
                Ok(uid) => uid,
            };
            if let Some(other) = uids.insert(uid, name.to_string()) {
                return Some(format!("UID {uid} is used by both {other} and {name}."));
            }
        }
        if !user.shell.trim().is_empty() && !valid_shell(user.shell.trim()) {
            return Some(format!(
                "Shell for {name} must be a pkgs.* package expression, for example pkgs.zsh."
            ));
        }
    }
    if !users.iter().any(is_valid_admin) {
        return Some(
            "At least one normal user with sudo privileges and a configured password is required."
                .to_string(),
        );
    }
    None
}

/// Build the user list editor. Every user is an expander with its account
/// fields, and the add/remove buttons enforce the administrator invariant.
pub(crate) fn build_users(record: &Record, ctx: &BuildContext) -> Editor {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
    root.set_hexpand(true);

    let hint = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .label("Manage the accounts in users.users. One normal account must keep sudo privileges and a password.")
        .css_classes(["dim-label", "caption"])
        .build();
    root.append(&hint);

    let status = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .visible(false)
        .css_classes(["error"])
        .build();
    root.append(&status);

    let rows_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    rows_box.set_hexpand(true);
    root.append(&rows_box);

    let rows: Rc<RefCell<Vec<UserRow>>> = Rc::new(RefCell::new(Vec::new()));
    let on_change: ChangeHook = Rc::new(RefCell::new(None));
    let notify: Rc<dyn Fn()> = {
        let rows = rows.clone();
        let on_change = on_change.clone();
        let status = status.clone();
        Rc::new(move || {
            refresh_user_state(&rows, &status);
            if let Some(callback) = on_change.borrow().as_ref() {
                callback();
            }
        })
    };

    let home = record.spec.home_manager.then(|| HomeUsers::load(record));
    let default_state_version = home
        .as_ref()
        .map(|home| home.default_state_version().to_string())
        .unwrap_or_else(|| "25.05".to_string());
    let scans = scan_users(&record.value);
    let orig_count = scans.len();
    for scan in scans {
        let home_fields = home
            .as_ref()
            .map(|home| home.entry(&scan.name))
            .unwrap_or_default();
        let row = make_user_row(
            scan,
            home_fields,
            &default_state_version,
            record.spec.home_manager,
            ctx,
            &rows_box,
            &rows,
            &notify,
        );
        rows_box.append(&row.expander);
        rows.borrow_mut().push(row);
    }

    let add = gtk::Button::with_label("Add user");
    add.set_halign(gtk::Align::Start);
    {
        let rows_box = rows_box.clone();
        let rows = rows.clone();
        let notify = notify.clone();
        let ctx = ctx.clone();
        let default_state_version = default_state_version.clone();
        let home_manager = record.spec.home_manager;
        add.connect_clicked(move |_| {
            let name = unique_user_name(&rows.borrow());
            let scan = UserFields::new(name);
            let home_fields = HomeFields {
                state_version: default_state_version.clone(),
                ..HomeFields::default()
            };
            let row = make_user_row(
                scan,
                home_fields,
                &default_state_version,
                home_manager,
                &ctx,
                &rows_box,
                &rows,
                &notify,
            );
            rows_box.append(&row.expander);
            rows.borrow_mut().push(row);
            notify();
        });
    }
    root.append(&add);

    refresh_user_state(&rows, &status);
    Editor::Users {
        root,
        rows,
        on_change,
        orig_count,
        home_manager: record.spec.home_manager,
        home,
    }
}

fn unique_user_name(rows: &[UserRow]) -> String {
    let names: Vec<String> = rows
        .iter()
        .map(|row| row.key.text().trim().to_string())
        .collect();
    let mut index = 1;
    loop {
        let candidate = if index == 1 {
            "user".to_string()
        } else {
            format!("user{index}")
        };
        if !names.iter().any(|name| name == &candidate) {
            return candidate;
        }
        index += 1;
    }
}

#[allow(clippy::too_many_arguments)]
fn make_user_row(
    scan: UserFields,
    home_fields: HomeFields,
    default_state_version: &str,
    home_manager: bool,
    ctx: &BuildContext,
    rows_box: &gtk::Box,
    rows: &Rc<RefCell<Vec<UserRow>>>,
    notify: &Rc<dyn Fn()>,
) -> UserRow {
    let orig = scan.clone();
    let expander = gtk::Expander::new(Some(&scan.name));
    expander.set_expanded(true);
    let content = gtk::Box::new(gtk::Orientation::Vertical, 6);
    content.set_margin_top(6);
    content.set_margin_bottom(6);
    content.set_margin_start(6);
    content.set_margin_end(6);

    let key = gtk::Entry::new();
    key.set_text(&scan.name);
    key.set_hexpand(true);
    let remove = gtk::Button::from_icon_name("user-trash-symbolic");
    remove.add_css_class("flat");
    remove.set_tooltip_text(Some("Remove user"));
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    header.append(&key);
    header.append(&remove);
    content.append(&header);

    {
        let expander = expander.clone();
        let notify = notify.clone();
        key.connect_changed(move |entry| {
            expander.set_label(Some(&entry.text()));
            notify();
        });
    }

    let description = gtk::Entry::new();
    description.set_text(&scan.description);
    description.set_hexpand(true);
    content.append(&field_block("Description", &description));

    let password = gtk::Entry::new();
    password.set_text(&scan.password);
    password.set_hexpand(true);
    password.add_css_class("monospace");
    password.set_placeholder_text(Some("No password set"));
    let change_password = gtk::Button::with_label("Change password");
    let password_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    password_row.append(&password);
    password_row.append(&change_password);
    content.append(&field_block("Password hash", &password_row));
    if let Some(prompt) = ctx.password_prompt.clone() {
        let password_entry = password.clone();
        let notify = notify.clone();
        change_password.connect_clicked(move |_| {
            let password_entry = password_entry.clone();
            let notify = notify.clone();
            prompt(Box::new(move |hash| {
                if let Some(hash) = hash {
                    password_entry.set_text(&hash);
                    notify();
                }
            }));
        });
    } else {
        change_password.set_sensitive(false);
        change_password.set_tooltip_text(Some("Password hashing is unavailable"));
    }

    let groups = gtk::Entry::new();
    groups.set_text(&scan.groups.join(", "));
    groups.set_hexpand(true);
    groups.set_placeholder_text(Some("Comma separated groups"));
    content.append(&field_block("Extra groups", &groups));

    let wheel = gtk::Switch::new();
    wheel.set_active(scan.wheel);
    wheel.set_halign(gtk::Align::Start);
    content.append(&field_block("Sudo privileges (wheel)", &wheel));

    let uid = gtk::Entry::new();
    uid.set_text(&scan.uid);
    uid.set_hexpand(true);
    uid.set_placeholder_text(Some("Automatic"));
    content.append(&field_block("UID", &uid));

    let shell = gtk::Entry::new();
    shell.set_text(&scan.shell);
    shell.set_hexpand(true);
    shell.set_placeholder_text(Some("pkgs.zsh"));
    shell.add_css_class("monospace");
    content.append(&field_block("Shell", &shell));

    let ssh_keys = gtk::TextView::new();
    ssh_keys.set_monospace(true);
    ssh_keys.set_wrap_mode(gtk::WrapMode::WordChar);
    ssh_keys.buffer().set_text(&scan.ssh_keys.join("\n"));
    let ssh_root = gtk::ScrolledWindow::builder()
        .child(&ssh_keys)
        .hexpand(true)
        .min_content_width(440)
        .min_content_height(72)
        .has_frame(true)
        .build();
    content.append(&field_block("SSH authorized keys", &ssh_root));

    let (extras, extras_root) = build_raw(
        &scan.extras,
        &OptionSpec {
            rows: Some(3),
            ..OptionSpec::default()
        },
    );
    content.append(&field_block("Extra options", &extras_root));

    let (home_state_version, home_extras) = if home_manager {
        let heading = gtk::Label::builder()
            .xalign(0.0)
            .label("Home-manager")
            .css_classes(["heading"])
            .build();
        content.append(&heading);
        let state_version = if home_fields.state_version.trim().is_empty() {
            default_state_version.to_string()
        } else {
            home_fields.state_version.clone()
        };
        let (home_extra_view, home_extra_root) = build_raw(
            &home_fields.extras,
            &OptionSpec {
                rows: Some(3),
                ..OptionSpec::default()
            },
        );
        content.append(&field_block("Extra home-manager options", &home_extra_root));
        (Some(state_version), Some(home_extra_view))
    } else {
        (None, None)
    };

    expander.set_child(Some(&content));

    let row = UserRow {
        key,
        description,
        password,
        groups,
        wheel,
        uid,
        shell,
        ssh_keys,
        extras,
        home_state_version,
        home_extras,
        remove,
        expander,
        orig,
        is_normal_user: scan.is_normal_user,
        is_normal_present: scan.is_normal_present,
    };
    connect_user_row(&row, notify);

    {
        let rows = rows.clone();
        let rows_box = rows_box.clone();
        let notify = notify.clone();
        let key = row.key.clone();
        let expander = row.expander.clone();
        row.remove.connect_clicked(move |_| {
            rows_box.remove(&expander);
            rows.borrow_mut().retain(|row| row.key != key);
            notify();
        });
    }

    row
}

fn connect_user_row(row: &UserRow, notify: &Rc<dyn Fn()>) {
    for entry in [
        &row.description,
        &row.password,
        &row.groups,
        &row.uid,
        &row.shell,
    ] {
        let notify = notify.clone();
        entry.connect_changed(move |_| notify());
    }
    {
        let notify = notify.clone();
        row.wheel.connect_active_notify(move |_| notify());
    }
    {
        let notify = notify.clone();
        row.ssh_keys.buffer().connect_changed(move |_| notify());
    }
    {
        let notify = notify.clone();
        row.extras.buffer().connect_changed(move |_| notify());
    }
    if let Some(extras) = &row.home_extras {
        let notify = notify.clone();
        extras.buffer().connect_changed(move |_| notify());
    }
}

fn refresh_user_state(rows: &Rc<RefCell<Vec<UserRow>>>, status: &gtk::Label) {
    let users: Vec<UserFields> = rows.borrow().iter().map(UserRow::read_fields).collect();
    let error = validation_error(&users);
    status.set_text(error.as_deref().unwrap_or(""));
    status.set_visible(error.is_some());

    let valid_admins = users.iter().filter(|user| is_valid_admin(user)).count();
    let can_remove_any = users.len() > 1;
    let rows = rows.borrow();
    for (row, user) in rows.iter().zip(users.iter()) {
        let can_remove = can_remove_any && (!is_valid_admin(user) || valid_admins > 1);
        row.remove.set_sensitive(can_remove);
        row.remove.set_tooltip_text(Some(if can_remove {
            "Remove user"
        } else {
            "At least one normal sudo user with a password must remain"
        }));
    }
}

fn field_block(label: &str, child: &impl IsA<gtk::Widget>) -> gtk::Box {
    let block = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let caption = gtk::Label::builder()
        .label(label)
        .xalign(0.0)
        .css_classes(["dim-label", "caption"])
        .build();
    block.append(&caption);
    block.append(child);
    block
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        alice = {
            isNormalUser = true;
            description = "Alice";
            hashedPassword = "$6$hash";
            extraGroups = [ "wheel" "video" ];
            uid = 1000;
            shell = pkgs.zsh;
            openssh.authorizedKeys.keys = [ "ssh-ed25519 AAAA" ];
            custom = true;
        };
        bob = {
            description = "Bob";
            extraGroups = [ "audio" ];
        };
    }"#;

    #[test]
    fn scans_user_fields_and_extras() {
        let users = scan_users(SAMPLE);
        assert_eq!(users.len(), 2);
        assert_eq!(users[0].name, "alice");
        assert_eq!(users[0].description, "Alice");
        assert_eq!(users[0].password, "$6$hash");
        assert_eq!(users[0].groups, vec!["video".to_string()]);
        assert!(users[0].wheel);
        assert_eq!(users[0].uid, "1000");
        assert_eq!(users[0].shell, "pkgs.zsh");
        assert_eq!(users[0].ssh_keys, vec!["ssh-ed25519 AAAA".to_string()]);
        assert!(users[0].is_normal_user);
        assert!(users[0].extras.contains("custom = true;"));
    }

    #[test]
    fn serializes_and_rescans_the_same_accounts() {
        let users = scan_users(SAMPLE);
        let nix = serialize_users(&users);
        assert!(nix.contains("alice = {"));
        assert!(nix.contains("isNormalUser = true;"));
        assert!(nix.contains("extraGroups = [ \"video\" \"wheel\" ];"));
        assert!(nix.contains("openssh.authorizedKeys.keys = [ \"ssh-ed25519 AAAA\" ];"));
        assert!(nix.contains("custom = true;"));
        let round_trip = scan_users(&nix);
        assert_eq!(round_trip, users);
    }

    #[test]
    fn an_untouched_row_is_unchanged() {
        let users = scan_users("{ alice = { isNormalUser = true; }; }");
        let fields = users[0].clone();
        assert_eq!(fields, fields.clone());
    }

    #[test]
    fn validation_requires_a_normal_sudo_user_with_a_password() {
        let missing_admin = vec![UserFields {
            name: "alice".into(),
            is_normal_user: true,
            password: "!".into(),
            ..UserFields::default()
        }];
        assert!(validation_error(&missing_admin).is_some());

        let valid = vec![UserFields {
            name: "alice".into(),
            is_normal_user: true,
            wheel: true,
            password: "$6$hash".into(),
            ..UserFields::default()
        }];
        assert_eq!(validation_error(&valid), None);
    }

    #[test]
    fn validation_rejects_empty_and_duplicate_accounts() {
        assert!(validation_error(&[]).is_some());
        let duplicate = vec![
            UserFields {
                name: "alice".into(),
                is_normal_user: true,
                wheel: true,
                password: "$6$hash".into(),
                ..UserFields::default()
            },
            UserFields {
                name: "alice".into(),
                ..UserFields::default()
            },
        ];
        assert!(validation_error(&duplicate).is_some());
    }

    fn admin(name: &str) -> UserFields {
        UserFields {
            name: name.into(),
            is_normal_user: true,
            wheel: true,
            password: "$6$hash".into(),
            ..UserFields::default()
        }
    }

    #[test]
    fn validation_rejects_invalid_names_and_groups() {
        let mut user = admin("alice");
        user.name = "bad name".into();
        assert!(validation_error(&[user]).is_some());

        let mut user = admin("alice");
        user.name = "1alice".into();
        assert!(validation_error(&[user]).is_some());

        let mut user = admin("alice");
        user.groups = vec!["bad group".into()];
        assert!(validation_error(&[user]).is_some());
    }

    #[test]
    fn validation_rejects_invalid_and_duplicate_uids() {
        let mut user = admin("alice");
        user.uid = "1000".into();
        assert_eq!(validation_error(&[user.clone()]), None);

        user.uid = "0".into();
        assert!(validation_error(&[user.clone()]).is_some());

        user.uid = "abc".into();
        assert!(validation_error(&[user]).is_some());

        let first = UserFields {
            uid: "1000".into(),
            ..admin("alice")
        };
        let second = UserFields {
            uid: "1000".into(),
            ..admin("bob")
        };
        assert!(validation_error(&[first, second]).is_some());
    }

    #[test]
    fn validation_rejects_invalid_shell_expressions() {
        let mut user = admin("alice");
        user.shell = "pkgs.zsh".into();
        assert_eq!(validation_error(&[user.clone()]), None);

        user.shell = "pkgs.kdePackages.konsole".into();
        assert_eq!(validation_error(&[user.clone()]), None);

        user.shell = "zsh".into();
        assert!(validation_error(&[user]).is_some());
    }

    #[test]
    fn password_set_check_ignores_locked_markers() {
        assert!(!password_is_set(""));
        assert!(!password_is_set("!"));
        assert!(!password_is_set("*"));
        assert!(password_is_set("$6$hash"));
    }

    #[test]
    fn scans_home_block_and_derives_identity_fields() {
        let entries = scan_home_block(
            r#"{
                alice = {
                    home.stateVersion = "25.05";
                    home.username = "alice";
                    home.homeDirectory = "/home/alice";
                    imports = [ self.modules.homeManager.foo ];
                };
            }"#,
        );
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].0, "alice");
        assert_eq!(entries[0].1.state_version, "25.05");
        assert!(entries[0]
            .1
            .extras
            .contains("imports = [ self.modules.homeManager.foo ];"));
        assert!(!entries[0].1.extras.contains("home.username"));
        assert!(!entries[0].1.extras.contains("home.homeDirectory"));

        let nix = serialize_home(&entries);
        assert!(nix.contains("home.stateVersion = \"25.05\";"));
        assert!(nix.contains("imports = [ self.modules.homeManager.foo ];"));
        assert!(!nix.contains("home.username"));
    }

    #[test]
    fn scans_dotted_home_assignments() {
        let assignment = |segments: &[&str], value: &str| scan::SiblingAssignment {
            segments: segments
                .iter()
                .map(|segment| (*segment).to_string())
                .collect(),
            value: value.to_string(),
            value_start: 0,
            value_end: 0,
            full_start: 0,
            full_end: 0,
        };
        let dotted = assignment(
            &["home-manager", "users", "alice", "home", "stateVersion"],
            "\"25.05\"",
        );
        let imports = assignment(
            &["home-manager", "users", "alice", "imports"],
            "[ self.modules.homeManager.foo ]",
        );
        let entries = scan_home_dotted(&[&dotted, &imports]);
        assert_eq!(entries[0].1.state_version, "25.05");
        assert!(entries[0]
            .1
            .extras
            .contains("imports = [ self.modules.homeManager.foo ];"));
    }

    fn home_record(text: &str) -> (std::path::PathBuf, Record) {
        let dir = std::env::temp_dir().join(format!(
            "nsg-users-home-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("users.nix");
        std::fs::write(&path, text).unwrap();
        let scan = crate::scan::scan_file(&path, text);
        let record = scan
            .records
            .into_iter()
            .find(|record| record.path() == "config.users.users")
            .unwrap();
        (dir, record)
    }

    #[test]
    fn loads_home_manager_block_from_the_sibling_assignment() {
        let text = r#"{ ... }: {
  config = {
    /* @option widget = "users"; label = "Accounts"; homeManager = true; */
    users.users = { user = { isNormalUser = true; }; };
    home-manager.users = {
      user = {
        home.stateVersion = "25.05";
        imports = [ self.modules.homeManager.foo ];
      };
    };
  };
}
"#;
        let (dir, record) = home_record(text);
        let home = HomeUsers::load(&record);
        assert_eq!(home.entry("user").state_version, "25.05");
        assert!(home.entry("user").extras.contains("imports ="));
        assert!(matches!(home.spans, HomeSpans::Block { .. }));
        assert_eq!(home.orig_nix, serialize_home(&home.entries));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn loads_dotted_home_manager_assignments() {
        let text = r#"{ ... }: {
  config = {
    /* @option widget = "users"; label = "Accounts"; homeManager = true; */
    users.users = { user = { isNormalUser = true; }; };
    home-manager.users.user.home.stateVersion = "25.05";
  };
}
"#;
        let (dir, record) = home_record(text);
        let home = HomeUsers::load(&record);
        assert_eq!(home.entry("user").state_version, "25.05");
        assert!(matches!(home.spans, HomeSpans::Dotted { .. }));
        let _ = std::fs::remove_dir_all(dir);
    }
}
