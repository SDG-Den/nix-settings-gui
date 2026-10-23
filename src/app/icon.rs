use std::path::Path;
use std::rc::Rc;

use super::{files, state::App};
use crate::widgets::IconActions;

/// Build the copy operation the webapps icon picker calls.
pub(super) fn actions(app: &Rc<App>) -> IconActions {
    let add_app = app.clone();
    IconActions {
        add: Rc::new(move |source: &Path, dir: &Path| {
            let base = add_app.base.borrow().clone();
            let result = files::add_file(&base, source, dir, "icon");
            match &result {
                Ok(path) => add_app.status.set_text(&format!(
                    "Added icon {}",
                    path.file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default()
                )),
                Err(error) => add_app
                    .status
                    .set_text(&format!("Icon add failed: {error}")),
            }
            result
        }),
    }
}
