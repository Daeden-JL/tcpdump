// Records the git commit the binary was built from, so the page can show it
// next to the version. Builds without git (or outside a checkout) still work;
// the commit is simply left out.
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs");
    let commit = Command::new("git")
        .args(["rev-parse", "--short=7", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|c| !c.is_empty() && c.chars().all(|ch| ch.is_ascii_hexdigit()))
        .unwrap_or_default();
    println!("cargo:rustc-env=TCPDUMP_DEV_COMMIT={commit}");
}
