// The commit `sheepdog --version` names (PLAN.md §10.5): `git rev-parse --short HEAD` at build
// time, or "unknown" outside a git checkout of this crate (a crate tarball, or a crate vendored
// inside another repository, whose HEAD is not this crate's). The watched files come from git
// itself: in a worktree `.git` is a file, and HEAD and the refs live elsewhere.
use std::path::{Path, PathBuf};

fn git(args: &[&str]) -> Option<String> {
    std::process::Command::new("git")
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}

fn main() {
    let here = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap_or_default());
    let ours = git(&["rev-parse", "--show-toplevel"]).and_then(|t| Path::new(&t).canonicalize().ok()) == here.canonicalize().ok();
    let sha = if ours { git(&["rev-parse", "--short", "HEAD"]) } else { None };
    println!("cargo:rustc-env=SHEEPDOG_COMMIT={}", sha.as_deref().unwrap_or("unknown"));
    println!("cargo:rerun-if-changed=build.rs");
    if !ours {
        return;
    }
    let abs = |p: String| if Path::new(&p).is_absolute() { PathBuf::from(p) } else { here.join(p) };
    if let Some(gd) = git(&["rev-parse", "--git-dir"]).map(abs) {
        println!("cargo:rerun-if-changed={}", gd.join("HEAD").display());
    }
    if let Some(cd) = git(&["rev-parse", "--git-common-dir"]).map(abs) {
        for f in ["refs", "packed-refs"] {
            if cd.join(f).exists() {
                println!("cargo:rerun-if-changed={}", cd.join(f).display());
            }
        }
    }
}
