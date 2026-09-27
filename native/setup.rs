//! `sudo porta setup`: give this porta the user namespaces a host restricts.
//!
//! Ubuntu from 23.10 lets an unprivileged process create a user namespace but
//! gives it no rights there, unless an AppArmor profile grants `userns` to the
//! program. The `.deb` brings that profile for `/usr/bin/porta`. A porta
//! installed any other way — the tarball's installer, `almide install` — sits
//! in a directory its user can write, and a profile for that path would let
//! anything the user runs take the name and the grant with it. So setup
//! copies this binary to a directory only root can write, every directory
//! above it too, and writes the profile for that path alone:
//! `/usr/local/bin/porta` where `/usr/local/bin` is root's alone, otherwise
//! `/usr/libexec/porta/porta` with a link to it from `/usr/local/bin` (a CI
//! runner's image leaves `/usr/local/bin`, and `/opt`, writable by its user;
//! `/usr/libexec` is the package manager's). A profile
//! attaches to the file a link resolves to, so replacing the link grants
//! nothing.

use std::path::{Path, PathBuf};

/// The directories setup may put porta in, in order of preference.
const DIRECTORIES: [&str; 2] = ["/usr/local/bin", "/usr/libexec/porta"];
/// Where a shell finds porta once set up.
const LINK: &str = "/usr/local/bin/porta";

/// Why this host needs no setup, or `None` when it does.
fn not_needed() -> Option<&'static str> {
    if !cfg!(target_os = "linux") {
        return Some("only Linux restricts user namespaces through AppArmor; nothing to set up here");
    }
    let restricted = std::fs::read_to_string("/proc/sys/kernel/apparmor_restrict_unprivileged_userns").map(|value| value.trim() == "1").unwrap_or(false);
    if !restricted {
        return Some("this host does not restrict unprivileged user namespaces; porta has them already");
    }
    if !Path::new("/etc/apparmor.d/abi/4.0").exists() {
        return Some("this host's AppArmor predates the userns rule (Ubuntu 22.04 and earlier) and does not restrict user namespaces with it");
    }
    None
}

/// The first of `dir` and the directories above it that someone other than
/// root can write, or `None` when root alone can. One that does not exist yet
/// is setup's to create, as root, with no write for anyone else.
fn writable_by_others(dir: &Path) -> Option<PathBuf> {
    use std::os::unix::fs::MetadataExt;
    dir.ancestors()
        .find(|path| std::fs::metadata(path).is_ok_and(|meta| meta.uid() != 0 || meta.mode() & 0o022 != 0))
        .map(Path::to_path_buf)
}

/// The first of [`DIRECTORIES`] only root can write, as the path porta goes to.
fn target() -> Result<PathBuf, String> {
    let mut reasons = Vec::new();
    for dir in DIRECTORIES.iter().map(Path::new) {
        match writable_by_others(dir) {
            None => return Ok(dir.join("porta")),
            Some(open) => reasons.push(format!("{} (through {})", dir.display(), open.display())),
        }
    }
    Err(format!("someone other than root can write {}, so a profile for porta there would grant userns to whoever replaces it", reasons.join(" and ")))
}

/// The profile for `target`, named after its path as Ubuntu names them.
fn profile_path(target: &Path) -> PathBuf {
    let name = target.to_string_lossy().trim_start_matches('/').replace('/', ".");
    PathBuf::from("/etc/apparmor.d").join(name)
}

fn profile_text(target: &Path) -> String {
    let name = profile_path(target).file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default();
    format!(
        "# Written by `porta setup`: lets {} create user namespaces, and\n\
         # nothing else; the program is otherwise unconfined, as it was before.\n\
         abi <abi/4.0>,\ninclude <tunables/global>\n\n\
         profile {name} {} flags=(unconfined) {{\n  userns,\n\n  include if exists <local/{name}>\n}}\n",
        target.display(),
        target.display()
    )
}

/// What setup will do, in words, before it does it.
fn plan(source: &Path, target: &Path) -> String {
    let mut text = format!(
        "porta setup will:\n  copy {} to {}, owned by root, mode 0755\n  write {} granting userns to {} alone, and load it\n",
        source.display(),
        target.display(),
        profile_path(target).display(),
        target.display()
    );
    if target != Path::new(LINK) {
        text.push_str(&format!("  link {LINK} to it (/usr/local/bin is not root's alone here)\n"));
    }
    text
}

fn install(source: &Path, target: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let dir = target.parent().unwrap_or(Path::new("/"));
    std::fs::create_dir_all(dir).map_err(|error| format!("cannot create {}: {error}", dir.display()))?;
    let staged = target.with_extension("setup");
    std::fs::copy(source, &staged).map_err(|error| format!("cannot copy porta to {}: {error}", staged.display()))?;
    std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755)).map_err(|error| error.to_string())?;
    std::os::unix::fs::chown(&staged, Some(0), Some(0)).map_err(|error| error.to_string())?;
    std::fs::rename(&staged, target).map_err(|error| format!("cannot put porta at {}: {error}", target.display()))?;
    if target != Path::new(LINK) && Path::new(LINK).parent().is_some_and(Path::exists) {
        let _ = std::fs::remove_file(LINK);
        std::os::unix::fs::symlink(target, LINK).map_err(|error| format!("cannot link {LINK}: {error}"))?;
    }
    let profile = profile_path(target);
    std::fs::write(&profile, profile_text(target)).map_err(|error| format!("cannot write {}: {error}", profile.display()))?;
    apparmor_parser(&["-r", &profile.to_string_lossy()])
}

fn apparmor_parser(args: &[&str]) -> Result<(), String> {
    let status = std::process::Command::new("apparmor_parser").args(args).status().map_err(|error| format!("cannot run apparmor_parser: {error}"))?;
    status.success().then_some(()).ok_or_else(|| format!("apparmor_parser {} failed", args.join(" ")))
}

/// Whether the installed porta, run as the user who ran sudo, now has its
/// namespaces: the check that matters is the one without root.
fn verified(target: &Path) -> String {
    use std::os::unix::process::CommandExt;
    let id = |name: &str| std::env::var(name).ok().and_then(|value| value.parse::<u32>().ok());
    let (Some(uid), Some(gid)) = (id("SUDO_UID"), id("SUDO_GID")) else {
        return format!("run `{} check` as yourself to see the namespaces\n", target.display());
    };
    let output = std::process::Command::new(target).arg("check").uid(uid).gid(gid).output();
    match output {
        Ok(output) if String::from_utf8_lossy(&output.stdout).lines().any(|line| line.starts_with("ok") && line.contains("namespace")) => {
            format!("verified: {} gives each command its own PID, mount and network namespace\n", target.display())
        }
        _ => format!("the profile is loaded, but `{} check` does not show the namespaces yet; run it as yourself to see why\n", target.display()),
    }
}

/// The porta a shell finds first, when it is not the one setup installed.
fn shadowed(target: &Path) -> Option<String> {
    let path = std::env::var_os("PATH")?;
    let found = std::env::split_paths(&path).map(|dir| dir.join("porta")).find(|candidate| candidate.is_file())?;
    let found = std::fs::canonicalize(found).ok()?;
    (found != target).then(|| format!("note: `porta` on this PATH is {}; put /usr/local/bin first, or remove that one, to use the one set up\n", found.display()))
}

fn undo() -> Result<String, String> {
    let mut removed = Vec::new();
    for target in DIRECTORIES.iter().map(|dir| Path::new(dir).join("porta")) {
        let profile = profile_path(&target);
        if profile.exists() {
            let _ = apparmor_parser(&["-R", &profile.to_string_lossy()]);
            std::fs::remove_file(&profile).map_err(|error| format!("cannot remove {}: {error}", profile.display()))?;
            let _ = std::fs::remove_file(&target);
            removed.push(format!("{} and {}", profile.display(), target.display()));
        }
    }
    if std::fs::read_link(LINK).is_ok() {
        let _ = std::fs::remove_file(LINK);
    }
    Ok(if removed.is_empty() { "nothing to undo\n".to_string() } else { format!("removed {}\n", removed.join("; ")) })
}

/// `porta setup [--dry-run | --undo]`.
pub fn wt_setup(mode: impl AsRef<str>) -> String {
    setup(mode.as_ref()).trim_end().to_string()
}

fn setup(mode: &str) -> String {
    let root = unsafe { libc::geteuid() } == 0;
    if mode == "undo" {
        return if root { undo().unwrap_or_else(|reason| format!("Error: {reason}\n")) } else { "Error: porta setup --undo needs root; run it with sudo\n".into() };
    }
    if let Some(reason) = not_needed() {
        return format!("{reason}\n");
    }
    let source = match std::env::current_exe().and_then(std::fs::canonicalize) {
        Ok(source) => source,
        Err(error) => return format!("Error: cannot find porta's own binary: {error}\n"),
    };
    let target = match target() {
        Ok(target) => target,
        Err(reason) => return format!("Error: {reason}\n"),
    };
    let mut text = plan(&source, &target);
    if mode == "dry-run" {
        return text;
    }
    if !root {
        text.push_str("it needs root for both; run: sudo porta setup\n");
        return text;
    }
    if let Err(reason) = install(&source, &target) {
        return format!("{text}Error: {reason}\n");
    }
    text.push_str(&verified(&target));
    text.push_str(&shadowed(&target).unwrap_or_default());
    text
}
