use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

use crate::config::AppConfig;
use crate::model::Record;
use crate::nixd::OptionInfo;
use crate::scan::ScanResult;
use crate::widgets::Editor;

use super::apply::ApplyState;

pub(super) enum Msg {
    Scanned(Result<ScanResult, String>),
    Saved(Result<usize, String>),
    SaveProgress(f64, String),
    Inserted(Result<(), String>),
    SnippetInserted(Result<(), String>),
    Infos(String, Vec<(String, OptionInfo)>),
    Suggestions(Vec<String>),
    PluginRegistry(Result<Vec<crate::metadata::RegistryPlugin>, String>),
    Invalid(Vec<String>),
    GitPulled(Result<bool, String>),
    GitSynced(Result<String, String>),
    ApplyFinished {
        path: String,
        result: Result<(), String>,
    },
}

pub(super) type Tx = async_channel::Sender<Msg>;

/// A pending package-search result handler plus its request id, keyed in
/// `PackageService::callbacks`.
pub(super) type PackageCallback = Box<dyn FnOnce(Vec<String>)>;
pub(super) type PackageCallbacks = Rc<RefCell<HashMap<u64, PackageCallback>>>;

/// One rendered option in a menu: the record it edits, the live control, and
/// the widgets that let the row be filtered or refined after nixd answers.
pub(super) struct ControlRow {
    pub(super) record: Record,
    pub(super) editor: Rc<RefCell<Editor>>,
    pub(super) row: gtk::Box,
    pub(super) slot: gtk::Box,
    pub(super) desc: gtk::Label,
    pub(super) dirty: gtk::Label,
    pub(super) live: gtk::Image,
    pub(super) reset: gtk::Button,
    /// Position of this control's record in source order, used to preserve
    /// declaration order when no explicit `order` is set.
    pub(super) src_index: usize,
    /// Whether this control's `visibleWhen` dependency currently holds.
    pub(super) dep_visible: Cell<bool>,
}

impl ControlRow {
    pub(super) fn set_editor(&self, editor: Editor) {
        while let Some(child) = self.slot.first_child() {
            self.slot.remove(&child);
        }
        self.slot.append(&editor.root());
        self.slot.append(&self.reset);
        *self.editor.borrow_mut() = editor;
    }
}

pub(super) struct MenuPage {
    pub(super) file: String,
    pub(super) controls: Vec<Rc<ControlRow>>,
    pub(super) root: gtk::Widget,
    pub(super) content: gtk::Box,
    pub(super) group_headers: Vec<(bool, String, gtk::Label)>,
    pub(super) group_inserts: Vec<(String, gtk::Button)>,
}

/// An "Add <snippet>" button placed at an `@insert` marker's group.
pub(super) struct InsertButton {
    pub(super) group: String,
    pub(super) order: i64,
    pub(super) button: gtk::Button,
}

/// A single long-lived nixd process shared by all package searches. Keystrokes
/// are queued to a worker thread; results come back on the main thread keyed by
/// request id so several popovers can search concurrently.
pub(super) struct PackageService {
    requests: async_channel::Sender<(u64, String, String)>,
    callbacks: PackageCallbacks,
    next_id: Rc<Cell<u64>>,
}

impl PackageService {
    pub(super) fn new() -> Rc<Self> {
        let (requests, request_rx) = async_channel::bounded::<(u64, String, String)>(16);
        let (result_tx, result_rx) = async_channel::bounded::<(u64, Vec<String>)>(16);
        std::thread::spawn(move || {
            while let Ok((id, flake, term)) = request_rx.recv_blocking() {
                let flake_arg = if flake.is_empty() {
                    None
                } else {
                    Some(flake.as_str())
                };
                let names =
                    crate::nixd::complete_packages(flake_arg, &term, id).unwrap_or_default();
                let _ = result_tx.send_blocking((id, names));
            }
        });

        let callbacks: PackageCallbacks = Rc::new(RefCell::new(HashMap::new()));
        {
            let callbacks = callbacks.clone();
            glib::MainContext::default().spawn_local(async move {
                while let Ok((id, names)) = result_rx.recv().await {
                    if let Some(done) = callbacks.borrow_mut().remove(&id) {
                        done(names);
                    }
                }
            });
        }
        Rc::new(Self {
            requests,
            callbacks,
            next_id: Rc::new(Cell::new(0)),
        })
    }

    pub(super) fn search(&self, flake: String, term: String, done: PackageCallback) {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        self.callbacks.borrow_mut().insert(id, done);
        if self.requests.try_send((id, flake, term)).is_err() {
            self.callbacks.borrow_mut().remove(&id);
        }
    }
}

/// The DMS plugin registry id list, fetched at most once per flake open.
/// `pending` holds picker callbacks waiting on an in-flight fetch.
pub(super) struct PluginRegistryState {
    pub(super) cache: RefCell<Option<Vec<crate::metadata::RegistryPlugin>>>,
    pub(super) pending: RefCell<Vec<Box<dyn FnOnce(Vec<crate::metadata::RegistryPlugin>)>>>,
    pub(super) loading: Cell<bool>,
}

pub(super) struct App {
    pub(super) window: adw::ApplicationWindow,
    pub(super) title: gtk::Label,
    pub(super) flake_combo: gtk::ComboBoxText,
    pub(super) search: gtk::SearchEntry,
    pub(super) host_combo: gtk::ComboBoxText,
    pub(super) system_toggle: gtk::ToggleButton,
    pub(super) advanced_toggle: gtk::ToggleButton,
    pub(super) list: gtk::ListBox,
    pub(super) stack: gtk::Stack,
    pub(super) nixd_banner: adw::Banner,
    pub(super) status: gtk::Label,
    pub(super) spinner: gtk::Spinner,
    pub(super) apply_spinner: gtk::Spinner,
    pub(super) progress: gtk::ProgressBar,
    pub(super) package_service: Rc<PackageService>,
    pub(super) plugin_registry: Rc<PluginRegistryState>,
    pub(super) apply_state: Rc<ApplyState>,
    pub(super) tx: Tx,

    pub(super) config: RefCell<AppConfig>,
    pub(super) base: RefCell<String>,
    pub(super) scan: RefCell<Option<ScanResult>>,
    pub(super) targets: RefCell<Vec<String>>,
    pub(super) target: RefCell<String>,
    pub(super) hosts: RefCell<Vec<String>>,
    pub(super) current_host: RefCell<String>,
    pub(super) current_file: RefCell<String>,

    pub(super) flakes: RefCell<Vec<PathBuf>>,
    pub(super) pages: RefCell<Vec<MenuPage>>,
    pub(super) suggest_store: RefCell<Option<gtk::ListStore>>,
    pub(super) suggest_parent: RefCell<Option<String>>,
    pub(super) suggest_cache: RefCell<HashMap<String, Vec<String>>>,
    pub(super) system_only: Cell<bool>,
    pub(super) advanced: Cell<bool>,
    pub(super) manual_host: Cell<bool>,
    pub(super) pending_rebuild: Cell<bool>,
    pub(super) watchers: RefCell<Vec<gio::FileMonitor>>,
}
