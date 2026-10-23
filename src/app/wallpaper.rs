use std::path::{Path, PathBuf};
use std::rc::Rc;

use super::git;
use super::state::App;
use crate::widgets::WallpaperActions;

/// Build the copy and remove operations the wallpaper picker calls.
pub(super) fn actions(app: &Rc<App>) -> WallpaperActions {
    let add_app = app.clone();
    let remove_app = app.clone();
    WallpaperActions {
        add: Rc::new(move |source: &Path, dir: &Path| {
            let base = add_app.base.borrow().clone();
            let result = add_file(&base, source, dir);
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
            let result = remove_file(&base, path);
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

fn add_file(base: &str, source: &Path, dir: &Path) -> Result<PathBuf, String> {
    let name = source
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .ok_or_else(|| format!("{} has no file name", source.display()))?;
    let name = safe_name(&name);
    let destination = dir.join(&name);
    if destination.exists() {
        return Err(format!("a wallpaper named {name} already exists"));
    }
    std::fs::create_dir_all(dir)
        .map_err(|error| format!("cannot create {}: {error}", dir.display()))?;
    std::fs::copy(source, &destination).map_err(|error| {
        format!(
            "cannot copy {} to {}: {error}",
            source.display(),
            destination.display()
        )
    })?;
    if let Err(error) = git::stage_path(base, &destination) {
        let _ = std::fs::remove_file(&destination);
        return Err(error);
    }
    Ok(destination)
}

fn remove_file(base: &str, path: &Path) -> Result<(), String> {
    let relative = git::relative_in_repo(base, path)?;
    std::fs::remove_file(path)
        .map_err(|error| format!("cannot remove {}: {error}", path.display()))?;
    git::stage_deletion(base, &relative)
}

fn safe_name(name: &str) -> String {
    let mut out = String::new();
    for character in name.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
            out.push(character);
        } else {
            out.push('-');
        }
    }
    if out.is_empty() {
        "wallpaper".to_string()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_names_for_nix_path_literals() {
        assert_eq!(
            safe_name("ever forest (dark).png"),
            "ever-forest--dark-.png"
        );
        assert_eq!(safe_name("paper.png"), "paper.png");
        assert_eq!(safe_name(""), "wallpaper");
    }

    #[test]
    fn refuses_to_overwrite_an_existing_wallpaper() {
        let root =
            std::env::temp_dir().join(format!("nix-settings-gui-wallpaper-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("source.png");
        std::fs::write(&source, b"one").unwrap();
        let dir = root.join("wallpapers");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("source.png"), b"two").unwrap();
        let error = add_file(root.to_str().unwrap(), &source, &dir).unwrap_err();
        assert!(error.contains("already exists"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
