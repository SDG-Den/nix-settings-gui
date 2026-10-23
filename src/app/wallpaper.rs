use std::path::Path;
use std::rc::Rc;

use super::{files, state::App};
use crate::widgets::WallpaperActions;

/// Build the copy and remove operations the wallpaper picker calls.
pub(super) fn actions(app: &Rc<App>) -> WallpaperActions {
    let add_app = app.clone();
    let remove_app = app.clone();
    WallpaperActions {
        add: Rc::new(move |source: &Path, dir: &Path| {
            let base = add_app.base.borrow().clone();
            let result = files::add_file(&base, source, dir, "wallpaper");
            match &result {
                Ok(path) => add_app.status.set_text(&format!(
                    "Added wallpaper {}",
                    path.file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default()
                )),
                Err(error) => add_app
                    .status
                    .set_text(&format!("Wallpaper add failed: {error}")),
            }
            result
        }),
        remove: Rc::new(move |path: &Path| {
            let base = remove_app.base.borrow().clone();
            let result = files::remove_file(&base, path);
            match &result {
                Ok(()) => remove_app.status.set_text("Removed wallpaper"),
                Err(error) => remove_app
                    .status
                    .set_text(&format!("Wallpaper remove failed: {error}")),
            }
            result
        }),
    }
}
