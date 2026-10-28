use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub(crate) struct ThemeRoot {
    pub name: String,
    pub label: String,
    pub path: PathBuf,
    pub parent: PathBuf,
    pub has_cursors: bool,
}

fn icon_dirs() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let home = env::var_os("HOME").map(PathBuf::from);
    let data_home = env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| home.as_ref().map(|home| home.join(".local/share")));
    if let Some(data_home) = data_home {
        roots.push(data_home.join("icons"));
    }
    let data_dirs =
        env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    for dir in data_dirs.split(':').filter(|dir| !dir.is_empty()) {
        roots.push(PathBuf::from(dir).join("icons"));
    }
    if let Some(home) = home {
        roots.push(home.join(".icons"));
    }
    roots
}

pub(crate) fn roots() -> Vec<ThemeRoot> {
    roots_in(&icon_dirs())
}

fn roots_in(dirs: &[PathBuf]) -> Vec<ThemeRoot> {
    let mut seen = HashSet::new();
    let mut themes = Vec::new();
    for parent in dirs {
        let Ok(entries) = fs::read_dir(parent) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if name == "default" {
                continue;
            }
            let index = path.join("index.theme");
            let cursors = path.join("cursors");
            let has_cursors = cursors.is_dir();
            if !index.is_file() && !has_cursors {
                continue;
            }
            let canonical = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
            if !seen.insert(canonical) {
                continue;
            }
            let label = fs::read_to_string(&index)
                .ok()
                .and_then(|text| index_name(&text))
                .unwrap_or_else(|| name.clone());
            themes.push(ThemeRoot {
                name,
                label,
                path,
                parent: parent.clone(),
                has_cursors,
            });
        }
    }
    themes.sort_by_key(|theme| theme.name.to_lowercase());
    themes
}

fn index_name(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let (key, value) = line.split_once('=')?;
        if key.trim() != "Name" {
            return None;
        }
        let value = value.trim().trim_matches('"').trim();
        (!value.is_empty()).then(|| value.to_string())
    })
}

pub(crate) fn find(name: &str) -> Option<ThemeRoot> {
    roots().into_iter().find(|theme| theme.name == name)
}

pub(crate) fn find_cursor(name: &str) -> Option<ThemeRoot> {
    roots()
        .into_iter()
        .find(|theme| theme.name == name && theme.has_cursors)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let root = env::temp_dir().join(format!("nsg-theme-catalog-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn reads_display_name_and_cursor_flag() {
        let root = temp_root("read");
        let theme = root.join("MyTheme");
        fs::create_dir_all(theme.join("cursors")).unwrap();
        fs::write(theme.join("index.theme"), "[Icon Theme]\nName=My Theme\n").unwrap();
        let themes = roots_in(std::slice::from_ref(&root));
        assert_eq!(themes.len(), 1);
        assert_eq!(themes[0].name, "MyTheme");
        assert_eq!(themes[0].label, "My Theme");
        assert!(themes[0].has_cursors);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn skips_the_default_alias_and_duplicate_paths() {
        let root = temp_root("dedupe");
        let theme = root.join("Theme");
        fs::create_dir_all(&theme).unwrap();
        fs::write(theme.join("index.theme"), "Name=Theme\n").unwrap();
        std::os::unix::fs::symlink(&theme, root.join("default")).unwrap();
        std::os::unix::fs::symlink(&theme, root.join("Alias")).unwrap();
        let themes = roots_in(std::slice::from_ref(&root));
        assert_eq!(themes.len(), 1);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn ignores_directories_without_theme_data() {
        let root = temp_root("empty");
        fs::create_dir_all(root.join("NotATheme")).unwrap();
        assert!(roots_in(std::slice::from_ref(&root)).is_empty());
        let _ = fs::remove_dir_all(root);
    }
}
