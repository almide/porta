//! What this host can enforce, said before the first run.
//!
//! porta refuses a run whose policy the kernel cannot express. That is the
//! right answer at run time and a poor first impression, so `porta check`
//! asks the same questions the run would and prints the answers: which
//! primitives are present, what each one covers, and what porta will
//! therefore refuse here. `--json` gives the same report to a machine.

/// One enforcement primitive and whether this host has it.
struct Primitive {
    name: &'static str,
    covers: &'static str,
    present: bool,
    /// What porta does when it is absent.
    otherwise: &'static str,
}

fn render(platform: &str, primitives: &[Primitive]) -> String {
    let mut text = format!("porta on {platform}\n\n");
    for primitive in primitives {
        let mark = if primitive.present { "ok " } else { "-- " };
        text.push_str(&format!("{mark}{:<44} {}\n", primitive.name, primitive.covers));
        if !primitive.present {
            text.push_str(&format!("   {}\n", primitive.otherwise));
        }
    }
    let missing = primitives.iter().filter(|primitive| !primitive.present).count();
    text.push('\n');
    if missing == 0 {
        text.push_str("Every rule porta can express is enforced on this host.\n");
    } else {
        text.push_str("A rule this host cannot enforce refuses the run rather than running with less.\n");
    }
    text
}

fn render_json(platform: &str, primitives: &[Primitive]) -> String {
    let quoted = |text: &str| format!("\"{}\"", crate::json_text::escape_json_text(text));
    let items: Vec<String> = primitives
        .iter()
        .map(|primitive| {
            format!(
                "{{\"name\":{},\"covers\":{},\"present\":{},\"otherwise\":{}}}",
                quoted(primitive.name),
                quoted(primitive.covers),
                primitive.present,
                quoted(primitive.otherwise),
            )
        })
        .collect();
    let missing = primitives.iter().filter(|primitive| !primitive.present).count();
    format!(
        "{{\"platform\":{},\"all_enforced\":{},\"missing\":{},\"primitives\":[{}]}}",
        quoted(platform),
        missing == 0,
        missing,
        items.join(","),
    )
}

/// The human report: which primitives this host has and what porta refuses.
pub fn wt_sandbox_check() -> String {
    let (platform, primitives) = probe();
    render(&platform, &primitives)
}

/// The same report as one JSON object.
pub fn wt_sandbox_check_json() -> String {
    let (platform, primitives) = probe();
    render_json(&platform, &primitives)
}

#[cfg(target_os = "macos")]
fn probe() -> (String, Vec<Primitive>) {
    let exec = std::path::Path::new("/usr/bin/sandbox-exec").exists();
    let version = std::process::Command::new("/usr/bin/sw_vers")
        .arg("-productVersion")
        .output()
        .ok()
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    (
        format!("macOS {version}"),
        vec![
            Primitive {
                name: "sandbox-exec (Seatbelt)",
                covers: "writes, reads, TCP ports, Unix sockets, mach services, process info",
                present: exec,
                otherwise: "no native command can run; porta will refuse every run",
            },
            Primitive {
                name: "denial log (unified log)",
                covers: "after a failed run, which flag each refusal would have needed",
                present: std::path::Path::new("/usr/bin/log").exists(),
                otherwise: "runs are enforced but refusals are not explained",
            },
        ],
    )
}

#[cfg(target_os = "linux")]
fn probe() -> (String, Vec<Primitive>) {
    let abi = crate::landlock::abi_version().unwrap_or(0);
    let seccomp = crate::seccomp::available();
    let kernel = std::fs::read_to_string("/proc/sys/kernel/osrelease").map(|text| text.trim().to_string()).unwrap_or_default();
    let landlock = |min: i64| abi >= min;
    (
        format!("Linux {kernel}, Landlock ABI {abi}"),
        vec![
            Primitive {
                name: "Landlock filesystem rules (ABI 1)",
                covers: "writes outside mounts; reads under --read-policy strict",
                present: landlock(1),
                otherwise: "no native command can run; porta will refuse every run",
            },
            Primitive {
                name: "cross-directory rename and link (ABI 2)",
                covers: "mv and ln between directories inside a mount",
                present: landlock(2),
                otherwise: "they fail with EXDEV inside a mount on this kernel",
            },
            Primitive {
                name: "truncate as its own right (ABI 3)",
                covers: "truncating a file outside every mount is refused",
                present: landlock(3),
                otherwise: "truncate is not separately controlled on this kernel",
            },
            Primitive {
                name: "TCP port rules (ABI 4)",
                covers: "--allow-net, --allow-bind, proxy mode",
                present: landlock(4),
                otherwise: "--allow-net and --proxy-allow are refused here",
            },
            Primitive {
                name: "signal and abstract-socket scoping (ABI 6)",
                covers: "no signals to, no abstract sockets of, processes outside the sandbox",
                present: landlock(6),
                otherwise: "those two channels stay open on this kernel",
            },
            Primitive {
                name: "seccomp filter",
                covers: "ptrace, process_vm_*, mounts, namespaces, io_uring, raw/packet sockets, MPTCP",
                present: seccomp,
                otherwise: "porta will refuse every run: the baseline filter is part of the policy",
            },
        ],
    )
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn probe() -> (String, Vec<Primitive>) {
    (
        std::env::consts::OS.to_string(),
        vec![Primitive {
            name: "native sandbox",
            covers: "everything",
            present: false,
            otherwise: "porta has no enforcement backend on this platform and refuses every native run",
        }],
    )
}
