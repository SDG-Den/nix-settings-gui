use super::grid::{
    grid_index_of, grid_reattach, grid_repack, grid_restore_pinned, grid_select,
    grid_update_size_caption, GridLayoutState, GridTile,
};
use super::grid_layout::GRID_GAP;
use super::names::widget_display_name;
use gtk::prelude::*;
use std::rc::Rc;
/// Attach the click, move and resize gestures to one tile. Indices are resolved
/// from the button itself so a delete shifts them safely.
pub(crate) fn wire_grid_tile(state: &Rc<GridLayoutState>, tile: &GridTile) {
    {
        let st = state.clone();
        tile.tile.connect_toggled(move |button| {
            if st.programmatic.get() {
                return;
            }
            let Some(index) = grid_index_of(&st, button) else {
                return;
            };
            if button.is_active() {
                grid_select(&st, index);
            } else if *st.selected.borrow() == Some(index) {
                st.programmatic.set(true);
                button.set_active(true);
                st.programmatic.set(false);
            }
        });
    }
    if let Some(switch) = &tile.enabled {
        let st = state.clone();
        let button = tile.tile.clone();
        let label = tile.label.clone();
        let id = tile.id.clone();
        switch.connect_active_notify(move |switch| {
            let active = switch.is_active();
            let name = widget_display_name(&id);
            if active {
                label.set_text(&name);
                button.set_opacity(1.0);
            } else {
                label.set_text(&format!("{name} (off)"));
                button.set_opacity(0.4);
            }
            st.notify();
        });
    }
    wire_grid_move(state, tile);
    wire_grid_resize(state, tile, &tile.right_strip, true, false);
    wire_grid_resize(state, tile, &tile.bottom_strip, false, true);
    wire_grid_resize(state, tile, &tile.corner_strip, true, true);
}

pub(crate) fn wire_grid_move(state: &Rc<GridLayoutState>, tile: &GridTile) {
    let drag = gtk::GestureDrag::new();
    tile.tile.add_controller(drag.clone());
    let st = state.clone();
    let button = tile.tile.clone();
    drag.connect_drag_begin(move |_, _, _| {
        let Some(index) = grid_index_of(&st, &button) else {
            return;
        };
        *st.drag_index.borrow_mut() = Some(index);
        *st.size_index.borrow_mut() = None;
        st.cancelled.set(false);
        let width = button.width().max(1) as f64;
        let height = button.height().max(1) as f64;
        {
            let tiles = st.tiles.borrow();
            *st.pinned.borrow_mut() = tiles.iter().map(|tile| tile.cell).collect();
            if let Some(tile) = tiles.get(index) {
                let w = tile.cell.w.max(1) as f64;
                let h = tile.cell.h.max(1) as f64;
                st.steps.set((
                    (width + GRID_GAP as f64) / w,
                    (height + GRID_GAP as f64) / h,
                ));
            }
        }
        grid_select(&st, index);
    });
    let st = state.clone();
    drag.connect_drag_update(move |_, dx, dy| {
        if st.cancelled.get() {
            return;
        }
        let Some(index) = *st.drag_index.borrow() else {
            return;
        };
        let Some(base) = st.pinned.borrow().get(index).copied() else {
            return;
        };
        let (col_step, row_step) = st.steps.get();
        let columns = st.profile.columns;
        let col = (base.col as f64 + (dx / col_step).round())
            .clamp(0.0, (columns - base.w).max(0) as f64) as i64;
        let row = (base.row as f64 + (dy / row_step).round()).max(0.0) as i64;
        {
            let mut tiles = st.tiles.borrow_mut();
            if let Some(tile) = tiles.get_mut(index) {
                tile.cell.col = col;
                tile.cell.row = row;
            }
        }
        grid_repack(&st, Some(index), false);
        grid_reattach(&st);
        grid_update_size_caption(&st);
    });
    let st = state.clone();
    drag.connect_drag_end(move |_, _, _| {
        if st.cancelled.get() {
            grid_restore_pinned(&st);
        } else {
            grid_repack(&st, None, true);
        }
        grid_reattach(&st);
        grid_update_size_caption(&st);
        *st.drag_index.borrow_mut() = None;
        st.notify();
    });
}

pub(crate) fn wire_grid_resize(
    state: &Rc<GridLayoutState>,
    tile: &GridTile,
    strip: &gtk::Box,
    horizontal: bool,
    vertical: bool,
) {
    let drag = gtk::GestureDrag::new();
    strip.add_controller(drag.clone());
    let st = state.clone();
    let button = tile.tile.clone();
    drag.connect_drag_begin(move |_, _, _| {
        let Some(index) = grid_index_of(&st, &button) else {
            return;
        };
        if *st.selected.borrow() != Some(index) {
            return;
        }
        *st.size_index.borrow_mut() = Some(index);
        *st.drag_index.borrow_mut() = None;
        st.cancelled.set(false);
        let width = button.width().max(1) as f64;
        let height = button.height().max(1) as f64;
        {
            let tiles = st.tiles.borrow();
            *st.pinned.borrow_mut() = tiles.iter().map(|tile| tile.cell).collect();
            if let Some(tile) = tiles.get(index) {
                let w = tile.cell.w.max(1) as f64;
                let h = tile.cell.h.max(1) as f64;
                st.steps.set((
                    (width + GRID_GAP as f64) / w,
                    (height + GRID_GAP as f64) / h,
                ));
            }
        }
    });
    let st = state.clone();
    drag.connect_drag_update(move |_, dx, dy| {
        if st.cancelled.get() {
            return;
        }
        let Some(index) = *st.size_index.borrow() else {
            return;
        };
        let Some(base) = st.pinned.borrow().get(index).copied() else {
            return;
        };
        let (col_step, row_step) = st.steps.get();
        let columns = st.profile.columns;
        let last = match st.tiles.borrow().get(index) {
            Some(tile) => tile.cell,
            None => return,
        };
        let mut w = base.w;
        let mut h = base.h;
        if horizontal {
            w = (base.w as f64 + (dx / col_step).round())
                .clamp(1.0, (columns - base.col).max(1) as f64) as i64;
        }
        if vertical {
            h = (base.h as f64 + (dy / row_step).round()).max(1.0) as i64;
        }
        {
            let mut tiles = st.tiles.borrow_mut();
            if let Some(tile) = tiles.get_mut(index) {
                tile.cell.w = w;
                tile.cell.h = h;
            }
        }
        if !grid_repack(&st, Some(index), false) {
            {
                let mut tiles = st.tiles.borrow_mut();
                if let Some(tile) = tiles.get_mut(index) {
                    tile.cell = last;
                }
            }
            grid_repack(&st, Some(index), false);
        }
        grid_reattach(&st);
        grid_update_size_caption(&st);
    });
    let st = state.clone();
    drag.connect_drag_end(move |_, _, _| {
        *st.size_index.borrow_mut() = None;
        if st.cancelled.get() {
            grid_restore_pinned(&st);
        }
        grid_repack(&st, None, false);
        grid_reattach(&st);
        grid_update_size_caption(&st);
        st.notify();
    });
}
