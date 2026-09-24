//! Reading refusals back from a strace log.

use super::*;

#[test]
fn a_policy_refusal_is_kept_and_a_permission_refusal_is_not() -> std::io::Result<()> {
    let home = std::env::temp_dir().join(format!("porta-why-{}", std::process::id()));
    std::fs::create_dir_all(&home)?;
    let home = home.to_string_lossy().to_string();
    let trace = format!(
        "10 openat(AT_FDCWD</w>, \"{home}/secret\", O_RDONLY) = -1 EACCES (Permission denied)\n\
         10 openat(AT_FDCWD</w>, \"/etc/shadow\", O_RDONLY) = -1 EACCES (Permission denied)\n\
         11 mkdirat(AT_FDCWD</w>, \"{home}/new\", 0777 <unfinished ...>\n\
         11 <... mkdirat resumed>) = -1 EACCES (Permission denied)\n\
         12 connect(3<socket:[9]>, {{sa_family=AF_INET, sin_port=htons(443), sin_addr=inet_addr(\"1.1.1.1\")}}, 16) = -1 EACCES (Permission denied)\n\
         12 socket(AF_INET, SOCK_DGRAM|SOCK_CLOEXEC, IPPROTO_IP) = -1 EAFNOSUPPORT (Address family not supported by protocol)\n"
    );
    std::fs::write(format!("{home}/secret"), "x")?;
    let found = refusals(&trace, &[]);
    let targets: Vec<&str> = found.iter().map(|denial| denial.target.as_str()).collect();
    assert!(targets.contains(&format!("{home}/secret").as_str()), "{targets:?}");
    assert!(targets.contains(&format!("{home}/new").as_str()), "{targets:?}");
    assert!(targets.contains(&"remote:*:443"), "{targets:?}");
    assert!(targets.contains(&"AF_INET SOCK_DGRAM|SOCK_CLOEXEC"), "{targets:?}");
    // Whoever may read shadow outside is refused it by nothing but the policy.
    if std::fs::File::open("/etc/shadow").is_err() {
        assert!(!targets.contains(&"/etc/shadow"), "{targets:?}");
    }
    std::fs::remove_dir_all(&home)
}

#[test]
fn a_relative_path_is_joined_to_its_directory() {
    assert_eq!(paths("AT_FDCWD</work>, \"a/b\", O_RDONLY"), vec!["/work/a/b".to_string()]);
    assert_eq!(paths("3</old>, \"x\", 4</new>, \"y\", 0"), vec!["/old/x".to_string(), "/new/y".to_string()]);
    assert_eq!(paths("\"/abs \\\"q\\\"\""), vec!["/abs \"q\"".to_string()]);
}
