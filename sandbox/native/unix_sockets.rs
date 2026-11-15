//! The Unix sockets a run closes on Linux, as paths.
//!
//! The preset names credential sockets by pattern (`[unix] deny`, and
//! `--deny-unix`), and macOS matches the pattern at `connect`. Linux has no
//! rule over a pathname socket's `connect` that Landlock or seccomp can
//! express, so porta finds the sockets bound on this host now, in
//! `/proc/net/unix`, and covers each one that matches with `/dev/null` in the
//! command's mount namespace: a `connect` there reaches a character device
//! and is refused. A socket bound after the run starts is not covered.
#![cfg(target_os = "linux")]

use std::os::unix::fs::FileTypeExt;

/// The pathname sockets bound in this network namespace. Each line of
/// `/proc/net/unix` ends in the path, when there is one; the fields before it
/// hold no `/`, so the first ` /` starts it. Abstract sockets (`@…`) have no
/// path, and Landlock's scope already keeps the command from those outside.
fn bound() -> Vec<String> {
    let table = std::fs::read_to_string("/proc/net/unix").unwrap_or_default();
    let mut paths: Vec<String> = table.lines().skip(1).filter_map(|line| line.find(" /").map(|at| line[at + 1..].to_string())).collect();
    paths.sort();
    paths.dedup();
    paths
}

/// The sockets `deny_unix` matches and `allowed` does not name, each as the
/// path the kernel resolves and only where it is still a socket this user
/// can reach. One beneath a path `hidden` covers is closed with it already,
/// and could not be mounted on once it is.
pub(crate) fn closed(deny_unix: &[String], allowed: &[String], hidden: &[String]) -> Result<Vec<String>, String> {
    let patterns = deny_unix
        .iter()
        .map(|pattern| regex::Regex::new(pattern).map_err(|error| format!("--deny-unix {pattern} is not a regular expression: {error}")))
        .collect::<Result<Vec<_>, _>>()?;
    let resolve = |path: &String| std::fs::canonicalize(path).map(|path| path.to_string_lossy().to_string()).unwrap_or_else(|_| path.clone());
    let allowed: Vec<String> = allowed.iter().map(resolve).collect();
    let mut sockets = Vec::new();
    for path in bound() {
        if !patterns.iter().any(|pattern| pattern.is_match(&path)) {
            continue;
        }
        let Ok(meta) = std::fs::metadata(&path) else { continue };
        let real = resolve(&path);
        let beneath = |dir: &String| real == *dir || real.starts_with(&format!("{dir}/"));
        if !meta.file_type().is_socket() || allowed.contains(&real) || hidden.iter().any(beneath) || sockets.contains(&real) {
            continue;
        }
        sockets.push(real);
    }
    Ok(sockets)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bound_socket_that_matches_is_closed_unless_allowed() {
        let dir = std::env::temp_dir().join(format!("porta-unix-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("agent.sock");
        let _listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let real = std::fs::canonicalize(&path).unwrap().to_string_lossy().to_string();
        let pattern = vec![r"/agent\.sock$".to_string()];
        assert!(closed(&pattern, &[], &[]).unwrap().contains(&real));
        assert!(!closed(&pattern, &[real.clone()], &[]).unwrap().contains(&real));
        assert!(!closed(&pattern, &[], &[dir.to_string_lossy().to_string()]).unwrap().contains(&real));
        assert!(closed(&["(".to_string()], &[], &[]).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
