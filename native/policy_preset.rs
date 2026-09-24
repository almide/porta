//! What a run closes beyond its own grants, as data rather than code.
//!
//! porta's enforcement knows three things it can do: deny a read, protect a
//! name inside a writable mount, refuse a Unix socket. Which paths, names and
//! sockets deserve it is a policy, and differs between hosts and people, so it
//! lives in a preset — `presets/default.toml`, shipped inside the binary and
//! shown by `porta explain` — and in what the caller adds with `--deny-read`,
//! `--protect`, `--deny-unix` or the same keys in `porta.toml`. `--preset none`
//! starts from nothing; `--preset <file>` starts from another document.

const DEFAULT_PRESET: &str = include_str!("presets/default.toml");

/// Everything one run closes beyond its grants, resolved for this host.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Closures {
    /// Absolute paths, as the kernel resolves them, closed to reads. A path
    /// that does not exist on this host is kept as written: there is nothing
    /// there to read, and on macOS the rule still covers one created later.
    pub(crate) deny_read: Vec<String>,
    /// Names relative to each writable mount's root.
    pub(crate) protect: Vec<String>,
    /// Regular expressions over a Unix socket's path.
    pub(crate) deny_unix: Vec<String>,
}

#[derive(serde::Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct Document {
    read: Section,
    write: WriteSection,
    unix: Section,
}

#[derive(serde::Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct Section {
    deny: Vec<String>,
}

#[derive(serde::Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct WriteSection {
    protect: Vec<String>,
}

/// The preset a request named: `default`, `none`, or a path to a preset file.
fn preset_text(preset: &str) -> Result<String, String> {
    match preset {
        "" | "default" => Ok(DEFAULT_PRESET.to_string()),
        "none" => Ok(String::new()),
        path => std::fs::read_to_string(path).map_err(|error| format!("--preset {path} cannot be read: {error}")),
    }
}

/// The preset's closures plus the caller's own, resolved against `home`.
pub(crate) fn resolve(preset: &str, extra: &Closures, home: Option<&str>) -> Result<Closures, String> {
    let document: Document = toml::from_str(&preset_text(preset)?).map_err(|error| format!("--preset {preset}: {error}"))?;
    let mut closures = Closures::default();
    for entry in document.read.deny.iter().chain(&extra.deny_read) {
        let path = resolved_path(entry, home)?;
        if !closures.deny_read.contains(&path) {
            closures.deny_read.push(path);
        }
    }
    for name in document.write.protect.iter().chain(&extra.protect) {
        let name = protected_name(name)?;
        if !closures.protect.contains(&name) {
            closures.protect.push(name);
        }
    }
    for pattern in document.unix.deny.iter().chain(&extra.deny_unix) {
        // A control character would carry the profile's rule onto a second
        // line; no socket path needs one.
        if pattern.is_empty() || pattern.chars().any(char::is_control) || regex::Regex::new(pattern).is_err() {
            return Err(format!("--deny-unix takes a regular expression over a socket path, not {pattern:?}"));
        }
        if !closures.deny_unix.contains(pattern) {
            closures.deny_unix.push(pattern.clone());
        }
    }
    Ok(closures)
}

/// `~/x` against the home, then the path the kernel resolves: a rule on a
/// symlink would name a path nothing ever opens. A path under the home with no
/// home known is refused rather than guessed.
fn resolved_path(entry: &str, home: Option<&str>) -> Result<String, String> {
    let expanded = match entry.strip_prefix("~/") {
        Some(rest) => format!("{}/{rest}", home.ok_or_else(|| format!("{entry} names the home directory, and HOME is not set"))?),
        None if entry.starts_with('/') => entry.to_string(),
        None => return Err(format!("--deny-read takes an absolute path or ~/…, not {entry}")),
    };
    Ok(std::fs::canonicalize(&expanded).map(|path| path.to_string_lossy().to_string()).unwrap_or(expanded))
}

/// A name relative to a mount root, without a way out of it.
fn protected_name(name: &str) -> Result<String, String> {
    let trimmed = name.trim_matches('/');
    if trimmed.is_empty() || trimmed.split('/').any(|part| part == ".." || part == "." || part.is_empty()) {
        return Err(format!("--protect takes a name inside a mount, like .envrc or .claude/commands, not {name}"));
    }
    Ok(trimmed.to_string())
}

/// The shipped default, for `porta explain --preset` and the tests.
pub(crate) fn default_text() -> &'static str {
    DEFAULT_PRESET
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_parses_and_resolves_home_paths() {
        let closures = resolve("default", &Closures::default(), Some("/home/u")).unwrap();
        assert!(closures.deny_read.contains(&"/home/u/.aws".to_string()));
        assert!(closures.protect.contains(&".envrc".to_string()));
        assert!(!closures.deny_unix.is_empty());
    }

    #[test]
    fn none_leaves_only_what_the_caller_adds() {
        let extra = Closures { deny_read: vec!["~/secret".into()], ..Closures::default() };
        let closures = resolve("none", &extra, Some("/home/u")).unwrap();
        assert_eq!(closures.deny_read, vec!["/home/u/secret".to_string()]);
        assert!(closures.protect.is_empty());
    }

    #[test]
    fn a_protected_name_cannot_leave_the_mount() {
        for name in ["../x", "a/../b", "", "/"] {
            assert!(protected_name(name).is_err(), "{name}");
        }
    }

    #[test]
    fn a_socket_pattern_must_be_a_regular_expression() {
        let extra = Closures { deny_unix: vec!["(".into()], ..Closures::default() };
        assert!(resolve("none", &extra, Some("/home/u")).is_err());
    }
}
