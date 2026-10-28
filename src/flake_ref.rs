use std::fmt::Write;

/// A flake reference in URL-like form. Only the attributes that are meaningful
/// for the selected type are rendered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlakeRef {
    Indirect {
        id: String,
        ref_: Option<String>,
        rev: Option<String>,
    },
    Path {
        path: String,
    },
    Git {
        transport: GitTransport,
        url: String,
        query: Query,
    },
    Mercurial {
        transport: MercurialTransport,
        url: String,
        query: Query,
    },
    Tarball {
        transport: FileTransport,
        url: String,
        nar_hash: Option<String>,
    },
    File {
        transport: FileTransport,
        url: String,
        nar_hash: Option<String>,
    },
    Github {
        owner: String,
        repo: String,
        selector: Option<String>,
        query: Query,
    },
    Gitlab {
        owner: String,
        repo: String,
        selector: Option<String>,
        query: Query,
    },
    Sourcehut {
        owner: String,
        repo: String,
        selector: Option<String>,
        query: Query,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Query {
    pub dir: Option<String>,
    pub ref_: Option<String>,
    pub rev: Option<String>,
    pub host: Option<String>,
    pub nar_hash: Option<String>,
    pub shallow: bool,
    pub submodules: bool,
    pub lfs: bool,
}

impl Query {
    fn append(&self, out: &mut String) {
        let mut parameters = Vec::new();
        if let Some(value) = non_empty(&self.host) {
            parameters.push(format!("host={}", encode_query_value(value)));
        }
        if let Some(value) = non_empty(&self.dir) {
            parameters.push(format!("dir={}", encode_query_value(value)));
        }
        if let Some(value) = non_empty(&self.ref_) {
            parameters.push(format!("ref={}", encode_query_value(value)));
        }
        if let Some(value) = non_empty(&self.rev) {
            parameters.push(format!("rev={}", encode_query_value(value)));
        }
        if let Some(value) = non_empty(&self.nar_hash) {
            parameters.push(format!("narHash={}", encode_query_value(value)));
        }
        if self.shallow {
            parameters.push("shallow=1".to_string());
        }
        if self.submodules {
            parameters.push("submodules=1".to_string());
        }
        if self.lfs {
            parameters.push("lfs=1".to_string());
        }
        if !parameters.is_empty() {
            out.push('?');
            out.push_str(&parameters.join("&"));
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitTransport {
    Http,
    Https,
    Ssh,
    Git,
    File,
}

impl GitTransport {
    fn prefix(self) -> &'static str {
        match self {
            Self::Http => "git+http",
            Self::Https => "git+https",
            Self::Ssh => "git+ssh",
            Self::Git => "git+git",
            Self::File => "git+file",
        }
    }

    fn from_id(id: &str) -> Option<Self> {
        match id {
            "http" => Some(Self::Http),
            "https" => Some(Self::Https),
            "ssh" => Some(Self::Ssh),
            "git" => Some(Self::Git),
            "file" => Some(Self::File),
            _ => None,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Https => "https",
            Self::Ssh => "ssh",
            Self::Git => "git",
            Self::File => "file",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MercurialTransport {
    Http,
    Https,
    Ssh,
    File,
}

impl MercurialTransport {
    fn prefix(self) -> &'static str {
        match self {
            Self::Http => "hg+http",
            Self::Https => "hg+https",
            Self::Ssh => "hg+ssh",
            Self::File => "hg+file",
        }
    }

    fn from_id(id: &str) -> Option<Self> {
        match id {
            "http" => Some(Self::Http),
            "https" => Some(Self::Https),
            "ssh" => Some(Self::Ssh),
            "file" => Some(Self::File),
            _ => None,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Https => "https",
            Self::Ssh => "ssh",
            Self::File => "file",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileTransport {
    Http,
    Https,
    File,
}

impl FileTransport {
    fn prefix(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Https => "https",
            Self::File => "file",
        }
    }

    fn from_id(id: &str) -> Option<Self> {
        match id {
            "http" => Some(Self::Http),
            "https" => Some(Self::Https),
            "file" => Some(Self::File),
            _ => None,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Https => "https",
            Self::File => "file",
        }
    }
}

pub fn parse(input: &str) -> Result<FlakeRef, String> {
    let text = input.trim();
    if text.is_empty() {
        return Err("flake location is empty".to_string());
    }
    if text.starts_with('/') || text.starts_with("./") || text.starts_with("../") {
        return Ok(FlakeRef::Path {
            path: text.to_string(),
        });
    }
    if let Some(path) = text.strip_prefix("path:") {
        return Ok(FlakeRef::Path {
            path: path.to_string(),
        });
    }
    if let Some(rest) = text.strip_prefix("flake:") {
        return parse_indirect(rest);
    }

    const GIT_PREFIXES: [(&str, GitTransport); 5] = [
        ("git+https:", GitTransport::Https),
        ("git+http:", GitTransport::Http),
        ("git+ssh:", GitTransport::Ssh),
        ("git+file:", GitTransport::File),
        ("git+git:", GitTransport::Git),
    ];
    for (prefix, transport) in GIT_PREFIXES {
        if let Some(rest) = text.strip_prefix(prefix) {
            let (url, query) = split_query(rest);
            return Ok(FlakeRef::Git {
                transport,
                url: strip_authority(url),
                query,
            });
        }
    }

    const HG_PREFIXES: [(&str, MercurialTransport); 4] = [
        ("hg+https:", MercurialTransport::Https),
        ("hg+http:", MercurialTransport::Http),
        ("hg+ssh:", MercurialTransport::Ssh),
        ("hg+file:", MercurialTransport::File),
    ];
    for (prefix, transport) in HG_PREFIXES {
        if let Some(rest) = text.strip_prefix(prefix) {
            let (url, query) = split_query(rest);
            return Ok(FlakeRef::Mercurial {
                transport,
                url: strip_authority(url),
                query,
            });
        }
    }

    if let Some(rest) = text.strip_prefix("tarball+") {
        let (transport, url, query) = split_file_transport(rest)?;
        return Ok(FlakeRef::Tarball {
            transport,
            url,
            nar_hash: query.nar_hash,
        });
    }
    if let Some(rest) = text.strip_prefix("file+") {
        let (transport, url, query) = split_file_transport(rest)?;
        return Ok(FlakeRef::File {
            transport,
            url,
            nar_hash: query.nar_hash,
        });
    }

    if let Some(rest) = text.strip_prefix("github:") {
        let (owner, repo, selector, query) = parse_hosted(rest)?;
        return Ok(FlakeRef::Github {
            owner,
            repo,
            selector,
            query,
        });
    }
    if let Some(rest) = text.strip_prefix("gitlab:") {
        let (owner, repo, selector, query) = parse_hosted(rest)?;
        return Ok(FlakeRef::Gitlab {
            owner,
            repo,
            selector,
            query,
        });
    }
    if let Some(rest) = text.strip_prefix("sourcehut:") {
        let (owner, repo, selector, query) = parse_hosted(rest)?;
        return Ok(FlakeRef::Sourcehut {
            owner,
            repo,
            selector,
            query,
        });
    }

    if let Some(rest) = text.strip_prefix("git:") {
        let (url, query) = split_query(rest);
        let url = strip_authority(url);
        let transport = if url.starts_with('/') {
            GitTransport::File
        } else {
            GitTransport::Git
        };
        return Ok(FlakeRef::Git {
            transport,
            url,
            query,
        });
    }
    if let Some(rest) = text.strip_prefix("ssh:") {
        let (url, query) = split_query(rest);
        return Ok(FlakeRef::Git {
            transport: GitTransport::Ssh,
            url: strip_authority(url),
            query,
        });
    }

    if text.starts_with("https://") || text.starts_with("http://") {
        let (url, query) = split_query(text);
        let transport = if url.starts_with("https://") {
            FileTransport::Https
        } else {
            FileTransport::Http
        };
        let url = strip_authority(url);
        if is_archive(&url) {
            return Ok(FlakeRef::Tarball {
                transport,
                url,
                nar_hash: query.nar_hash,
            });
        }
        return Ok(FlakeRef::File {
            transport,
            url,
            nar_hash: query.nar_hash,
        });
    }

    parse_indirect(text)
}

fn parse_indirect(text: &str) -> Result<FlakeRef, String> {
    let parts: Vec<&str> = text.split('/').filter(|part| !part.is_empty()).collect();
    let Some(id) = parts.first() else {
        return Err("flake registry id is empty".to_string());
    };
    let selector = parts.get(1).map(|part| decode(part));
    let rev = parts.get(2).map(|part| decode(part));
    Ok(FlakeRef::Indirect {
        id: decode(id),
        ref_: selector,
        rev,
    })
}

fn parse_hosted(input: &str) -> Result<(String, String, Option<String>, Query), String> {
    let (rest, query) = split_query(input);
    let parts: Vec<&str> = rest.split('/').filter(|part| !part.is_empty()).collect();
    let (Some(owner), Some(repo)) = (parts.first(), parts.get(1)) else {
        return Err("expected owner/repository in flake location".to_string());
    };
    let selector = if parts.len() > 2 {
        Some(decode(&parts[2..].join("/")))
    } else {
        None
    };
    Ok((decode(owner), decode(repo), selector, query))
}

fn split_file_transport(input: &str) -> Result<(FileTransport, String, Query), String> {
    let (rest, query) = split_query(input);
    for (prefix, transport) in [
        ("https://", FileTransport::Https),
        ("http://", FileTransport::Http),
        ("file://", FileTransport::File),
        ("file:", FileTransport::File),
    ] {
        if let Some(url) = rest.strip_prefix(prefix) {
            return Ok((transport, strip_authority(url), query));
        }
    }
    Err("expected http://, https://, or file:// in flake location".to_string())
}

fn split_query(input: &str) -> (&str, Query) {
    let (base, raw) = match input.split_once('?') {
        Some((base, query)) => (base, Some(query)),
        None => (input, None),
    };
    (base, raw.map(parse_query).unwrap_or_default())
}

fn parse_query(input: &str) -> Query {
    let mut query = Query::default();
    for parameter in input.split('&').filter(|parameter| !parameter.is_empty()) {
        let (key, value) = match parameter.split_once('=') {
            Some((key, value)) => (key, value),
            None => (parameter, "1"),
        };
        let value = decode(value);
        match key {
            "dir" => query.dir = Some(value),
            "ref" => query.ref_ = Some(value),
            "rev" => query.rev = Some(value),
            "host" => query.host = Some(value),
            "narHash" | "narhash" => query.nar_hash = Some(value),
            "shallow" => query.shallow = truthy(&value),
            "submodules" => query.submodules = truthy(&value),
            "lfs" => query.lfs = truthy(&value),
            _ => {}
        }
    }
    query
}

fn truthy(value: &str) -> bool {
    matches!(value, "1" | "true" | "yes" | "on")
}

fn strip_authority(url: &str) -> String {
    url.trim_start_matches("//").to_string()
}

fn is_archive(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    [
        ".zip", ".tar", ".tgz", ".tar.gz", ".tar.xz", ".tar.bz2", ".tar.zst",
    ]
    .iter()
    .any(|suffix| lower.ends_with(suffix))
}

fn non_empty(value: &Option<String>) -> Option<&str> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

pub fn render(reference: &FlakeRef) -> String {
    let mut out = String::new();
    match reference {
        FlakeRef::Indirect { id, ref_, rev } => {
            out.push_str("flake:");
            out.push_str(&encode_path(id));
            if let Some(value) = non_empty(ref_) {
                out.push('/');
                out.push_str(&encode_path(value));
            }
            if let Some(value) = non_empty(rev) {
                out.push('/');
                out.push_str(&encode_path(value));
            }
        }
        FlakeRef::Path { path } => {
            out.push_str("path:");
            out.push_str(path);
        }
        FlakeRef::Git {
            transport,
            url,
            query,
        } => {
            let _ = write!(out, "{}://{}", transport.prefix(), url);
            query.append(&mut out);
        }
        FlakeRef::Mercurial {
            transport,
            url,
            query,
        } => {
            let _ = write!(out, "{}://{}", transport.prefix(), url);
            query.append(&mut out);
        }
        FlakeRef::Tarball {
            transport,
            url,
            nar_hash,
        } => {
            let _ = write!(out, "tarball+{}://{}", transport.prefix(), url);
            append_nar_hash(&mut out, nar_hash);
        }
        FlakeRef::File {
            transport,
            url,
            nar_hash,
        } => {
            let _ = write!(out, "file+{}://{}", transport.prefix(), url);
            append_nar_hash(&mut out, nar_hash);
        }
        FlakeRef::Github {
            owner,
            repo,
            selector,
            query,
        } => {
            render_hosted("github", owner, repo, selector, query, &mut out);
        }
        FlakeRef::Gitlab {
            owner,
            repo,
            selector,
            query,
        } => {
            render_hosted("gitlab", owner, repo, selector, query, &mut out);
        }
        FlakeRef::Sourcehut {
            owner,
            repo,
            selector,
            query,
        } => {
            render_hosted("sourcehut", owner, repo, selector, query, &mut out);
        }
    }
    out
}

fn render_hosted(
    kind: &str,
    owner: &str,
    repo: &str,
    selector: &Option<String>,
    query: &Query,
    out: &mut String,
) {
    let _ = write!(
        out,
        "{kind}:{}/{}",
        encode_query_value(owner),
        encode_query_value(repo)
    );
    if let Some(value) = non_empty(selector) {
        out.push('/');
        out.push_str(&encode_path(value));
    }
    query.append(out);
}

fn append_nar_hash(out: &mut String, nar_hash: &Option<String>) {
    if let Some(value) = non_empty(nar_hash) {
        out.push_str("?narHash=");
        out.push_str(&encode_query_value(value));
    }
}

pub fn git_transport(id: &str) -> Option<GitTransport> {
    GitTransport::from_id(id)
}

pub fn mercurial_transport(id: &str) -> Option<MercurialTransport> {
    MercurialTransport::from_id(id)
}

pub fn file_transport(id: &str) -> Option<FileTransport> {
    FileTransport::from_id(id)
}

pub fn encode_query_value(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char)
            }
            _ => {
                let _ = write!(out, "%{byte:02X}");
            }
        }
    }
    out
}

fn encode_path(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                out.push(byte as char)
            }
            _ => {
                let _ = write!(out, "%{byte:02X}");
            }
        }
    }
    out
}

fn decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let high = hex(bytes[index + 1]);
            let low = hex(bytes[index + 2]);
            if let (Some(high), Some(low)) = (high, low) {
                out.push(high * 16 + low);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(input: &str) {
        let parsed = parse(input).unwrap_or_else(|error| panic!("{input}: {error}"));
        let rendered = render(&parsed);
        let reparsed = parse(&rendered).unwrap_or_else(|error| panic!("{rendered}: {error}"));
        assert_eq!(parsed, reparsed, "{input} rendered as {rendered}");
    }

    #[test]
    fn round_trips_every_url_type() {
        for input in [
            "nixpkgs",
            "nixpkgs/nixos-unstable",
            "nixpkgs/a3a3dda3bacf61e8a39258a0ed9c924eeca8e293",
            "nixpkgs/nixos-unstable/a3a3dda3bacf61e8a39258a0ed9c924eeca8e293",
            "path:/home/user/sub/dir",
            "./sub/dir",
            "../parent",
            "/home/alice/src/patchelf",
            "git+https://github.com/NixOS/patchelf",
            "git+https://github.com/NixOS/patchelf?ref=master",
            "git+https://github.com/NixOS/patchelf?ref=master&rev=f34751b88bd07d7f44f5cd3200fb4122bf916c7e",
            "git+ssh://git@github.com/NixOS/nix?ref=v1.2.3",
            "git+git://github.com/edolstra/dwarffs?ref=unstable",
            "git+file:///home/my-user/some-repo/some-repo",
            "git:/home/user/sub/dir",
            "hg+https://example.org/repo",
            "tarball+https://example.org/flake.tar.gz",
            "https://github.com/NixOS/patchelf/archive/master.tar.gz",
            "file+https://example.org/flake.nix",
            "file+file:///home/user/flake",
            "github:NixOS/nixpkgs",
            "github:NixOS/nixpkgs/nixos-20.09",
            "github:NixOS/nixpkgs/pull/357207/head",
            "github:NixOS/nixpkgs/a3a3dda3bacf61e8a39258a0ed9c924eeca8e293",
            "github:edolstra/nix-warez?dir=blender",
            "github:internal/project?host=company-github.example.org",
            "gitlab:veloren/veloren/master",
            "gitlab:veloren%2Fdev/rfcs",
            "sourcehut:~misterio/nix-colors/main",
            "sourcehut:~misterio/nix-colors?host=hg.sr.ht",
        ] {
            round_trip(input);
        }
    }

    #[test]
    fn renders_nixpkgs_channel() {
        let reference = FlakeRef::Github {
            owner: "NixOS".to_string(),
            repo: "nixpkgs".to_string(),
            selector: Some("nixos-unstable".to_string()),
            query: Query::default(),
        };
        assert_eq!(render(&reference), "github:NixOS/nixpkgs/nixos-unstable");
    }

    #[test]
    fn parses_query_flags() {
        let reference =
            parse("git+https://example.org/repo?dir=pkgs&shallow=1&submodules=1&lfs=1").unwrap();
        let FlakeRef::Git { query, .. } = reference else {
            panic!("not a git reference");
        };
        assert_eq!(query.dir.as_deref(), Some("pkgs"));
        assert!(query.shallow);
        assert!(query.submodules);
        assert!(query.lfs);
    }

    #[test]
    fn encodes_spaces_in_selectors() {
        let reference = FlakeRef::Github {
            owner: "NixOS".to_string(),
            repo: "nixpkgs".to_string(),
            selector: Some("release branch".to_string()),
            query: Query::default(),
        };
        assert_eq!(render(&reference), "github:NixOS/nixpkgs/release%20branch");
    }
}
