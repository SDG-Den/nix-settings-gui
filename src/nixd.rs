use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::Duration;

use serde_json::{json, Value};

use crate::value;

fn nix_string(s: &str) -> String {
    let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

/// Build the nixd `options.<name>.expr` for a target.
pub fn options_expr(flake_path: &str, target: &str) -> Option<String> {
    let f = nix_string(flake_path);
    let parts: Vec<&str> = target.split(':').collect();
    match parts.first().copied()? {
        "fp" => Some(format!("(builtins.getFlake {f}).debug.options")),
        "nixos" => {
            let host = nix_string(parts.get(1)?);
            Some(format!(
                "(builtins.getFlake {f}).nixosConfigurations.{host}.options"
            ))
        }
        "home" => {
            let user = nix_string(parts.get(1)?);
            Some(format!(
                "(builtins.getFlake {f}).homeConfigurations.{user}.options"
            ))
        }
        "hm" => {
            let host = nix_string(parts.get(1)?);
            Some(format!(
                "(builtins.getFlake {f}).nixosConfigurations.{host}.options.home-manager.users.type.getSubOptions []"
            ))
        }
        "darwin" => {
            let host = nix_string(parts.get(1)?);
            Some(format!(
                "(builtins.getFlake {f}).darwinConfigurations.{host}.options"
            ))
        }
        _ => None,
    }
}

/// Type and description nixd reports for one option.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OptionInfo {
    pub type_str: Option<String>,
    pub description: String,
}

/// Split nixd's hover markdown into a type line and the remaining description.
pub fn split_hover(hover: &str) -> OptionInfo {
    let type_str = value::extract_type(hover);
    let description = hover
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .skip_while(|line| line.to_lowercase().starts_with("type"))
        .skip_while(|line| Some(line.to_string()) == type_str)
        .collect::<Vec<_>>()
        .join("\n");
    OptionInfo {
        type_str,
        description,
    }
}

fn write_message(stdin: &mut ChildStdin, value: &Value) -> std::io::Result<()> {
    let body = serde_json::to_vec(value)?;
    write!(stdin, "Content-Length: {}\r\n\r\n", body.len())?;
    stdin.write_all(&body)?;
    stdin.flush()
}

fn read_message(reader: &mut BufReader<ChildStdout>) -> std::io::Result<Value> {
    let mut length: Option<usize> = None;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "nixd closed the connection",
            ));
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some(rest) = trimmed.strip_prefix("Content-Length:") {
            length = rest.trim().parse().ok();
        }
    }
    let length = length.ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "missing Content-Length")
    })?;
    let mut buf = vec![0u8; length];
    reader.read_exact(&mut buf)?;
    serde_json::from_slice(&buf)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

fn spawn_watchdog(mut child: Child, timeout: Duration) {
    std::thread::spawn(move || {
        std::thread::sleep(timeout);
        let _ = child.kill();
    });
}

fn handshake(
    stdin: &mut ChildStdin,
    reader: &mut BufReader<ChildStdout>,
    config: &Value,
) -> Result<(), String> {
    let init = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "processId": std::process::id(),
            "rootUri": Value::Null,
            "capabilities": {},
            "initializationOptions": Value::Null,
        }
    });
    write_message(stdin, &init).map_err(|e| e.to_string())?;

    loop {
        let msg = read_message(reader).map_err(|e| e.to_string())?;
        let method = msg.get("method").and_then(Value::as_str);
        let id = msg.get("id").cloned();

        if let Some(method) = method {
            if let Some(id) = id {
                let result = if method == "workspace/configuration" {
                    let count = msg
                        .pointer("/params/items")
                        .and_then(Value::as_array)
                        .map(|a| a.len())
                        .unwrap_or(0);
                    Value::Array(vec![config.clone(); count])
                } else {
                    Value::Null
                };
                let reply = json!({"jsonrpc": "2.0", "id": id, "result": result});
                write_message(stdin, &reply).map_err(|e| e.to_string())?;
            }
            continue;
        }

        if id.and_then(|v| v.as_i64()) == Some(1) {
            let done = json!({"jsonrpc": "2.0", "method": "initialized", "params": {}});
            write_message(stdin, &done).map_err(|e| e.to_string())?;
            return Ok(());
        }
    }
}

/// Ask nixd for hover information about an option path in the given flake.
pub fn query_hover(flake_path: &str, target: &str, option_path: &str) -> Result<String, String> {
    let expr = options_expr(flake_path, target)
        .ok_or_else(|| format!("no nixd option set for target {target}"))?;
    let config = json!({
        "nixpkgs": { "expr": "import <nixpkgs> { }" },
        "options": { "main": { "expr": expr } },
    });

    let mut child = Command::new("nixd")
        .arg("--config")
        .arg(config.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("failed to start nixd: {e}"))?;

    let mut stdin = child.stdin.take().ok_or("nixd has no stdin")?;
    let mut reader = BufReader::new(child.stdout.take().ok_or("nixd has no stdout")?);
    spawn_watchdog(child, Duration::from_secs(30));

    handshake(&mut stdin, &mut reader, &config)?;

    let uri = "file:///tmp/nix-settings-gui/synthetic.nix";
    let text = format!("{{ ... }}: {{\n  {option_path} = null;\n}}\n");
    // nixd resolves only the single attrpath segment under the cursor; hover the
    // leaf (e.g. `systemPackages`) so it reports the option's real type, not the
    // first segment (`environment`), which has no type.
    let last_len = option_path.rsplit('.').next().map(str::len).unwrap_or(0);
    let leaf_column = 2 + option_path.len() - last_len;
    let did_open = json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didOpen",
        "params": { "textDocument": { "uri": uri, "languageId": "nix", "version": 1, "text": text } }
    });
    write_message(&mut stdin, &did_open).map_err(|e| e.to_string())?;

    let hover = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "textDocument/hover",
        "params": {
            "textDocument": { "uri": uri },
            "position": { "line": 1, "character": leaf_column }
        }
    });
    write_message(&mut stdin, &hover).map_err(|e| e.to_string())?;

    let result = loop {
        let msg = read_message(&mut reader).map_err(|e| e.to_string())?;
        let method = msg.get("method").and_then(Value::as_str);
        let id = msg.get("id").cloned();

        if method.is_some() {
            if let Some(id) = id {
                let reply = json!({"jsonrpc": "2.0", "id": id, "result": Value::Null});
                let _ = write_message(&mut stdin, &reply);
            }
            continue;
        }

        if id.and_then(|v| v.as_i64()) == Some(2) {
            break msg.get("result").cloned().unwrap_or(Value::Null);
        }
    };

    Ok(render_hover(&result))
}

/// The nixpkgs expression nixd should evaluate. Mirrors the VSCodium nix-ide
/// setup: the edited flake's own `nixpkgs` input, falling back to `<nixpkgs>`
/// when the opened path is not a flake.
pub fn nixpkgs_expr(flake_path: Option<&str>) -> String {
    match flake_path {
        Some(path) => format!(
            "(builtins.getFlake {}).inputs.nixpkgs {{ }}",
            nix_string(path)
        ),
        None => "import <nixpkgs> { }".to_string(),
    }
}

/// A running nixd process spoken to over stdio.
pub struct Session {
    stdin: ChildStdin,
    reader: BufReader<ChildStdout>,
    nixpkgs: String,
    next_id: i64,
}

impl Session {
    fn start(config: &Value) -> Result<Self, String> {
        let nixpkgs = config
            .pointer("/nixpkgs/expr")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let mut child = Command::new("nixd")
            .arg("--config")
            .arg(config.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("failed to start nixd: {e}"))?;
        let mut stdin = child.stdin.take().ok_or("nixd has no stdin")?;
        let mut reader = BufReader::new(child.stdout.take().ok_or("nixd has no stdout")?);
        spawn_watchdog(child, Duration::from_secs(300));
        handshake(&mut stdin, &mut reader, config)?;
        Ok(Self {
            stdin,
            reader,
            nixpkgs,
            next_id: 2,
        })
    }

    /// Start a session whose nixpkgs is the flake's own input, for completing
    /// package attribute names.
    pub fn start_packages(flake_path: Option<&str>) -> Result<Self, String> {
        let config = json!({ "nixpkgs": { "expr": nixpkgs_expr(flake_path) }, "options": {} });
        Self::start(&config)
    }

    /// Complete package attribute names under `pkgs.<prefix>`. Successive calls
    /// reuse the same nixd process, which is what makes keystroke-by-keystroke
    /// search practical.
    pub fn complete_packages(&mut self, prefix: &str, seq: u64) -> Result<Vec<String>, String> {
        let uri = format!("file:///tmp/nix-settings-gui/packages-{seq}.nix");
        let text = format!("let pkgs = {}; in\n[ pkgs.{prefix} ]\n", self.nixpkgs);
        self.notify(&did_open(&uri, &text))?;

        let id = self.next_id;
        self.next_id += 1;
        let column = 7 + prefix.len();
        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "textDocument/completion",
            "params": {
                "textDocument": { "uri": uri },
                "position": { "line": 1, "character": column },
                "context": { "triggerKind": 1 },
            }
        });
        let result = self.request(id, &request)?;
        Ok(completion_labels(&result))
    }

    fn notify(&mut self, value: &Value) -> Result<(), String> {
        write_message(&mut self.stdin, value).map_err(|e| e.to_string())
    }

    fn request(&mut self, want: i64, value: &Value) -> Result<Value, String> {
        write_message(&mut self.stdin, value).map_err(|e| e.to_string())?;
        loop {
            let msg = read_message(&mut self.reader).map_err(|e| e.to_string())?;
            if msg.get("method").and_then(Value::as_str).is_some() {
                if let Some(id) = msg.get("id").cloned() {
                    let reply = json!({"jsonrpc": "2.0", "id": id, "result": Value::Null});
                    let _ = write_message(&mut self.stdin, &reply);
                }
                continue;
            }
            if msg.get("id").and_then(Value::as_i64) == Some(want) {
                return Ok(msg.get("result").cloned().unwrap_or(Value::Null));
            }
        }
    }

    /// Block until nixd publishes diagnostics for `uri`, then return their
    /// messages. An empty list means the document is clean.
    fn diagnostics(&mut self, uri: &str) -> Result<Vec<String>, String> {
        loop {
            let msg = read_message(&mut self.reader).map_err(|e| e.to_string())?;
            let method = msg.get("method").and_then(Value::as_str);
            if method == Some("textDocument/publishDiagnostics")
                && msg
                    .pointer("/params/textDocument/uri")
                    .and_then(Value::as_str)
                    == Some(uri)
            {
                return Ok(collect_diagnostics(&msg));
            }
            if method.is_some() {
                if let Some(id) = msg.get("id").cloned() {
                    let reply = json!({"jsonrpc": "2.0", "id": id, "result": Value::Null});
                    let _ = write_message(&mut self.stdin, &reply);
                }
            }
        }
    }
}

fn did_open(uri: &str, text: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didOpen",
        "params": {
            "textDocument": {
                "uri": uri,
                "languageId": "nix",
                "version": 1,
                "text": text,
            }
        }
    })
}

/// The diagnostic messages nixd reports for one assignment inside a module.
pub fn query_errors(flake_path: Option<&str>, assignment: &str) -> Result<Vec<String>, String> {
    let nixpkgs = nixpkgs_expr(flake_path);
    let config = json!({ "nixpkgs": { "expr": nixpkgs }, "options": {} });
    let mut session = Session::start(&config)?;

    let uri = "file:///tmp/nix-settings-gui/validate.nix";
    let text = format!("let pkgs = {nixpkgs}; in\n{{\n  {assignment}\n}}\n");
    session.notify(&did_open(uri, &text))?;
    session.diagnostics(uri)
}

fn completion_labels(result: &Value) -> Vec<String> {
    let array = match result {
        Value::Object(o) => o.get("items").and_then(Value::as_array),
        Value::Array(a) => Some(a),
        _ => None,
    };
    let Some(array) = array else {
        return Vec::new();
    };
    let mut names: Vec<String> = array
        .iter()
        .filter_map(|item| item.get("label").and_then(Value::as_str))
        .filter(|label| {
            label
                .chars()
                .next()
                .map(|c| c.is_ascii_alphabetic() || c == '_')
                .unwrap_or(false)
                && !label.contains('.')
        })
        .map(str::to_string)
        .collect();
    names.sort();
    names.dedup();
    names
}

fn collect_diagnostics(msg: &Value) -> Vec<String> {
    msg.pointer("/params/diagnostics")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|d| d.get("message").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Query one option's type and description.
pub fn query_option(
    flake_path: &str,
    target: &str,
    option_path: &str,
) -> Result<OptionInfo, String> {
    let hover = query_hover(flake_path, target, option_path)?;
    Ok(split_hover(&hover))
}

fn render_hover(result: &Value) -> String {
    if result.is_null() {
        return String::new();
    }
    match result.get("contents") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Object(o)) => o
            .get("value")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|i| match i {
                Value::String(s) => Some(s.clone()),
                Value::Object(o) => o.get("value").and_then(Value::as_str).map(str::to_string),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_option_exprs() {
        assert_eq!(
            options_expr("/flake", "fp").unwrap(),
            "(builtins.getFlake \"/flake\").debug.options"
        );
        assert_eq!(
            options_expr("/flake", "nixos:x570").unwrap(),
            "(builtins.getFlake \"/flake\").nixosConfigurations.\"x570\".options"
        );
        assert!(options_expr("/flake", "bogus").is_none());
    }

    #[test]
    fn builds_nixpkgs_exprs() {
        assert_eq!(nixpkgs_expr(None), "import <nixpkgs> { }");
        assert_eq!(
            nixpkgs_expr(Some("/home/u/flake")),
            "(builtins.getFlake \"/home/u/flake\").inputs.nixpkgs { }"
        );
    }

    #[test]
    fn renders_completion_labels() {
        let result = json!({ "items": [
            { "label": "git" },
            { "label": "git-lfs" },
            { "label": "git" },
            { "label": "lib.attrs" },
            { "label": "3d" }
        ] });
        assert_eq!(completion_labels(&result), vec!["git", "git-lfs"]);
        assert!(completion_labels(&Value::Null).is_empty());
    }

    #[test]
    fn reads_diagnostic_messages() {
        let msg = json!({ "params": { "diagnostics": [
            { "message": "undefined variable 'git'" },
            { "message": "second" }
        ] } });
        assert_eq!(
            collect_diagnostics(&msg),
            vec!["undefined variable 'git'", "second"]
        );
    }

    #[test]
    fn splits_type_and_description() {
        let hover = "boolean\n\nWhether to enable the service.";
        let info = split_hover(hover);
        assert_eq!(info.type_str.as_deref(), Some("boolean"));
        assert_eq!(info.description, "Whether to enable the service.");
    }
}
