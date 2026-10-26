use regex::Regex;

pub struct LaunchArgs {
    pub page: Option<Regex>,
}

impl LaunchArgs {
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Self, String> {
        let mut page = None;
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            if arg == "--page" {
                let value = args.next().ok_or("--page requires a pattern")?;
                page = Some(value);
            } else if let Some(value) = arg.strip_prefix("--page=") {
                page = Some(value.to_string());
            } else {
                return Err(format!("unknown argument: {arg}"));
            }
        }
        let page = match page {
            Some(pattern) => Some(
                Regex::new(&pattern).map_err(|error| format!("invalid --page regex: {error}"))?,
            ),
            None => None,
        };
        Ok(Self { page })
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
    fn rejects_missing_page_value() {
        assert!(parse(&["--page"]).is_err());
    }

    #[test]
    fn rejects_invalid_regex() {
        assert!(parse(&["--page", "["]).is_err());
    }
}
