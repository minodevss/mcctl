use std::fs;
use std::process::Command;

fn mcctl() -> Command {
    Command::new(env!("CARGO_BIN_EXE_mcctl"))
}

fn running_as_root() -> bool {
    fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find_map(|line| line.strip_prefix("Uid:"))
                .and_then(|ids| ids.split_whitespace().nth(1).map(str::to_owned))
        })
        .is_some_and(|uid| uid == "0")
}

#[test]
fn cli_help_lists_commands() {
    let output = mcctl().arg("--help").output().unwrap();
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    for command in [
        "status",
        "new",
        "delete",
        "start",
        "stop",
        "restart",
        "update",
        "console",
        "logs",
        "doctor",
        "uninstall",
    ] {
        assert!(
            help.lines()
                .any(|line| line.trim_start().starts_with(command)),
            "{command} missing from:\n{help}"
        );
    }
    for hidden in ["serve", "run "] {
        assert!(
            !help
                .lines()
                .any(|line| line.trim_start().starts_with(hidden)),
            "{hidden} should be hidden:\n{help}"
        );
    }
    assert!(help.contains("sudo mcctl new survival fabric 26.3"));
}

#[test]
fn cli_prints_version() {
    let output = mcctl().arg("-V").output().unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        format!("mcctl {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn cli_rejects_bad_usage_with_exit_code_2() {
    let output = mcctl().args(["start", "Not_A_Name"]).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    let output = mcctl().arg("frobnicate").output().unwrap();
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn cli_requires_root_for_mutations() {
    if running_as_root() {
        return;
    }
    let output = mcctl().args(["start", "x"]).output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("sudo mcctl start x"), "{stderr}");
}
