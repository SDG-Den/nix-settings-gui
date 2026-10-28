use std::io::Write;
use std::process::{Command, Stdio};

/// Hash a plaintext password for `users.users.<name>.hashedPassword`.
///
/// Prefer `mkpasswd` with yescrypt, which matches the default NixOS hash
/// format. Fall back to `openssl passwd -6` when mkpasswd is unavailable. The
/// password always travels over stdin so it never appears in the process list.
pub fn hash_password(password: &str) -> Result<String, String> {
    let mut errors = Vec::new();
    match run_hasher("mkpasswd", &["-m", "yescrypt", "-s"], password) {
        Ok(hash) => return Ok(hash),
        Err(error) => errors.push(error),
    }
    match run_hasher("openssl", &["passwd", "-6", "-stdin"], password) {
        Ok(hash) => Ok(hash),
        Err(error) => {
            errors.push(error);
            Err(errors.join("; "))
        }
    }
}

fn run_hasher(program: &str, args: &[&str], password: &str) -> Result<String, String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("cannot run {program}: {error}"))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(password.as_bytes())
            .and_then(|_| stdin.write_all(b"\n"))
            .map_err(|error| format!("cannot write to {program}: {error}"))?;
    }

    let output = child
        .wait_with_output()
        .map_err(|error| format!("cannot wait for {program}: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            format!("{program} exited unsuccessfully")
        } else {
            format!("{program}: {stderr}")
        });
    }
    let hash = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !looks_like_hash(&hash) {
        return Err(format!("{program} did not return a password hash"));
    }
    Ok(hash)
}

fn looks_like_hash(text: &str) -> bool {
    text.starts_with('$') && text.len() > 3
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_a_missing_hasher() {
        assert!(run_hasher("nix-settings-gui-no-such-hasher", &[], "x").is_err());
    }

    #[test]
    fn recognises_hash_shaped_output() {
        assert!(looks_like_hash("$6$salt$hash"));
        assert!(looks_like_hash("$y$j9T$salt$hash"));
        assert!(!looks_like_hash("plain"));
        assert!(!looks_like_hash("$"));
    }
}
