//! Process exit for `viewr version`. Parse errors leave through `main`.

use std::process::Command;

#[test]
fn version_help_prints_and_an_extra_argument_exits_2() {
    let bin = env!("CARGO_BIN_EXE_viewr");
    let help = Command::new(bin)
        .args(["version", "--help"])
        .output()
        .expect("run viewr version --help");
    assert!(help.status.success(), "status {:?}", help.status);
    let stdout = String::from_utf8(help.stdout).expect("help stdout");
    assert!(
        stdout.contains("viewr version - show the version"),
        "{stdout}"
    );
    assert_eq!(help.stderr.as_slice(), b"");

    let extra = Command::new(bin)
        .args(["version", "extra"])
        .output()
        .expect("run viewr version extra");
    assert_eq!(extra.status.code(), Some(2), "status {:?}", extra.status);
    let stderr = String::from_utf8(extra.stderr).expect("extra stderr");
    assert!(
        stderr.contains("`viewr version` takes no argument 'extra'"),
        "{stderr}"
    );
}
