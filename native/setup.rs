//! `sudo porta setup`: give this porta the user namespaces a host restricts.
//!
//! Ubuntu from 23.10 lets an unprivileged process create a user namespace but
//! gives it no rights there, unless an AppArmor profile grants `userns` to the
//! program. The `.deb` brings that profile for `/usr/bin/porta`. A porta
//! installed any other way — the tarball's installer, `almide install` — sits
//! in a directory its user can write, and a profile for that path would let
//! anything the user runs take the name and the grant with it. So setup
//! copies this binary to `/usr/local/bin/porta`, owned by root and writable by
//! nothing else, and writes the profile for that path alone.

use std::path::Path;

/// Where setup puts porta: a directory only root can write to.
const TARGET: &str = "/usr/local/bin/porta";
/// The profile for [`TARGET`], named after its path as Ubuntu names them.
const PROFILE: &str = "/etc/apparmor.d/usr.local.bin.porta";

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

fn profile_text() -> String {
    format!(
        "# Written by `porta setup`: lets {TARGET} create user namespaces, and\n\
         # nothing else; the program is otherwise unconfined, as it was before.\n\
         abi <abi/4.0>,\ninclude <tunables/global>\n\n\
         profile usr.local.bin.porta {TARGET} flags=(unconfined) {{\n  userns,\n\n  include if exists <local/usr.local.bin.porta>\n}}\n"
    )
}

/// What setup will do, in words, before it does it.
fn plan(source: &Path) -> String {
    format!(
        "porta setup will:\n  copy {} to {TARGET}, owned by root, mode 0755\n  write {PROFILE} granting userns to {TARGET} alone, and load it\n",
        source.display()
    )
}

/// `/usr/local/bin` must be root's alone, or the copy is no safer than the original.
fn target_dir_is_root_only() -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    let dir = Path::new(TARGET).parent().unwrap_or(Path::new("/"));
    let meta = std::fs::metadata(dir).map_err(|error| format!("{} cannot be used: {error}", dir.display()))?;
    if meta.uid() != 0 || meta.mode() & 0o022 != 0 {
        return Err(format!("{} is writable by someone other than root; a profile for a binary there would grant userns to whoever replaces it", dir.display()));
    }
    Ok(())
}

fn install(source: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    target_dir_is_root_only()?;
    let staged = format!("{TARGET}.setup");
    std::fs::copy(source, &staged).map_err(|error| format!("cannot copy porta to {staged}: {error}"))?;
    std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755)).map_err(|error| error.to_string())?;
    std::os::unix::fs::chown(&staged, Some(0), Some(0)).map_err(|error| error.to_string())?;
    std::fs::rename(&staged, TARGET).map_err(|error| format!("cannot put porta at {TARGET}: {error}"))?;
    std::fs::write(PROFILE, profile_text()).map_err(|error| format!("cannot write {PROFILE}: {error}"))?;
    apparmor_parser(&["-r", PROFILE])
}

fn apparmor_parser(args: &[&str]) -> Result<(), String> {
    let status = std::process::Command::new("apparmor_parser").args(args).status().map_err(|error| format!("cannot run apparmor_parser: {error}"))?;
    status.success().then_some(()).ok_or_else(|| format!("apparmor_parser {} failed", args.join(" ")))
}

/// Whether the installed porta, run as the user who ran sudo, now has its
/// namespaces: the check that matters is the one without root.
fn verified() -> String {
    use std::os::unix::process::CommandExt;
    let id = |name: &str| std::env::var(name).ok().and_then(|value| value.parse::<u32>().ok());
    let (Some(uid), Some(gid)) = (id("SUDO_UID"), id("SUDO_GID")) else {
        return format!("run `{TARGET} check` as yourself to see the namespaces\n");
    };
    let output = std::process::Command::new(TARGET).arg("check").uid(uid).gid(gid).output();
    match output {
        Ok(output) if String::from_utf8_lossy(&output.stdout).lines().any(|line| line.starts_with("ok") && line.contains("namespace")) => {
            format!("verified: {TARGET} gives each command its own PID, mount and network namespace\n")
        }
        _ => format!("the profile is loaded, but `{TARGET} check` does not show the namespaces yet; run it as yourself to see why\n"),
    }
}

/// The porta a shell finds first, when it is not the one setup installed.
fn shadowed() -> Option<String> {
    let path = std::env::var_os("PATH")?;
    let found = std::env::split_paths(&path).map(|dir| dir.join("porta")).find(|candidate| candidate.is_file())?;
    let found = std::fs::canonicalize(found).ok()?;
    (found != Path::new(TARGET)).then(|| format!("note: `porta` on this PATH is {}; put /usr/local/bin first, or remove that one, to use the one set up\n", found.display()))
}

fn undo() -> Result<String, String> {
    if Path::new(PROFILE).exists() {
        let _ = apparmor_parser(&["-R", PROFILE]);
        std::fs::remove_file(PROFILE).map_err(|error| format!("cannot remove {PROFILE}: {error}"))?;
    }
    let _ = std::fs::remove_file(TARGET);
    Ok(format!("removed {PROFILE} and {TARGET}\n"))
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
    let mut text = plan(&source);
    if mode == "dry-run" {
        return text;
    }
    if !root {
        text.push_str("it needs root for both; run: sudo porta setup\n");
        return text;
    }
    if let Err(reason) = install(&source) {
        return format!("{text}Error: {reason}\n");
    }
    text.push_str(&verified());
    text.push_str(&shadowed().unwrap_or_default());
    text
}
