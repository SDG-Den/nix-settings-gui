use regex::Regex;

pub const HELP: &str = "\
Usage: nix-settings-gui [OPTIONS]

Options:
  --page <regex>        Open the menu whose name or file matches <regex>
  --descriptors <file>  Load descriptor sidecar definitions from <file>
  -h, --help            Print this help and exit";

pub struct LaunchArgs {
    pub page: Option<Regex>,
    /// Path to a descriptor sidecar, from `--descriptors <file>`.
    pub descriptors: Option<String>,
    /// Whether `--help` or `-h` was passed.
    pub help: bool,
}

impl LaunchArgs {
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Self, String> {
        let mut page = None;
        let mut descriptors = None;
        let mut help = false;
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            if arg == "--help" || arg == "-h" {
                help = true;
            } else if arg == "--page" {
                let value = args.next().ok_or("--page requires a pattern")?;
                page = Some(value);
            } else if let Some(value) = arg.strip_prefix("--page=") {
                page = Some(value.to_string());
            } else if arg == "--descriptors" {
                let value = args.next().ok_or("--descriptors requires a file")?;
                descriptors = Some(value);
            } else if let Some(value) = arg.strip_prefix("--descriptors=") {
                descriptors = Some(value.to_string());
            } else {
                return Err(format!("unknown argument: {arg}"));
            }
        }
        if help {
            return Ok(Self {
                page: None,
                descriptors: None,
                help: true,
            });
        }
        let page = match page {
            Some(pattern) => Some(
                Regex::new(&pattern).map_err(|error| format!("invalid --page regex: {error}"))?,
            ),
            None => None,
        };
        Ok(Self {
            page,
            descriptors,
            help,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<LaunchArgs, String> {
        LaunchArgs::parse(args.iter().map(|arg| arg.to_string()))
    }

    #[test]
    fn parses_page_in_both_forms() {
        assert!(parse(&["--page", "System"]).unwrap().page.is_some());
        assert!(parse(&["--page=System"]).unwrap().page.is_some());
    }

    #[test]
    fn parses_descriptors_in_both_forms() {
        assert_eq!(
            parse(&["--descriptors", "/tmp/d.json"])
                .unwrap()
                .descriptors
                .as_deref(),
            Some("/tmp/d.json")
        );
        assert_eq!(
            parse(&["--descriptors=/tmp/d.json"])
                .unwrap()
                .descriptors
                .as_deref(),
            Some("/tmp/d.json")
        );
    }

    #[test]
    fn rejects_missing_page_value() {
        assert!(parse(&["--page"]).is_err());
        assert!(parse(&["--descriptors"]).is_err());
    }

    #[test]
    fn rejects_invalid_regex() {
        assert!(parse(&["--page", "["]).is_err());
    }

    #[test]
    fn parses_help_in_both_forms() {
        assert!(parse(&["--help"]).unwrap().help);
        assert!(parse(&["-h"]).unwrap().help);
    }

    #[test]
    fn help_wins_over_other_arguments() {
        let args = parse(&["--help", "--page", "["]).unwrap();
        assert!(args.help);
        assert!(args.page.is_none());
    }
}
