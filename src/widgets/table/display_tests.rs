use super::*;

fn grid_cells(grid: &gtk::Grid) -> Vec<(i32, i32)> {
    let Some(manager) = grid.layout_manager() else {
        return Vec::new();
    };
    let children = grid.observe_children();
    let mut out = Vec::new();
    for index in 0..children.n_items() {
        let Some(object) = children.item(index) else {
            continue;
        };
        let Ok(child) = object.downcast::<gtk::Widget>() else {
            continue;
        };
        let Ok(cell) = manager
            .layout_child(&child)
            .downcast::<gtk::GridLayoutChild>()
        else {
            continue;
        };
        out.push((cell.column(), cell.row()));
    }
    out
}

fn grid_child(grid: &gtk::Grid, column: i32, row: i32) -> Option<gtk::Widget> {
    let manager = grid.layout_manager()?;
    let children = grid.observe_children();
    for index in 0..children.n_items() {
        let child = children.item(index)?.downcast::<gtk::Widget>().ok()?;
        let cell = manager
            .layout_child(&child)
            .downcast::<gtk::GridLayoutChild>()
            .ok()?;
        if cell.column() == column && cell.row() == row {
            return Some(child);
        }
    }
    None
}

fn table_grid(record: &Record) -> (Editor, gtk::Grid) {
    let ctx = BuildContext::default();
    let editor = build_table(record, &ctx, false);
    let root = editor.root().downcast::<gtk::Box>().unwrap();
    let grid = root.first_child().unwrap().downcast::<gtk::Grid>().unwrap();
    (editor, grid)
}

fn table_record(value: &str, columns: &[&str], types: &[&str]) -> Record {
    Record {
        value: value.to_string(),
        spec: OptionSpec {
            widget: Some(WidgetKind::TableAttrset),
            columns: Some(columns.iter().map(|name| name.to_string()).collect()),
            column_types: Some(types.iter().map(|name| name.to_string()).collect()),
            ..OptionSpec::default()
        },
        ..Record::default()
    }
}

#[test]
#[ignore = "needs a display; run with `cargo test -- --ignored`"]
fn table_grid_aligns_columns_and_reindexes_on_remove() {
    if gtk::init().is_err() {
        eprintln!("skipping table grid runtime check: no display");
        return;
    }
    let record = table_record(
            "{ a = { enable = true; value = \"x\"; }; b = { enable = false; value = \"a much longer value\"; }; }",
            &["enable", "value"],
            &["bool", "str"],
        );
    let (editor, grid) = table_grid(&record);
    let root = editor.root().downcast::<gtk::Box>().unwrap();
    assert_eq!(root.halign(), gtk::Align::Fill);
    assert_eq!(grid.halign(), gtk::Align::Fill);

    // Header captions sit on the same grid columns as every row's cells.
    let cells = grid_cells(&grid);
    assert!(
        cells.contains(&(1, 0)) && cells.contains(&(2, 0)),
        "{cells:?}"
    );
    assert!(
        !cells.contains(&(0, 0)),
        "no caption over the key column: {cells:?}"
    );
    for row in 1..=2 {
        for column in 0..4 {
            assert!(
                cells.contains(&(column, row)),
                "row {row} column {column}: {cells:?}"
            );
        }
    }
    let caption_enable = grid_child(&grid, 1, 0).unwrap();
    let caption_value = grid_child(&grid, 2, 0).unwrap();
    assert!(!caption_enable.hexpands());
    assert!(caption_value.hexpands());

    let enabled = grid_child(&grid, 1, 1)
        .unwrap()
        .downcast::<gtk::Switch>()
        .unwrap();
    assert!(enabled.is_active());
    assert!(!enabled.hexpands());
    assert_eq!(
        enabled.valign(),
        gtk::Align::Center,
        "switches keep their normal height in a row sized by the icon"
    );
    let disabled = grid_child(&grid, 1, 2)
        .unwrap()
        .downcast::<gtk::Switch>()
        .unwrap();
    assert!(!disabled.is_active());
    let value = grid_child(&grid, 2, 1)
        .unwrap()
        .downcast::<gtk::Entry>()
        .unwrap();
    assert!(value.hexpands(), "text cells absorb the extra width");

    // An added row opens with a switch and a text entry, not free text.
    let root = editor.root().downcast::<gtk::Box>().unwrap();
    root.last_child()
        .unwrap()
        .downcast::<gtk::Button>()
        .unwrap()
        .emit_clicked();
    assert!(grid_child(&grid, 0, 3).is_some(), "row 3 key slot");
    let added = grid_child(&grid, 1, 3)
        .unwrap()
        .downcast::<gtk::Switch>()
        .unwrap();
    assert!(!added.is_active());
    let added_value = grid_child(&grid, 2, 3)
        .unwrap()
        .downcast::<gtk::Entry>()
        .unwrap();
    assert_eq!(added_value.text(), "");

    grid_child(&grid, 0, 3)
        .unwrap()
        .downcast::<gtk::Entry>()
        .unwrap()
        .set_text("smoke");
    added.set_active(true);
    let on = editor.to_nix();
    assert!(
        on.contains("smoke = { enable = true; value = \"\"; };"),
        "{on}"
    );
    assert!(!on.contains("enable = \"true\""), "{on}");
    std::fs::write(std::env::temp_dir().join("i01-add-row.nix"), &on).unwrap();

    // Removing a row pulls the rows below it up, so slots stay dense.
    grid_child(&grid, 3, 1)
        .unwrap()
        .downcast::<gtk::Button>()
        .unwrap()
        .emit_clicked();
    let cells = grid_cells(&grid);
    assert!(!cells.contains(&(0, 3)), "rows shifted up: {cells:?}");
    assert!(
        cells.contains(&(0, 1)) && cells.contains(&(0, 2)),
        "{cells:?}"
    );
    let after = editor.to_nix();
    assert!(!after.contains("a = {"), "{after}");
    assert!(
        after.contains("b = {") && after.contains("smoke = {"),
        "{after}"
    );

    // Saved off, then re-scanned: the cell is still a switch.
    added.set_active(false);
    let off = editor.to_nix();
    assert!(off.contains("enable = false;"), "{off}");
    let reloaded = Record {
        value: off,
        spec: record.spec.clone(),
        ..Record::default()
    };
    let (_second, second_grid) = table_grid(&reloaded);
    let reopened = grid_child(&second_grid, 1, 2).unwrap();
    assert!(
        reopened.downcast::<gtk::Switch>().is_ok(),
        "a row saved with enable = false re-opens as a switch"
    );
    assert!(grid_child(&second_grid, 1, 1)
        .unwrap()
        .downcast::<gtk::Switch>()
        .is_ok());
}

#[test]
#[ignore = "needs a display; run with `cargo test -- --ignored`"]
fn table_grid_fills_the_available_width() {
    if gtk::init().is_err() {
        eprintln!("skipping table width check: no display");
        return;
    }
    let record = table_record(
        "{ youtube = { enable = true; name = \"YouTube\"; url = \"https://www.youtube.com\"; icon = null; }; }",
        &["enable", "name", "url", "icon"],
        &["bool", "str", "str", "icon"],
    );
    let (editor, grid) = table_grid(&record);
    let window = gtk::Window::new();
    window.set_default_size(900, 240);
    window.set_child(Some(&editor.root()));
    window.present();
    let context = gtk::glib::MainContext::default();
    while context.iteration(false) {}
    let grid_width = grid.width();
    let root_width = editor.root().width();
    assert!(
        grid_width >= root_width - 20,
        "grid {grid_width} should fill root {root_width}"
    );
    window.close();
}

#[test]
#[ignore = "needs a display; run with `cargo test -- --ignored`"]
fn table_grid_keeps_locked_labels_and_single_column_captions() {
    if gtk::init().is_err() {
        eprintln!("skipping table grid runtime check: no display");
        return;
    }
    let record = table_record(
        "{ /* @table-item label = \"PhotoCraft\"; */ photo = { enable = true; }; }",
        &["enable"],
        &["bool"],
    );
    let (_editor, grid) = table_grid(&record);
    let heading = grid_child(&grid, 0, 1)
        .unwrap()
        .downcast::<gtk::Label>()
        .unwrap();
    assert_eq!(heading.label(), "PhotoCraft");
    let caption = grid_child(&grid, 1, 0)
        .unwrap()
        .downcast::<gtk::Label>()
        .unwrap();
    assert_eq!(caption.label(), "enable");
    assert!(grid_child(&grid, 1, 1)
        .unwrap()
        .downcast::<gtk::Switch>()
        .is_ok());
    assert!(grid_child(&grid, 2, 1)
        .unwrap()
        .downcast::<gtk::Button>()
        .is_ok());
    assert!(
        !grid_cells(&grid).contains(&(0, 0)),
        "no caption over locked labels"
    );
}

#[test]
#[ignore = "needs a display; run with `cargo test -- --ignored`"]
fn table_grid_locked_row_without_label_shows_key_heading() {
    if gtk::init().is_err() {
        eprintln!("skipping table grid runtime check: no display");
        return;
    }
    let record = table_record("{ photo = { enable = true; }; }", &["enable"], &["bool"]);
    let (_editor, grid) = table_grid(&record);
    let heading = grid_child(&grid, 0, 1)
        .unwrap()
        .downcast::<gtk::Label>()
        .unwrap();
    assert_eq!(heading.label(), "photo");
    assert!(grid_child(&grid, 1, 1)
        .unwrap()
        .downcast::<gtk::Switch>()
        .is_ok());
}
