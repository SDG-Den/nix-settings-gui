use std::path::{Path, PathBuf};

use super::git;

/// Copy `source` into `dir` under a sanitized name and stage it with git.
pub(super) fn add_file(
    base: &str,
    source: &Path,
    dir: &Path,
    noun: &str,
) -> Result<PathBuf, String> {
    let name = source
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .ok_or_else(|| format!("{} has no file name", source.display()))?;
    let name = safe_name(&name, noun);
    let destination = dir.join(&name);
    if destination.exists() {
        return Err(format!("a {noun} named {name} already exists"));
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

/// Remove a file from the repository and stage the deletion.
pub(super) fn remove_file(base: &str, path: &Path) -> Result<(), String> {
    let relative = git::relative_in_repo(base, path)?;
    std::fs::remove_file(path)
        .map_err(|error| format!("cannot remove {}: {error}", path.display()))?;
    git::stage_deletion(base, &relative)
}

/// Keep only characters that are safe in a Nix path literal.
pub(super) fn safe_name(name: &str, fallback: &str) -> String {
    let mut out = String::new();
    for character in name.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
            out.push(character);
        } else {
            out.push('-');
        }
    }
    if out.is_empty() {
        fallback.to_string()
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
            safe_name("ever forest (dark).png", "wallpaper"),
            "ever-forest--dark-.png"
        );
        assert_eq!(safe_name("paper.png", "wallpaper"), "paper.png");
        assert_eq!(safe_name("", "wallpaper"), "wallpaper");
        assert_eq!(safe_name("", "icon"), "icon");
    }

    #[test]
    fn refuses_to_overwrite_an_existing_file() {
        let root =
            std::env::temp_dir().join(format!("nix-settings-gui-file-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("source.png");
        std::fs::write(&source, b"one").unwrap();
        let dir = root.join("icons");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("source.png"), b"two").unwrap();
        let error = add_file(root.to_str().unwrap(), &source, &dir, "icon").unwrap_err();
        assert!(error.contains("already exists"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
