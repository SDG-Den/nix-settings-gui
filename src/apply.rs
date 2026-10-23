//! Pure helpers for the command that applies an option immediately.

/// The token in an apply command that stands for the option's value.
pub const VALUE_TOKEN: &str = "%%val%%";

/// One request to apply an option: the command from its `@option` marker, the
/// option's path, and its current value as a Nix literal.
#[derive(Debug, Clone)]
pub struct ApplyRequest {
    /// The option's path, shown in the status line.
    pub path: String,
    /// The command template from the `@option` marker.
    pub command: String,
    /// The option's current value, as a Nix literal.
    pub value: String,
}

/// Quote a value so the shell hands it to the command as a single argument.
pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Replace every `%%val%%` in the template with the quoted value. A template
/// without the token is left as it is.
pub fn substitute(template: &str, value: &str) -> String {
    if !template.contains(VALUE_TOKEN) {
        return template.to_string();
    }
    template.replace(VALUE_TOKEN, &shell_quote(value))
}

/// The plain text of a Nix literal: a quoted string loses its quotes, so
/// `"dark"` becomes `dark` and a number stays a number.
pub fn plain_value(nix: &str) -> String {
    crate::widgets::nix::parse_string_literal(nix)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_values_for_the_shell() {
        assert_eq!(shell_quote("dark"), "'dark'");
        assert_eq!(shell_quote(""), "''");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }

    #[test]
    fn substitutes_the_value_token() {
        assert_eq!(
            substitute("dms ipc call theme set %%val%%", "dark"),
            "dms ipc call theme set 'dark'"
        );
        assert_eq!(
            substitute("cmd %%val%% and %%val%%", "x"),
            "cmd 'x' and 'x'"
        );
    }

    #[test]
    fn leaves_a_template_without_the_token_alone() {
        assert_eq!(
            substitute("dms ipc call theme set", "dark"),
            "dms ipc call theme set"
        );
    }

    #[test]
    fn reads_the_plain_value_of_a_literal() {
        assert_eq!(plain_value("\"dark\""), "dark");
        assert_eq!(plain_value("42"), "42");
        assert_eq!(plain_value("true"), "true");
        assert_eq!(plain_value("[ \"a\" ]"), "[ \"a\" ]");
    }
}
