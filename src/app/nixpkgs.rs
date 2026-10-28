use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;

use adw::prelude::*;
use gtk::gio;
use gtk::glib;

use crate::flake_inputs::{self, FlakeInput, InputDocument};
use crate::flake_lock;
use crate::flake_ref::{self, FlakeRef, Query};
use crate::nixpkgs as nixpkgs_model;
use crate::ref_suggest;

use super::inputs::{flake_file, save_edits};
use super::pump::show_error;
use super::state::App;

type ScanTask = Box<dyn FnOnce() -> Result<Vec<String>, String> + Send>;
type Done = Rc<dyn Fn(&[String])>;

/// The nixpkgs card shown above the regular input list.
pub(super) fn section(app: &Rc<App>, document: &InputDocument) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::new();
    group.set_title("Nixpkgs");
    let input = document.input(nixpkgs_model::INPUT_NAME).cloned();

    let row = adw::ActionRow::new();
    row.set_title("Source");
    let button = gtk::Button::new();
    button.set_valign(gtk::Align::Center);
    let mut lock_row = None;

    match &input {
        Some(input) => {
            let url = input.url.clone().unwrap_or_default();
            row.set_subtitle(if url.is_empty() { "No URL set" } else { &url });
            button.set_label("Configure");
            let locked =
                flake_lock::read_input(Path::new(&flake_file(app)), nixpkgs_model::INPUT_NAME);
            let pinned = adw::ActionRow::new();
            pinned.set_title("Pinned revision");
            match locked {
                Some(locked) => {
                    let rev = locked
                        .rev
                        .as_deref()
                        .map(short_rev)
                        .unwrap_or_else(|| "unknown".to_string());
                    let date = locked.last_modified.map(format_time).unwrap_or_default();
                    pinned.set_subtitle(&format!("{rev}{date}"));
                }
                None => pinned.set_subtitle("Not present in flake.lock"),
            }
            lock_row = Some(pinned);
        }
        None => {
            row.set_subtitle("No nixpkgs input is declared in flake.nix");
            button.set_label("Add nixpkgs");
        }
    }

    row.add_suffix(&button);
    group.add(&row);
    if let Some(lock_row) = lock_row {
        group.add(&lock_row);
    }
    {
        let app = app.clone();
        button.connect_clicked(move |_| open_dialog(&app, input.clone()));
    }
    group
}

fn short_rev(rev: &str) -> String {
    rev.chars().take(12).collect()
}

fn format_time(seconds: i64) -> String {
    glib::DateTime::from_unix_local(seconds)
        .ok()
        .and_then(|date| date.format("%Y-%m-%d").ok())
        .map(|text| format!(" · {text}"))
        .unwrap_or_default()
}

#[derive(Clone)]
struct FormState {
    kind: String,
    channel: String,
    channel_rev: String,
    indirect_id: String,
    indirect_ref: String,
    indirect_rev: String,
    host: String,
    owner: String,
    repo: String,
    selector_kind: String,
    selector: String,
    commit: String,
    transport: String,
    url: String,
    ref_: String,
    rev: String,
    dir: String,
    nar_hash: String,
    shallow: bool,
    submodules: bool,
    lfs: bool,
}

impl Default for FormState {
    fn default() -> Self {
        Self {
            kind: "channel".to_string(),
            channel: "nixos-unstable".to_string(),
            channel_rev: String::new(),
            indirect_id: "nixpkgs".to_string(),
            indirect_ref: String::new(),
            indirect_rev: String::new(),
            host: String::new(),
            owner: String::new(),
            repo: String::new(),
            selector_kind: "branch".to_string(),
            selector: String::new(),
            commit: String::new(),
            transport: "https".to_string(),
            url: String::new(),
            ref_: String::new(),
            rev: String::new(),
            dir: String::new(),
            nar_hash: String::new(),
            shallow: false,
            submodules: false,
            lfs: false,
        }
    }
}

fn state_from_input(input: Option<&FlakeInput>) -> FormState {
    let mut state = FormState::default();
    let Some(url) = input.and_then(|input| input.url.as_deref()) else {
        return state;
    };
    let Ok(reference) = flake_ref::parse(url) else {
        return state;
    };
    match reference {
        FlakeRef::Github {
            owner,
            repo,
            selector,
            query,
        } => {
            let is_nixpkgs = owner.eq_ignore_ascii_case("NixOS") && repo == "nixpkgs";
            let channel_selector = selector
                .as_deref()
                .map(|selector| selector.starts_with("nixos-"))
                .unwrap_or(true);
            if is_nixpkgs && channel_selector {
                state.kind = "channel".to_string();
                state.channel = selector
                    .clone()
                    .unwrap_or_else(|| "nixos-unstable".to_string());
            } else {
                apply_hosted(&mut state, "github", owner, repo, selector, query);
            }
        }
        FlakeRef::Gitlab {
            owner,
            repo,
            selector,
            query,
        } => apply_hosted(&mut state, "gitlab", owner, repo, selector, query),
        FlakeRef::Sourcehut {
            owner,
            repo,
            selector,
            query,
        } => apply_hosted(&mut state, "sourcehut", owner, repo, selector, query),
        FlakeRef::Git {
            transport,
            url,
            query,
        } => {
            state.kind = "git".to_string();
            state.transport = transport.id().to_string();
            state.url = url;
            apply_query(&mut state, query);
        }
        FlakeRef::Mercurial {
            transport,
            url,
            query,
        } => {
            state.kind = "mercurial".to_string();
            state.transport = transport.id().to_string();
            state.url = url;
            apply_query(&mut state, query);
        }
        FlakeRef::Tarball {
            transport,
            url,
            nar_hash,
        } => {
            state.kind = "tarball".to_string();
            state.transport = transport.id().to_string();
            state.url = url;
            state.nar_hash = nar_hash.unwrap_or_default();
        }
        FlakeRef::File {
            transport,
            url,
            nar_hash,
        } => {
            state.kind = "file".to_string();
            state.transport = transport.id().to_string();
            state.url = url;
            state.nar_hash = nar_hash.unwrap_or_default();
        }
        FlakeRef::Path { path } => {
            state.kind = "path".to_string();
            state.url = path;
        }
        FlakeRef::Indirect { id, ref_, rev } => {
            state.kind = "indirect".to_string();
            state.indirect_id = id;
            state.indirect_ref = ref_.unwrap_or_default();
            state.indirect_rev = rev.unwrap_or_default();
        }
    }
    state
}

fn apply_hosted(
    state: &mut FormState,
    kind: &str,
    owner: String,
    repo: String,
    selector: Option<String>,
    query: Query,
) {
    state.kind = kind.to_string();
    state.owner = owner;
    state.repo = repo;
    match selector {
        Some(selector) if looks_like_rev(&selector) => state.commit = selector,
        Some(selector) => state.selector = selector,
        None => {}
    }
    apply_query(state, query);
}

fn apply_query(state: &mut FormState, query: Query) {
    state.host = query.host.unwrap_or_default();
    state.dir = query.dir.unwrap_or_default();
    state.ref_ = query.ref_.unwrap_or_default();
    state.rev = query.rev.unwrap_or_default();
    state.shallow = query.shallow;
    state.submodules = query.submodules;
    state.lfs = query.lfs;
    if state.nar_hash.is_empty() {
        state.nar_hash = query.nar_hash.unwrap_or_default();
    }
}

fn looks_like_rev(value: &str) -> bool {
    value.len() == 40 && value.chars().all(|character| character.is_ascii_hexdigit())
}

fn build_ref(state: &FormState) -> Result<FlakeRef, String> {
    match state.kind.as_str() {
        "channel" => {
            let channel = required("channel", &state.channel)?;
            let selector = if state.channel_rev.trim().is_empty() {
                channel
            } else {
                state.channel_rev.trim().to_string()
            };
            Ok(FlakeRef::Github {
                owner: "NixOS".to_string(),
                repo: "nixpkgs".to_string(),
                selector: Some(selector),
                query: query_from(state),
            })
        }
        "indirect" => Ok(FlakeRef::Indirect {
            id: required("registry id", &state.indirect_id)?,
            ref_: optional(&state.indirect_ref),
            rev: optional(&state.indirect_rev),
        }),
        "github" | "gitlab" | "sourcehut" => {
            let owner = required("owner", &state.owner)?;
            let owner = if state.kind == "sourcehut" {
                sourcehut_owner(&owner)
            } else {
                owner
            };
            let repo = required("repository", &state.repo)?;
            let selector = if state.commit.trim().is_empty() {
                optional(&state.selector)
            } else {
                optional(&state.commit)
            };
            let query = query_from(state);
            let reference = match state.kind.as_str() {
                "github" => FlakeRef::Github {
                    owner,
                    repo,
                    selector,
                    query,
                },
                "gitlab" => FlakeRef::Gitlab {
                    owner,
                    repo,
                    selector,
                    query,
                },
                _ => FlakeRef::Sourcehut {
                    owner,
                    repo,
                    selector,
                    query,
                },
            };
            Ok(reference)
        }
        "git" => {
            let transport = flake_ref::git_transport(&state.transport)
                .ok_or_else(|| "select a git transport".to_string())?;
            let mut query = query_from(state);
            query.ref_ = optional(&state.ref_);
            query.rev = optional(&state.rev);
            Ok(FlakeRef::Git {
                transport,
                url: required("repository URL", &state.url)?,
                query,
            })
        }
        "mercurial" => {
            let transport = flake_ref::mercurial_transport(&state.transport)
                .ok_or_else(|| "select a Mercurial transport".to_string())?;
            let mut query = query_from(state);
            query.ref_ = optional(&state.ref_);
            query.rev = optional(&state.rev);
            Ok(FlakeRef::Mercurial {
                transport,
                url: required("repository URL", &state.url)?,
                query,
            })
        }
        "path" => Ok(FlakeRef::Path {
            path: required("path", &state.url)?,
        }),
        "tarball" => {
            let transport = flake_ref::file_transport(&state.transport)
                .ok_or_else(|| "select a tarball transport".to_string())?;
            Ok(FlakeRef::Tarball {
                transport,
                url: required("tarball URL", &state.url)?,
                nar_hash: optional(&state.nar_hash),
            })
        }
        "file" => {
            let transport = flake_ref::file_transport(&state.transport)
                .ok_or_else(|| "select a file transport".to_string())?;
            Ok(FlakeRef::File {
                transport,
                url: required("file URL", &state.url)?,
                nar_hash: optional(&state.nar_hash),
            })
        }
        other => Err(format!("unknown flake location type `{other}`")),
    }
}

fn query_from(state: &FormState) -> Query {
    Query {
        dir: optional(&state.dir),
        ref_: None,
        rev: None,
        host: optional(&state.host),
        nar_hash: None,
        shallow: state.shallow,
        submodules: state.submodules,
        lfs: state.lfs,
    }
}

fn optional(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn required(label: &str, value: &str) -> Result<String, String> {
    optional(value).ok_or_else(|| format!("Enter a {label}"))
}

fn sourcehut_owner(owner: &str) -> String {
    if owner.starts_with('~') {
        owner.to_string()
    } else {
        format!("~{owner}")
    }
}

#[derive(Clone)]
struct SuggestField {
    entry: gtk::Entry,
    store: gtk::ListStore,
    button: gtk::Button,
}

impl SuggestField {
    fn new(value: &str, placeholder: &str, scan_label: &str) -> Self {
        let entry = gtk::Entry::new();
        entry.set_text(value);
        entry.set_hexpand(true);
        entry.set_placeholder_text(Some(placeholder));
        let store = gtk::ListStore::new(&[glib::Type::STRING]);
        let completion = gtk::EntryCompletion::new();
        completion.set_model(Some(&store));
        completion.set_text_column(0);
        completion.set_minimum_key_length(0);
        entry.set_completion(Some(&completion));
        let button = gtk::Button::with_label(scan_label);
        button.set_valign(gtk::Align::Center);
        Self {
            entry,
            store,
            button,
        }
    }

    fn fill(&self, values: &[String]) {
        self.store.clear();
        for value in values {
            self.store.set(&self.store.append(), &[(0, value)]);
        }
        self.entry.grab_focus();
        if let Some(completion) = self.entry.completion() {
            completion.complete();
        }
    }

    fn append_to(&self, container: &gtk::Box) {
        container.append(&self.entry);
        container.append(&self.button);
    }
}

struct FormGrid<'a> {
    grid: &'a gtk::Grid,
    row: i32,
}

impl<'a> FormGrid<'a> {
    fn new(grid: &'a gtk::Grid) -> Self {
        Self { grid, row: 0 }
    }

    fn add<W: IsA<gtk::Widget>>(&mut self, label: &str, widget: &W) {
        self.grid
            .attach(&gtk::Label::new(Some(label)), 0, self.row, 1, 1);
        self.grid.attach(widget, 1, self.row, 1, 1);
        self.row += 1;
    }

    fn field(&mut self, label: &str, field: &SuggestField) {
        let container = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        field.append_to(&container);
        self.add(label, &container);
    }

    fn switch(&mut self, label: &str, switch: &gtk::Switch) {
        switch.set_halign(gtk::Align::Start);
        self.add(label, switch);
    }

    fn note(&mut self, text: &str) {
        let label = gtk::Label::builder()
            .label(text)
            .xalign(0.0)
            .wrap(true)
            .css_classes(["dim-label"])
            .build();
        self.grid.attach(&label, 0, self.row, 2, 1);
        self.row += 1;
    }

    fn span(&mut self, widget: &impl IsA<gtk::Widget>) {
        self.grid.attach(widget, 0, self.row, 2, 1);
        self.row += 1;
    }
}

fn text_entry(value: &str, placeholder: &str) -> gtk::Entry {
    let entry = gtk::Entry::new();
    entry.set_text(value);
    entry.set_hexpand(true);
    entry.set_placeholder_text(Some(placeholder));
    entry
}

fn transport_combo(ids: &[&str], current: &str) -> gtk::ComboBoxText {
    let combo = gtk::ComboBoxText::new();
    for id in ids {
        combo.append(Some(id), id);
    }
    if !ids.contains(&current) {
        combo.set_active_id(ids.first().copied());
    } else {
        combo.set_active_id(Some(current));
    }
    combo
}

type TextSetter = fn(&mut FormState, String);
type BoolSetter = fn(&mut FormState, bool);

fn bind_entry(
    entry: &gtk::Entry,
    state: &Rc<RefCell<FormState>>,
    update: &Rc<dyn Fn()>,
    setter: TextSetter,
) {
    let state = state.clone();
    let update = update.clone();
    entry.connect_changed(move |entry| {
        setter(&mut state.borrow_mut(), entry.text().to_string());
        update();
    });
}

fn bind_combo(
    combo: &gtk::ComboBoxText,
    state: &Rc<RefCell<FormState>>,
    update: &Rc<dyn Fn()>,
    setter: TextSetter,
) {
    let state = state.clone();
    let update = update.clone();
    combo.connect_changed(move |combo| {
        if let Some(id) = combo.active_id() {
            setter(&mut state.borrow_mut(), id.to_string());
            update();
        }
    });
}

fn bind_switch(
    switch: &gtk::Switch,
    state: &Rc<RefCell<FormState>>,
    update: &Rc<dyn Fn()>,
    setter: BoolSetter,
) {
    let state = state.clone();
    let update = update.clone();
    switch.connect_active_notify(move |switch| {
        setter(&mut state.borrow_mut(), switch.is_active());
        update();
    });
}

fn run_scan(
    button: &gtk::Button,
    error: &gtk::Label,
    cache: &Rc<RefCell<HashMap<String, Vec<String>>>>,
    make: impl Fn() -> Result<(String, ScanTask), String> + 'static,
    done: impl Fn(&[String]) + 'static,
) {
    let button = button.clone();
    let handler = button.clone();
    let error = error.clone();
    let cache = cache.clone();
    let done: Done = Rc::new(done);
    handler.connect_clicked(move |_| {
        error.set_text("");
        let (key, task) = match make() {
            Ok(value) => value,
            Err(message) => {
                error.set_text(&message);
                return;
            }
        };
        if let Some(values) = cache.borrow().get(&key).cloned() {
            done(&values);
            return;
        }
        let button = button.clone();
        let error = error.clone();
        let cache = cache.clone();
        let done = done.clone();
        button.set_sensitive(false);
        let (sender, receiver) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let _ = sender.send_blocking(task());
        });
        glib::MainContext::default().spawn_local(async move {
            let result = receiver
                .recv()
                .await
                .unwrap_or_else(|_| Err("scan stopped unexpectedly".to_string()));
            button.set_sensitive(true);
            match result {
                Ok(values) if values.is_empty() => error.set_text("Scan returned no results"),
                Ok(values) => {
                    cache.borrow_mut().insert(key, values.clone());
                    done(&values);
                }
                Err(message) => error.set_text(&message),
            }
        });
    });
}

fn set_text(entry: &gtk::Entry, value: &str) {
    entry.set_text(value);
}

fn build_form(
    app: &Rc<App>,
    grid: &gtk::Grid,
    state: &Rc<RefCell<FormState>>,
    update: &Rc<dyn Fn()>,
    cache: &Rc<RefCell<HashMap<String, Vec<String>>>>,
    error: &gtk::Label,
) {
    while let Some(child) = grid.first_child() {
        grid.remove(&child);
    }
    let kind = state.borrow().kind.clone();
    let mut form = FormGrid::new(grid);

    match kind.as_str() {
        "channel" => {
            let combo = gtk::ComboBoxText::new();
            let current = state.borrow().channel.clone();
            let channels = nixpkgs_model::channels();
            for channel in &channels {
                combo.append(Some(channel), channel);
            }
            let current = if current.trim().is_empty() {
                "nixos-unstable".to_string()
            } else {
                current
            };
            if !channels.iter().any(|channel| channel == &current) {
                combo.append(Some(&current), &current);
            }
            combo.set_active_id(Some(&current));
            bind_combo(&combo, state, update, |state, value| state.channel = value);
            let scan = gtk::Button::with_label("Scan channels");
            scan.set_valign(gtk::Align::Center);
            let container = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            combo.set_hexpand(true);
            container.append(&combo);
            container.append(&scan);
            form.add("Channel", &container);
            let combo_for_scan = combo.clone();
            let state_for_scan = state.clone();
            let update_for_scan = update.clone();
            run_scan(
                &scan,
                error,
                cache,
                || {
                    Ok((
                        "channels".to_string(),
                        Box::new(nixpkgs_model::scan_channels),
                    ))
                },
                move |values| {
                    combo_for_scan.remove_all();
                    for value in values {
                        combo_for_scan.append(Some(value), value);
                    }
                    if combo_for_scan.active_id().is_none() {
                        if let Some(first) = values.first() {
                            combo_for_scan.set_active_id(Some(first));
                            state_for_scan.borrow_mut().channel = first.clone();
                            update_for_scan();
                        }
                    }
                },
            );

            let commit = SuggestField::new(
                &state.borrow().channel_rev,
                "Optional commit hash",
                "Scan commits",
            );
            bind_entry(&commit.entry, state, update, |state, value| {
                state.channel_rev = value
            });
            form.field("Commit (optional)", &commit);
            let state_for_scan = state.clone();
            run_scan(
                &commit.button,
                error,
                cache,
                move || {
                    let channel = state_for_scan.borrow().channel.clone();
                    let channel = required("channel", &channel)?;
                    let key = format!("channel-commits:{channel}");
                    let branch = channel.clone();
                    Ok((
                        key,
                        Box::new(move || {
                            ref_suggest::scan_commits("github", None, "NixOS", "nixpkgs", &branch)
                        }),
                    ))
                },
                {
                    let commit = commit.clone();
                    move |values| commit.fill(values)
                },
            );
            form.note("Channels resolve to github:NixOS/nixpkgs/<branch>.");
        }
        "indirect" => {
            let id = SuggestField::new(&state.borrow().indirect_id, "nixpkgs", "Scan registry");
            bind_entry(&id.entry, state, update, |state, value| {
                state.indirect_id = value
            });
            form.field("Registry id", &id);
            run_scan(
                &id.button,
                error,
                cache,
                || Ok(("registry".to_string(), Box::new(ref_suggest::scan_registry))),
                {
                    let id = id.clone();
                    move |values| id.fill(values)
                },
            );
            let ref_entry = text_entry(&state.borrow().indirect_ref, "Optional branch or tag");
            bind_entry(&ref_entry, state, update, |state, value| {
                state.indirect_ref = value
            });
            form.add("Ref (optional)", &ref_entry);
            let rev_entry = text_entry(&state.borrow().indirect_rev, "Optional commit");
            bind_entry(&rev_entry, state, update, |state, value| {
                state.indirect_rev = value
            });
            form.add("Commit (optional)", &rev_entry);
            form.note("Uses the entry for this id in the flake registry.");
        }
        "github" | "gitlab" | "sourcehut" => {
            let host = text_entry(&state.borrow().host, "Optional host");
            bind_entry(&host, state, update, |state, value| state.host = value);
            form.add("Host (optional)", &host);

            let owner = SuggestField::new(
                &state.borrow().owner,
                if kind == "sourcehut" {
                    "~owner"
                } else {
                    "owner"
                },
                "Scan owners",
            );
            bind_entry(&owner.entry, state, update, |state, value| {
                state.owner = value
            });
            form.field("User or organization", &owner);
            let state_for_scan = state.clone();
            let kind_for_owner = kind.clone();
            run_scan(
                &owner.button,
                error,
                cache,
                move || {
                    let query = state_for_scan.borrow().owner.clone();
                    let query = required("user or organization", &query)?;
                    let key = format!("users:{kind_for_owner}:{query}");
                    let kind = kind_for_owner.clone();
                    Ok((
                        key,
                        Box::new(move || ref_suggest::scan_users(&kind, &query)),
                    ))
                },
                {
                    let owner = owner.clone();
                    move |values| owner.fill(values)
                },
            );

            let repo = SuggestField::new(&state.borrow().repo, "repository", "Scan repositories");
            bind_entry(&repo.entry, state, update, |state, value| {
                state.repo = value
            });
            form.field("Repository", &repo);
            let state_for_scan = state.clone();
            let kind_for_repo = kind.clone();
            run_scan(
                &repo.button,
                error,
                cache,
                move || {
                    let owner = state_for_scan.borrow().owner.clone();
                    let owner = required("owner", &owner)?;
                    let host = state_for_scan.borrow().host.clone();
                    let key = format!("repos:{kind_for_repo}:{}:{owner}", host.trim());
                    let kind = kind_for_repo.clone();
                    let host = if host.trim().is_empty() {
                        None
                    } else {
                        Some(host)
                    };
                    Ok((
                        key,
                        Box::new(move || ref_suggest::scan_repos(&kind, host.as_deref(), &owner)),
                    ))
                },
                {
                    let repo = repo.clone();
                    move |values| repo.fill(values)
                },
            );

            let selector_kind = gtk::ComboBoxText::new();
            selector_kind.append(Some("branch"), "Branch");
            selector_kind.append(Some("tag"), "Tag");
            selector_kind.set_active_id(Some(&state.borrow().selector_kind));
            bind_combo(&selector_kind, state, update, |state, value| {
                state.selector_kind = value
            });
            form.add("Ref kind", &selector_kind);

            let selector =
                SuggestField::new(&state.borrow().selector, "branch or tag", "Scan refs");
            bind_entry(&selector.entry, state, update, |state, value| {
                state.selector = value
            });
            form.field("Ref", &selector);
            let state_for_scan = state.clone();
            let kind_for_refs = kind.clone();
            run_scan(
                &selector.button,
                error,
                cache,
                move || {
                    let owner = state_for_scan.borrow().owner.clone();
                    let owner = required("owner", &owner)?;
                    let repo = state_for_scan.borrow().repo.clone();
                    let repo = required("repository", &repo)?;
                    let host = state_for_scan.borrow().host.clone();
                    let tag = state_for_scan.borrow().selector_kind == "tag";
                    let key = format!("refs:{kind_for_refs}:{}:{owner}:{repo}:{tag}", host.trim());
                    let kind = kind_for_refs.clone();
                    let host = if host.trim().is_empty() {
                        None
                    } else {
                        Some(host)
                    };
                    Ok((
                        key,
                        Box::new(move || {
                            ref_suggest::scan_refs(&kind, host.as_deref(), &owner, &repo, tag)
                        }),
                    ))
                },
                {
                    let selector = selector.clone();
                    move |values| selector.fill(values)
                },
            );

            let commit = SuggestField::new(
                &state.borrow().commit,
                "Optional commit hash",
                "Scan commits",
            );
            bind_entry(&commit.entry, state, update, |state, value| {
                state.commit = value
            });
            form.field("Commit (optional)", &commit);
            let state_for_scan = state.clone();
            let kind_for_commits = kind.clone();
            run_scan(
                &commit.button,
                error,
                cache,
                move || {
                    let owner = state_for_scan.borrow().owner.clone();
                    let owner = required("owner", &owner)?;
                    let repo = state_for_scan.borrow().repo.clone();
                    let repo = required("repository", &repo)?;
                    let branch = state_for_scan.borrow().selector.clone();
                    let branch = required("ref", &branch)?;
                    let host = state_for_scan.borrow().host.clone();
                    let key = format!(
                        "commits:{kind_for_commits}:{}:{owner}:{repo}:{branch}",
                        host.trim()
                    );
                    let kind = kind_for_commits.clone();
                    let host = if host.trim().is_empty() {
                        None
                    } else {
                        Some(host)
                    };
                    Ok((
                        key,
                        Box::new(move || {
                            ref_suggest::scan_commits(
                                &kind,
                                host.as_deref(),
                                &owner,
                                &repo,
                                &branch,
                            )
                        }),
                    ))
                },
                {
                    let commit = commit.clone();
                    move |values| commit.fill(values)
                },
            );

            let dir = text_entry(&state.borrow().dir, "Optional subdirectory");
            bind_entry(&dir, state, update, |state, value| state.dir = value);
            form.add("Directory (optional)", &dir);

            let submodules = gtk::Switch::new();
            submodules.set_active(state.borrow().submodules);
            bind_switch(&submodules, state, update, |state, value| {
                state.submodules = value
            });
            form.switch("Fetch git submodules", &submodules);

            let lfs = gtk::Switch::new();
            lfs.set_active(state.borrow().lfs);
            bind_switch(&lfs, state, update, |state, value| state.lfs = value);
            form.switch("Fetch git LFS files", &lfs);
        }
        "git" => {
            let transport = transport_combo(
                &["https", "http", "ssh", "git", "file"],
                &state.borrow().transport,
            );
            bind_combo(&transport, state, update, |state, value| {
                state.transport = value
            });
            form.add("Transport", &transport);

            let url = text_entry(&state.borrow().url, "example.org/owner/repo");
            bind_entry(&url, state, update, |state, value| state.url = value);
            form.add("Repository URL", &url);

            let ref_entry = SuggestField::new(&state.borrow().ref_, "branch or tag", "Scan refs");
            bind_entry(&ref_entry.entry, state, update, |state, value| {
                state.ref_ = value
            });
            form.field("Ref", &ref_entry);
            let state_for_scan = state.clone();
            run_scan(
                &ref_entry.button,
                error,
                cache,
                move || {
                    let url = state_for_scan.borrow().url.clone();
                    let url = required("repository URL", &url)?;
                    let key = format!("git-refs:{url}");
                    Ok((key, Box::new(move || ref_suggest::scan_git_refs(&url))))
                },
                {
                    let ref_entry = ref_entry.clone();
                    move |values| ref_entry.fill(values)
                },
            );

            let rev_entry = SuggestField::new(&state.borrow().rev, "commit hash", "Scan commits");
            bind_entry(&rev_entry.entry, state, update, |state, value| {
                state.rev = value
            });
            form.field("Commit (optional)", &rev_entry);
            let state_for_scan = state.clone();
            run_scan(
                &rev_entry.button,
                error,
                cache,
                move || {
                    let url = state_for_scan.borrow().url.clone();
                    let url = required("repository URL", &url)?;
                    let branch = state_for_scan.borrow().ref_.clone();
                    let branch = required("ref", &branch)?;
                    let key = format!("git-commits:{url}:{branch}");
                    Ok((
                        key,
                        Box::new(move || ref_suggest::scan_git_commits(&url, &branch)),
                    ))
                },
                {
                    let rev_entry = rev_entry.clone();
                    move |values| rev_entry.fill(values)
                },
            );

            let dir = text_entry(&state.borrow().dir, "Optional subdirectory");
            bind_entry(&dir, state, update, |state, value| state.dir = value);
            form.add("Directory (optional)", &dir);

            let shallow = gtk::Switch::new();
            shallow.set_active(state.borrow().shallow);
            bind_switch(&shallow, state, update, |state, value| {
                state.shallow = value
            });
            form.switch("Shallow clone", &shallow);

            let submodules = gtk::Switch::new();
            submodules.set_active(state.borrow().submodules);
            bind_switch(&submodules, state, update, |state, value| {
                state.submodules = value
            });
            form.switch("Fetch git submodules", &submodules);

            let lfs = gtk::Switch::new();
            lfs.set_active(state.borrow().lfs);
            bind_switch(&lfs, state, update, |state, value| state.lfs = value);
            form.switch("Fetch git LFS files", &lfs);
        }
        "mercurial" => {
            let transport =
                transport_combo(&["https", "http", "ssh", "file"], &state.borrow().transport);
            bind_combo(&transport, state, update, |state, value| {
                state.transport = value
            });
            form.add("Transport", &transport);
            let url = text_entry(&state.borrow().url, "example.org/owner/repo");
            bind_entry(&url, state, update, |state, value| state.url = value);
            form.add("Repository URL", &url);
            let ref_entry = text_entry(&state.borrow().ref_, "Optional branch or tag");
            bind_entry(&ref_entry, state, update, |state, value| state.ref_ = value);
            form.add("Ref (optional)", &ref_entry);
            let rev_entry = text_entry(&state.borrow().rev, "Optional commit");
            bind_entry(&rev_entry, state, update, |state, value| state.rev = value);
            form.add("Commit (optional)", &rev_entry);
            form.note("Mercurial refs and commits are typed manually.");
        }
        "path" => {
            let path = text_entry(&state.borrow().url, "/path/to/flake");
            bind_entry(&path, state, update, |state, value| state.url = value);
            let browse = gtk::Button::with_label("Browse");
            let container = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            path.set_hexpand(true);
            container.append(&path);
            container.append(&browse);
            form.add("Path", &container);
            let app = app.clone();
            let path_for_dialog = path.clone();
            browse.connect_clicked(move |_| {
                let dialog = gtk::FileDialog::builder()
                    .title("Choose flake directory")
                    .modal(true)
                    .build();
                let path = path_for_dialog.clone();
                dialog.select_folder(
                    Some(&app.window),
                    None::<&gio::Cancellable>,
                    move |result| {
                        if let Ok(file) = result {
                            if let Some(path_value) = file.path() {
                                set_text(&path, &path_value.display().to_string());
                            }
                        }
                    },
                );
            });
        }
        "tarball" | "file" => {
            let transport = transport_combo(&["https", "http", "file"], &state.borrow().transport);
            bind_combo(&transport, state, update, |state, value| {
                state.transport = value
            });
            form.add("Transport", &transport);
            let url = text_entry(&state.borrow().url, "example.org/archive.tar.gz");
            bind_entry(&url, state, update, |state, value| state.url = value);
            form.add("URL", &url);
            let nar_hash = text_entry(&state.borrow().nar_hash, "Optional sha256-...");
            bind_entry(&nar_hash, state, update, |state, value| {
                state.nar_hash = value
            });
            form.add("NAR hash (optional)", &nar_hash);
        }
        _ => form.note("Unknown flake location type."),
    }

    form.span(error);
}

fn kinds() -> [(&'static str, &'static str); 10] {
    [
        ("channel", "NixOS channel"),
        ("indirect", "Registry (indirect)"),
        ("github", "GitHub"),
        ("gitlab", "GitLab"),
        ("sourcehut", "SourceHut"),
        ("git", "Git"),
        ("mercurial", "Mercurial"),
        ("path", "Path"),
        ("tarball", "Tarball"),
        ("file", "File"),
    ]
}

fn open_dialog(app: &Rc<App>, existing: Option<FlakeInput>) {
    let file = flake_file(app);
    let state = Rc::new(RefCell::new(state_from_input(existing.as_ref())));

    let dialog = gtk::Dialog::builder()
        .title("Configure nixpkgs")
        .transient_for(&app.window)
        .modal(true)
        .default_width(680)
        .build();
    dialog.add_button("Cancel", gtk::ResponseType::Cancel);
    let accept = dialog.add_button("Save", gtk::ResponseType::Accept);
    accept.add_css_class("suggested-action");

    let content = gtk::Box::new(gtk::Orientation::Vertical, 10);
    content.set_margin_top(12);
    content.set_margin_bottom(12);
    content.set_margin_start(12);
    content.set_margin_end(12);

    let kind_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    kind_row.append(&gtk::Label::new(Some("Flake location")));
    let kind_combo = gtk::ComboBoxText::new();
    for (id, label) in kinds() {
        kind_combo.append(Some(id), label);
    }
    kind_combo.set_hexpand(true);
    kind_combo.set_active_id(Some(&state.borrow().kind));
    kind_row.append(&kind_combo);
    content.append(&kind_row);

    let grid = gtk::Grid::builder()
        .row_spacing(8)
        .column_spacing(10)
        .build();
    content.append(&grid);

    let preview = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .selectable(true)
        .css_classes(["dim-label"])
        .build();
    content.append(&preview);
    let error = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .css_classes(["error"])
        .build();
    content.append(&error);

    let cache: Rc<RefCell<HashMap<String, Vec<String>>>> = Rc::new(RefCell::new(HashMap::new()));
    let update: Rc<dyn Fn()> = {
        let state = state.clone();
        let preview = preview.clone();
        Rc::new(move || {
            let text = match build_ref(&state.borrow()) {
                Ok(reference) => flake_ref::render(&reference),
                Err(message) => format!("Incomplete: {message}"),
            };
            preview.set_text(&text);
        })
    };

    build_form(app, &grid, &state, &update, &cache, &error);
    update();
    {
        let app = app.clone();
        let grid = grid.clone();
        let state = state.clone();
        let update = update.clone();
        let cache = cache.clone();
        let error = error.clone();
        kind_combo.connect_changed(move |combo| {
            let Some(id) = combo.active_id() else {
                return;
            };
            state.borrow_mut().kind = id.to_string();
            build_form(&app, &grid, &state, &update, &cache, &error);
            update();
        });
    }

    dialog.content_area().append(&content);
    {
        let app = app.clone();
        let file = file.clone();
        dialog.connect_response(move |dialog, response| {
            if response != gtk::ResponseType::Accept {
                dialog.close();
                return;
            }
            let reference = match build_ref(&state.borrow()) {
                Ok(reference) => reference,
                Err(message) => {
                    error.set_text(&message);
                    return;
                }
            };
            let document = match flake_inputs::load(&file) {
                Ok(document) => document,
                Err(message) => {
                    show_error(&app.window, &message);
                    return;
                }
            };
            let mut input = document
                .input(nixpkgs_model::INPUT_NAME)
                .cloned()
                .unwrap_or_else(|| FlakeInput::new(nixpkgs_model::INPUT_NAME));
            input.url = Some(flake_ref::render(&reference));
            match document.edits_for_upsert(&input) {
                Ok(edits) => {
                    save_edits(&app, edits);
                    dialog.close();
                }
                Err(message) => error.set_text(&message),
            }
        });
    }

    dialog.present();
}
