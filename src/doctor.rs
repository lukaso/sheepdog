//! `sheepr doctor [--json] [--grants]` (PLAN.md §4.3, §4.4; PHASE2.md "P8 design"): which
//! mechanisms work on this machine, what is degraded and why, and the state's journals. Every
//! mechanism is probed on every call (nothing is cached). `--grants` (macOS) also probes whether
//! a disclaimed child can read `~/Documents`; that can raise a macOS privacy prompt, so it is off
//! by default.

#[cfg(target_os = "linux")]
use crate::linux as os;
#[cfg(target_os = "macos")]
use crate::macos as os;
use crate::fail;
use sheepr::ident::same;
use std::ffi::OsString;
use std::io::Write;
use std::os::unix::ffi::OsStrExt;

pub(crate) struct Mechanism {
    name: &'static str,
    ok: bool,
    detail: String,
}

pub(crate) const USAGE: &str = "sheepr doctor [--json] [--grants]";

pub fn main(args: &[OsString]) -> i32 {
    let (mut json, mut grants) = (false, false);
    for a in args {
        match a.as_bytes() {
            b"--json" => json = true,
            b"--grants" => grants = true,
            _ => {
                let what = if a.as_bytes().starts_with(b"-") { "unknown option" } else { "unexpected argument" };
                fail!("sheepr: doctor: {what} {}.", crate::shown(a));
                crate::say!("usage: {USAGE}");
                return 2;
            }
        }
    }
    let mechanisms = os::doctor_mechanisms();
    let mut degraded: Vec<String> = mechanisms.iter().filter(|m| !m.ok).map(|m| format!("{}: {}", m.name, m.detail)).collect();
    let notes = os::doctor_notes();
    let journals = journals();
    let grant = if grants { os::doctor_grant() } else { None };
    if grant == Some(false) {
        degraded.push("privacy: a disclaimed job cannot read ~/Documents (give sheepr Full Disk Access, or run with --inherit-terminal-permissions)".into());
    }
    let out = if json {
        let esc = crate::journal::json_str;
        let ms: Vec<String> = mechanisms.iter().map(|m| format!("{{\"name\":{},\"ok\":{},\"detail\":{}}}", esc(m.name), m.ok, esc(&m.detail))).collect();
        let ds: Vec<String> = degraded.iter().map(|d| esc(d)).collect();
        let ns: Vec<String> = notes.iter().map(|d| esc(d)).collect();
        let js = match &journals {
            Some((state, live, dead, other)) => format!("{{\"state\":{},\"live\":{live},\"dead\":{dead},\"other_boots\":{other}}}", esc(&state.display().to_string())),
            None => "null".into(),
        };
        format!(
            "{{\"v\":1,\"version\":{},\"platform\":{},\"mechanisms\":[{}],\"degraded\":[{}],\"notes\":[{}],\"journals\":{},\"grant_documents\":{}}}",
            esc(env!("CARGO_PKG_VERSION")),
            esc(std::env::consts::OS),
            ms.join(","),
            ds.join(","),
            ns.join(","),
            js,
            grant.map_or("null".into(), |g| g.to_string())
        )
    } else {
        let mut s = format!("sheepr {} on {}\n", env!("CARGO_PKG_VERSION"), std::env::consts::OS);
        for m in &mechanisms {
            s.push_str(&format!("  {} {}: {}\n", if m.ok { "ok  " } else { "FAIL" }, m.name, m.detail));
        }
        for n in &notes {
            s.push_str(&format!("  note {n}\n"));
        }
        match &journals {
            Some((state, live, dead, other)) => s.push_str(&format!("  state {}: {live} live job(s), {dead} dead (sheepr sweep ends them), {other} of other boots\n", state.display())),
            None => s.push_str("  state: none (no journals are kept)\n"),
        }
        if let Some(g) = grant {
            s.push_str(&format!("  privacy: a disclaimed job {} read ~/Documents\n", if g { "can" } else { "cannot" }));
        }
        if degraded.is_empty() {
            s.push_str("sheepr: nothing is degraded.");
        } else {
            s.push_str(&format!("sheepr: degraded: {}", degraded.join("; ")));
        }
        s
    };
    let _ = writeln!(std::io::stdout(), "{out}");
    0
}

/// (state dir, live jobs, dead jobs of this boot, journals of other boots or pid namespaces).
fn journals() -> Option<(std::path::PathBuf, usize, usize, usize)> {
    let state = crate::state::resolve(cfg!(debug_assertions), |k| std::env::var_os(k), |p| p.exists())?;
    let here = crate::sweep::folder(&state);
    let (mut live, mut dead, mut other) = (0, 0, 0);
    for b in std::fs::read_dir(state.join("jobs")).into_iter().flatten().flatten() {
        let this = here.as_ref().is_some_and(|h| *h == b.path());
        for f in std::fs::read_dir(b.path()).into_iter().flatten().flatten() {
            if f.path().extension().map_or(true, |x| x != "journal") {
                continue;
            }
            if !this {
                other += 1;
            } else if crate::sweep::read_header(&f.path()).is_some_and(|(_, sup)| same(sup.0, sup.1)) {
                live += 1;
            } else {
                dead += 1;
            }
        }
    }
    Some((state, live, dead, other))
}

pub(crate) fn mechanism(name: &'static str, ok: bool, detail: impl Into<String>) -> Mechanism {
    Mechanism { name, ok, detail: detail.into() }
}

pub(crate) type Mech = Mechanism;
