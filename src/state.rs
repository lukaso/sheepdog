//! Where sheepr keeps its state (PHASE2.md §1 decision 1, §0.2).
//!
//! Release: `$SHEEPR_STATE`, else `$XDG_STATE_HOME/sheepr` (only an absolute value, as the
//! XDG spec requires), else `$HOME/.local/state/sheepr`. Debug (the state wall): only
//! `$SHEEPR_TEST_STATE`, and only if that directory holds the sentinel `.sheepr-test` the
//! tests create; `SHEEPR_STATE`, `XDG_STATE_HOME` and `HOME` are never read. None = no state:
//! no journal, no sweep.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The sentinel a test state directory holds.
pub const SENTINEL: &str = ".sheepr-test";

/// The state directory, from an injected environment (`var`) and file test (`exists`).
#[allow(dead_code)] // wired in P1 (PHASE2.md §2)
pub fn resolve(debug: bool, var: impl Fn(&str) -> Option<OsString>, exists: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    let set = |k: &str| var(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    if debug {
        let d = set("SHEEPR_TEST_STATE")?;
        return exists(&d.join(SENTINEL)).then_some(d);
    }
    if let Some(d) = set("SHEEPR_STATE") {
        return Some(d);
    }
    if let Some(x) = set("XDG_STATE_HOME").filter(|x| x.is_absolute()) {
        return Some(x.join("sheepr"));
    }
    set("HOME").map(|h| h.join(".local/state/sheepr"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let m: HashMap<String, OsString> = pairs.iter().map(|(k, v)| (k.to_string(), OsString::from(v))).collect();
        move |k| m.get(k).cloned()
    }
    fn with_sentinel(dir: &'static str) -> impl Fn(&Path) -> bool {
        move |p| p == Path::new(dir).join(SENTINEL)
    }
    fn none(_: &Path) -> bool {
        false
    }

    /// The state wall: a debug build never reads the operator's variables, however they are
    /// set, and needs the sentinel.
    #[test]
    fn a_debug_build_uses_only_a_sentinelled_test_state() {
        let operator = [("SHEEPR_STATE", "/real/state"), ("XDG_STATE_HOME", "/real/xdg"), ("HOME", "/Users/op")];
        assert_eq!(resolve(true, env(&operator), none), None, "the operator's variables");
        let mut canary = operator.to_vec();
        canary.push(("SHEEPR_TEST_STATE", "/tmp/canary"));
        assert_eq!(resolve(true, env(&canary), none), None, "a test state without the sentinel");
        assert_eq!(resolve(true, env(&canary), with_sentinel("/tmp/canary")), Some(PathBuf::from("/tmp/canary")), "control");
        assert_eq!(resolve(true, env(&[("SHEEPR_TEST_STATE", "")]), |_| true), None, "empty");
    }

    #[test]
    fn a_release_build_follows_the_documented_order() {
        let all = [("SHEEPR_STATE", "/s"), ("XDG_STATE_HOME", "/x"), ("HOME", "/h"), ("SHEEPR_TEST_STATE", "/t")];
        assert_eq!(resolve(false, env(&all), |_| true), Some(PathBuf::from("/s")));
        assert_eq!(resolve(false, env(&all[1..]), |_| true), Some(PathBuf::from("/x/sheepr")));
        assert_eq!(resolve(false, env(&[("XDG_STATE_HOME", "rel"), ("HOME", "/h")]), none), Some(PathBuf::from("/h/.local/state/sheepr")), "a relative XDG value is ignored");
        assert_eq!(resolve(false, env(&all[2..]), |_| true), Some(PathBuf::from("/h/.local/state/sheepr")));
        assert_eq!(resolve(false, env(&[]), |_| true), None);
    }
}
