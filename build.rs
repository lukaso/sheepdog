// The commit `sheepdog --version` names (PLAN.md §10.5): `git rev-parse --short HEAD` at build
// time, or "unknown" outside a git checkout of this crate (a crate tarball, or a crate vendored
// inside another repository, whose HEAD is not this crate's). The watched files come from git
// itself: in a worktree `.git` is a file, and HEAD and the refs live elsewhere. Only this
// checkout's HEAD and its branch's ref are watched (a commit in another worktree, or a fetch,
// does not rebuild this one, unless its branch is packed: then the branch's directory is watched).
use std::path::{Path, PathBuf};

fn git(args: &[&str]) -> Option<String> {
    std::process::Command::new("git")
        .args(args)
        // an inherited GIT_DIR would make any directory look like that repository's top level
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_INDEX_FILE")
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
        // the branch HEAD names (none when detached: then HEAD itself changes on a commit)
        let reftable = cd.join("reftable").exists();
        if let Some(r) = git(&["symbolic-ref", "-q", "HEAD"]).filter(|_| !reftable) {
            // a packed branch has no loose file until its next commit writes one: watch the
            // directory that file will appear in (a missing watched file rebuilds every time)
            let f = cd.join(r);
            // (a branch named a/b loses its empty directory too: the nearest one that exists)
            let w = f.ancestors().find(|p| p.exists()).map_or(f.clone(), Path::to_path_buf);
            println!("cargo:rerun-if-changed={}", w.display());
        }
        // packed refs, and the reftable backend (its refs are all under reftable/)
        for f in ["packed-refs", "reftable"] {
            if cd.join(f).exists() {
                println!("cargo:rerun-if-changed={}", cd.join(f).display());
            }
        }
    }
}
