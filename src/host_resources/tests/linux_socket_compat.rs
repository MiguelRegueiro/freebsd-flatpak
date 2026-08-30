use std::fs;
use std::path::PathBuf;
use std::process::Command;

#[test]
fn peer_pidfd_einval_is_reported_as_an_unsupported_socket_option() {
    let project_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let output_dir = project_root.join("target/linux-socket-compat-tests");
    fs::create_dir_all(&output_dir).unwrap();
    let output = output_dir.join("socket-option-errno-shim-test");
    let compile = Command::new("/compat/linux/usr/bin/gcc")
        .args(["-O2", "-Wall", "-Wextra", "-Werror"])
        .arg("-DSOCKET_OPTION_ERRNO_SHIM_TEST")
        .arg(project_root.join("compatibility_helpers/socket-option-errno-shim.c"))
        .arg(project_root.join("tests/socket-option-errno-shim-test.c"))
        .arg("-o")
        .arg(&output)
        .env(
            "PATH",
            "/compat/linux/usr/bin:/compat/linux/bin:/usr/bin:/bin",
        )
        .status()
        .expect("compile socket-option errno shim test");
    assert!(compile.success());

    let run = Command::new(&output)
        .status()
        .expect("run socket-option errno shim test");
    assert!(run.success());
}
