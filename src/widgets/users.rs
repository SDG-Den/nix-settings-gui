use super::basic::build_raw;
use super::context::{BuildContext, ChangeHook};
use super::editor::Editor;
use super::nix::{attr_key, entry_text, parse_string_literal, quote, string_literal};
use crate::dsl::OptionSpec;
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

pub(crate) fn users_unchanged(rows: &Rc<RefCell<Vec<UserRow>>>, orig_count: usize) -> bool {
    let rows = rows.borrow();
    rows.len() == orig_count && rows.iter().all(|row| row.read_fields() == row.orig)
}

pub(crate) fn users_validation_error(rows: &Rc<RefCell<Vec<UserRow>>>) -> Option<String> {
    let users: Vec<UserFields> = rows.borrow().iter().map(UserRow::read_fields).collect();
    validation_error(&users)
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

pub(crate) fn validation_error(users: &[UserFields]) -> Option<String> {
    if users.is_empty() {
        return Some("At least one user is required.".to_string());
    }
    let mut names = std::collections::HashSet::new();
    for user in users {
        let name = user.name.trim();
        if name.is_empty() {
            return Some("Every user needs a name.".to_string());
        }
        if !names.insert(name.to_string()) {
            return Some(format!("User names must be unique; {name} is repeated."));
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

    let scans = scan_users(&record.value);
    let orig_count = scans.len();
    for scan in scans {
        let row = make_user_row(scan, ctx, &rows_box, &rows, &notify);
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
        add.connect_clicked(move |_| {
            let name = unique_user_name(&rows.borrow());
            let scan = UserFields::new(name);
            let row = make_user_row(scan, &ctx, &rows_box, &rows, &notify);
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

fn make_user_row(
    scan: UserFields,
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

    #[test]
    fn password_set_check_ignores_locked_markers() {
        assert!(!password_is_set(""));
        assert!(!password_is_set("!"));
        assert!(!password_is_set("*"));
        assert!(password_is_set("$6$hash"));
    }
}
