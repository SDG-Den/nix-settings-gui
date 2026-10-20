use super::basic::clear_rows;
use super::context::{BuildContext, ChangeHook};
use super::names::widget_display_name;
use super::nix::truthy;
use super::tile::{attach_tile_dnd, make_field_for_preset, make_widget_tile, WidgetTile};
use super::tile_nix::{parse_string_list, valid_widget_id};
use crate::app::menu::pick_icon_name;
use crate::dsl::{PresetFieldSpec, WidgetPresetSpec};
use crate::scan;
use gtk::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
/// Which widget list a tile belongs to; drives the picker pool and the
/// serialization form (center is a plain string list).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum TileSection {
    BarLeft,
    BarCenter,
    BarRight,
    Dock,
}

impl TileSection {
    pub(crate) fn picker_filter(self) -> &'static str {
        match self {
            TileSection::BarLeft => "left",
            TileSection::BarCenter => "center",
            TileSection::BarRight => "right",
            TileSection::Dock => "dock",
        }
    }

    pub(crate) fn heading(self) -> &'static str {
        match self {
            TileSection::BarLeft => "Left widgets",
            TileSection::BarCenter => "Center widgets",
            TileSection::BarRight => "Right widgets",
            TileSection::Dock => "Widgets",
        }
    }

    pub(crate) fn is_center(self) -> bool {
        matches!(self, TileSection::BarCenter)
    }
}
/// A whole tile list: heading, tiles, and add buttons.
pub(crate) struct WidgetTileList {
    pub(crate) root: gtk::Box,
    pub(crate) tiles: Rc<RefCell<Vec<WidgetTile>>>,
    /// Original id sequence as scanned, for unchanged detection.
    pub(crate) orig_ids: Vec<String>,
}
/// Scaffold a tile from a catalog preset: `preset.defaults` become the
/// fields, marked as present so they always serialize.
pub(crate) fn add_tile_from_preset(
    preset: &WidgetPresetSpec,
    specs: &[PresetFieldSpec],
    section: TileSection,
    ctx: &BuildContext,
    tiles: &Rc<RefCell<Vec<WidgetTile>>>,
    target: &gtk::ListBox,
    on_change: &ChangeHook,
) {
    if tiles.borrow().iter().any(|tile| tile.id == preset.id) {
        return;
    }
    let fields = if section.is_center() {
        Rc::new(RefCell::new(Vec::new()))
    } else {
        Rc::new(RefCell::new(
            preset
                .defaults
                .iter()
                .filter(|(name, _)| name.as_str() != "enabled")
                .map(|(name, lit)| {
                    let spec = specs.iter().find(|spec| spec.name == *name);
                    let mut field = make_field_for_preset(name, &lit.to_nix(), spec, ctx);
                    field.orig_present = true;
                    field
                })
                .collect(),
        ))
    };
    let tile = make_widget_tile(
        &preset.id,
        "",
        "",
        Some(preset.clone()),
        fields,
        specs.to_vec(),
        section,
        ctx,
        tiles,
        target,
        on_change,
    );
    target.append(&tile.row);
    tiles.borrow_mut().push(tile);
    if let Some(cb) = on_change.borrow().as_ref() {
        cb();
    }
}

/// Refill the add-picker with this section's presets the list does not
/// already contain, filtered by the search term.
pub(crate) fn populate_widget_picker(
    picker_list: &gtk::ListBox,
    section: TileSection,
    presets: &[(WidgetPresetSpec, Vec<PresetFieldSpec>)],
    tiles: &Rc<RefCell<Vec<WidgetTile>>>,
    term: &str,
) {
    clear_rows(picker_list);
    let taken: Vec<String> = tiles.borrow().iter().map(|tile| tile.id.clone()).collect();
    let term = term.trim().to_lowercase();
    for (preset, _) in presets
        .iter()
        .filter(|(preset, _)| {
            preset
                .sections
                .iter()
                .any(|value| value == section.picker_filter())
        })
        .filter(|(preset, _)| !taken.iter().any(|id| id == &preset.id))
        .filter(|(preset, _)| {
            term.is_empty()
                || preset.label.to_lowercase().contains(&term)
                || preset.id.to_lowercase().contains(&term)
        })
    {
        let row = gtk::ListBoxRow::new();
        row.set_widget_name(&preset.id);
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        line.set_margin_top(6);
        line.set_margin_bottom(6);
        line.set_margin_start(8);
        line.set_margin_end(8);
        if let Some(icon) = &preset.icon {
            let resolved = gtk::gdk::Display::default().and_then(|display| {
                let theme = gtk::IconTheme::for_display(&display);
                pick_icon_name(|name| theme.has_icon(name), Some(icon.as_str()))
            });
            if let Some(name) = resolved {
                let image = gtk::Image::from_icon_name(&name);
                image.set_pixel_size(24);
                line.append(&image);
            }
        }
        let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
        text.set_hexpand(true);
        let heading = if preset.label.is_empty() {
            widget_display_name(&preset.id)
        } else {
            preset.label.clone()
        };
        let label = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .label(heading)
            .css_classes(["heading"])
            .build();
        text.append(&label);
        if let Some(description) = &preset.description {
            let dim = gtk::Label::builder()
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .label(description)
                .css_classes(["dim-label"])
                .build();
            text.append(&dim);
        }
        line.append(&text);
        row.set_child(Some(&line));
        picker_list.append(&row);
    }
}

impl WidgetTileList {
    /// Build one widget tile list: heading, tiles with drag reorder, and
    /// the add buttons. `value` is the raw nix of the list.
    pub(crate) fn build(
        section: TileSection,
        value: &str,
        presets: &[(WidgetPresetSpec, Vec<PresetFieldSpec>)],
        ctx: &BuildContext,
        on_change: &ChangeHook,
    ) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
        root.set_hexpand(true);
        let heading = gtk::Label::builder()
            .xalign(0.0)
            .label(section.heading())
            .css_classes(["heading"])
            .build();
        root.append(&heading);

        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::None);
        list.set_hexpand(true);
        let tiles: Rc<RefCell<Vec<WidgetTile>>> = Rc::new(RefCell::new(Vec::new()));
        let mut orig_ids: Vec<String> = Vec::new();

        if section.is_center() {
            for id in parse_string_list(value) {
                orig_ids.push(id.clone());
                let tile = make_widget_tile(
                    &id,
                    &id,
                    "",
                    None,
                    Rc::new(RefCell::new(Vec::new())),
                    Vec::new(),
                    section,
                    ctx,
                    &tiles,
                    &list,
                    on_change,
                );
                list.append(&tile.row);
                tiles.borrow_mut().push(tile);
            }
        } else {
            for scan_row in scan::scan_table(value, false) {
                orig_ids.push(scan_row.key.clone());
                let mut enabled_raw = "true".to_string();
                let mut order_raw = String::new();
                let mut scanned: Vec<(String, String)> = Vec::new();
                for (name, raw) in &scan_row.fields {
                    match name.as_str() {
                        "enabled" => enabled_raw = raw.trim().to_string(),
                        "_order" => order_raw = raw.trim().to_string(),
                        _ => scanned.push((name.clone(), raw.trim().to_string())),
                    }
                }
                if !truthy(&enabled_raw) {
                    continue;
                }
                let preset_entry = presets.iter().find(|(preset, _)| preset.id == scan_row.key);
                let preset = preset_entry.map(|(preset, _)| preset.clone());
                let specs: Vec<PresetFieldSpec> = preset_entry
                    .map(|(_, specs)| specs.clone())
                    .unwrap_or_default();
                let fields = Rc::new(RefCell::new(
                    scanned
                        .iter()
                        .map(|(name, raw)| {
                            let spec = specs.iter().find(|spec| spec.name == *name);
                            make_field_for_preset(name, raw, spec, ctx)
                        })
                        .collect(),
                ));
                let tile = make_widget_tile(
                    &scan_row.key,
                    &scan_row.key,
                    &order_raw,
                    preset,
                    fields,
                    specs,
                    section,
                    ctx,
                    &tiles,
                    &list,
                    on_change,
                );
                list.append(&tile.row);
                tiles.borrow_mut().push(tile);
            }
        }

        attach_tile_dnd(&list, &tiles, on_change);
        root.append(&list);

        let presets = presets.to_vec();
        let picker_popover = gtk::Popover::new();
        picker_popover.set_position(gtk::PositionType::Bottom);
        let picker_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
        picker_box.set_margin_top(12);
        picker_box.set_margin_bottom(12);
        picker_box.set_margin_start(12);
        picker_box.set_margin_end(12);
        picker_box.set_size_request(320, -1);
        let search = gtk::SearchEntry::new();
        search.set_placeholder_text(Some("Search widgets"));
        let picker_list = gtk::ListBox::new();
        picker_list.set_selection_mode(gtk::SelectionMode::None);
        let scroll = gtk::ScrolledWindow::builder()
            .min_content_height(160)
            .max_content_height(280)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&picker_list)
            .build();
        picker_box.append(&search);
        picker_box.append(&scroll);
        picker_popover.set_child(Some(&picker_box));

        let repopulate: Rc<dyn Fn()> = {
            let (picker_list, presets, tiles) =
                (picker_list.clone(), presets.clone(), tiles.clone());
            let search = search.clone();
            Rc::new(move || {
                populate_widget_picker(&picker_list, section, &presets, &tiles, &search.text());
            })
        };
        picker_popover.connect_map({
            let repopulate = repopulate.clone();
            move |_| repopulate()
        });
        search.connect_search_changed({
            let repopulate = repopulate.clone();
            move |_| repopulate()
        });
        picker_list.connect_row_activated({
            let (presets, tiles) = (presets.clone(), tiles.clone());
            let (target, on_change) = (list.clone(), on_change.clone());
            let ctx = ctx.clone();
            let popover = picker_popover.clone();
            move |_, row| {
                let preset_id = row.widget_name().to_string();
                if let Some((preset, specs)) =
                    presets.iter().find(|(preset, _)| preset.id == preset_id)
                {
                    add_tile_from_preset(preset, specs, section, &ctx, &tiles, &target, &on_change);
                }
                popover.popdown();
            }
        });

        let add_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let add = gtk::MenuButton::builder()
            .label("Add widget")
            .halign(gtk::Align::Start)
            .popover(&picker_popover)
            .build();
        let add_custom = gtk::Button::with_label("Add custom…");
        add_custom.add_css_class("flat");
        add_custom.set_halign(gtk::Align::Start);
        add_row.append(&add);
        add_row.append(&add_custom);
        root.append(&add_row);

        let custom_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let custom_entry = gtk::Entry::new();
        custom_entry.set_placeholder_text(Some("widgetId"));
        custom_entry.set_width_chars(16);
        let custom_ok = gtk::Button::with_label("Add");
        custom_row.append(&custom_entry);
        custom_row.append(&custom_ok);
        custom_row.set_visible(false);
        root.append(&custom_row);

        add_custom.connect_clicked({
            let custom_row = custom_row.clone();
            let custom_entry = custom_entry.clone();
            move |_| {
                let show = !custom_row.is_visible();
                custom_row.set_visible(show);
                if show {
                    custom_entry.grab_focus();
                }
            }
        });
        custom_ok.connect_clicked({
            let (custom_entry, custom_row) = (custom_entry.clone(), custom_row.clone());
            let (tiles, on_change) = (tiles.clone(), on_change.clone());
            let (target, ctx) = (list.clone(), ctx.clone());
            move |_| {
                let id = custom_entry.text().trim().to_string();
                if !valid_widget_id(&id) || tiles.borrow().iter().any(|tile| tile.id == id) {
                    return;
                }
                let tile = make_widget_tile(
                    &id,
                    "",
                    "",
                    None,
                    Rc::new(RefCell::new(Vec::new())),
                    Vec::new(),
                    section,
                    &ctx,
                    &tiles,
                    &target,
                    &on_change,
                );
                custom_entry.set_text("");
                custom_row.set_visible(false);
                target.append(&tile.row);
                tiles.borrow_mut().push(tile);
                if let Some(cb) = on_change.borrow().as_ref() {
                    cb();
                }
            }
        });

        Self {
            root,
            tiles,
            orig_ids,
        }
    }

    pub(crate) fn root(&self) -> &gtk::Box {
        &self.root
    }
}
