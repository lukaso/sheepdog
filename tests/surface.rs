//! Phase-2 P9: the rest of the CLI surface (PLAN.md §10.5): the help screen, `sheepdog help
//! <command>`, a non-subcommand as a usage error, exit 2 for every usage error but `run`'s (125),
//! `"v": 1` on every JSON line and JSON errors `{"v":1,"error":{"code","message","fix"}}`.

mod common;

use common::json::{self, Json};
use std::process::{Command, Stdio};

fn sheepdog() -> &'static str {
    common::test_env();
    env!("CARGO_BIN_EXE_sheepdog")
}

struct Out {
    code: Option<i32>,
    out: String,
    err: String,
}

fn sd(args: &[&str]) -> Out {
    let o = Command::new(sheepdog()).args(args).stdin(Stdio::null()).output().unwrap();
    Out { code: o.status.code(), out: String::from_utf8_lossy(&o.stdout).into_owned(), err: String::from_utf8_lossy(&o.stderr).into_owned() }
}

const COMMANDS: [&str; 6] = ["run", "kill", "strays", "ps", "sweep", "doctor"];

/// `--help` (and `help`) shows the screen on stdout and exits 0; with no arguments it is shown
/// on stderr as a usage error (exit 2). The screen names every command.
#[test]
fn the_help_screen_names_every_command() {
    for args in [&["--help"][..], &["help"][..]] {
        let o = sd(args);
        assert_eq!(o.code, Some(0), "{args:?}: {}", o.err);
        for c in COMMANDS {
            assert!(o.out.split(|ch: char| !ch.is_ascii_alphanumeric()).any(|w| w == c), "{args:?}: the screen does not name {c}:\n{}", o.out);
        }
    }
    let bare = sd(&[]);
    assert_eq!(bare.code, Some(2));
    assert!(bare.err.contains("sheepdog run"), "no screen for a bare call:\n{}", bare.err);
}

/// `sheepdog help <command>` shows that command's usage (exit 0); an unknown one is a usage
/// error (2).
#[test]
fn help_for_each_command() {
    for c in COMMANDS {
        let o = sd(&["help", c]);
        assert_eq!(o.code, Some(0), "help {c}: {}", o.err);
        assert!(o.out.contains(&format!("sheepdog {c}")), "help {c}:\n{}", o.out);
    }
    assert_eq!(sd(&["help", "bogus"]).code, Some(2));
}

/// A command that is not a sheepdog subcommand is a usage error (2) that shows the fix built
/// from what was typed, and it does not run.
#[test]
fn a_non_subcommand_is_a_usage_error_that_shows_the_fix() {
    let d = std::env::temp_dir().join(format!("sd-surface-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let f = d.join("ran");
    let o = sd(&["/usr/bin/touch", f.to_str().unwrap()]);
    assert_eq!(o.code, Some(2), "{}", o.err);
    assert!(!f.exists(), "the command ran");
    assert!(o.err.contains(&format!("sheepdog run -- /usr/bin/touch {}", f.display())), "no fix:\n{}", o.err);
    let _ = std::fs::remove_dir_all(&d);
}

/// Usage errors exit 2 for every subcommand but `run`, which exits 125 (its command's own 2
/// passes through).
#[test]
fn usage_errors_exit_2_except_run_125() {
    for args in [&["ps", "--bogus"][..], &["kill"][..], &["kill", "--bogus", "1"][..], &["strays", "--bogus"][..], &["sweep", "--bogus"][..], &["doctor", "--bogus"][..], &["help", "bogus"][..]] {
        assert_eq!(sd(args).code, Some(2), "{args:?}");
    }
    assert_eq!(sd(&["run", "--bogus", "--", "/usr/bin/true"]).code, Some(125));
}

/// Every `--json` line carries `"v": 1` and parses; an error under `--json` is one line
/// `{"v":1,"error":{"code","message","fix"}}` on stdout.
#[test]
fn json_lines_carry_v1_and_errors_are_json() {
    let lines_ok = |o: &Out, what: &str| {
        assert!(!o.out.trim().is_empty(), "{what}: no output ({:?}): {}", o.code, o.err);
        for l in o.out.lines() {
            let j = json::parse(l).unwrap_or_else(|e| panic!("{what}: not JSON ({e:?}): {l}"));
            assert_eq!(j.get("v").and_then(Json::num), Some(1.0), "{what}: no \"v\":1 in {l}");
        }
    };
    let mut c = Command::new("/bin/sleep").arg("30").spawn().unwrap();
    let pid = c.id().to_string();
    lines_ok(&sd(&["ps", "--json", &pid]), "ps");
    lines_ok(&sd(&["kill", "--dry-run", "--json", &pid]), "kill --dry-run");
    lines_ok(&sd(&["doctor", "--json"]), "doctor");
    common::send_child(&mut c, libc::SIGKILL);
    let _ = c.wait();
    for (args, code) in [(&["ps", "--json", "1"][..], "refused"), (&["strays", "--json", "--kill"][..], "usage"), (&["doctor", "--json", "--bogus"][..], "usage"), (&["kill", "--json"][..], "usage")] {
        let o = sd(args);
        assert_ne!(o.code, Some(0), "{args:?}");
        lines_ok(&o, &format!("{args:?}"));
        let j = json::parse(o.out.lines().last().unwrap()).unwrap();
        let e = j.get("error").unwrap_or_else(|| panic!("{args:?}: no error object: {}", o.out));
        assert_eq!(e.get("code").and_then(Json::str), Some(code), "{args:?}: {}", o.out);
        assert!(e.get("message").and_then(Json::str).is_some_and(|m| !m.is_empty()), "{args:?}: {}", o.out);
        assert!(e.get("fix").and_then(Json::str).is_some(), "{args:?}: {}", o.out);
    }
}

/// `--version`: the version, the commit, the platform and (macOS) whether the responsibility API
/// is active.
#[test]
fn version_names_version_commit_and_platform() {
    let o = sd(&["--version"]);
    assert_eq!(o.code, Some(0));
    let line = o.out.trim();
    assert!(line.starts_with(&format!("sheepdog {} ", env!("CARGO_PKG_VERSION"))), "{line}");
    assert!(line.contains(std::env::consts::OS), "no platform: {line}");
}

/// The durations the help screen shows parse: `run --timeout 5m` runs its command (exit 0, not
/// the usage error 125); `strays --older-than 1h` and `2d` list (exit 0). The controls: an
/// unknown unit is `run`'s usage error (125) and `strays`' (2).
#[test]
fn the_help_durations_parse() {
    assert_eq!(sd(&["run", "--timeout", "5m", "--", "/usr/bin/true"]).code, Some(0));
    assert_eq!(sd(&["run", "--timeout", "5x", "--", "/usr/bin/true"]).code, Some(125));
    let strays = |v: &str| Command::new(sheepdog()).args(["strays", "--older-than", v]).env("SHEEPDOG_TEST_INERT", "1").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().unwrap().code();
    assert_eq!(strays("1h"), Some(0));
    assert_eq!(strays("2d"), Some(0));
    assert_eq!(strays("5x"), Some(2));
}

/// The JSON error message is the error itself, recorded where it happens, not the last line
/// said: `kill --json` with no target prints kill's usage and then ps's; the message is kill's.
#[test]
fn the_json_error_message_is_the_error_line() {
    let o = sd(&["kill", "--json"]);
    assert_eq!(o.code, Some(2));
    let j = json::parse(o.out.lines().last().unwrap_or("")).unwrap_or_else(|e| panic!("not JSON ({e:?}): {}", o.out));
    let m = j.get("error").and_then(|e| e.get("message")).and_then(Json::str).unwrap_or("").to_string();
    assert!(m.starts_with("usage: sheepdog kill"), "message: {m:?}");
}

/// A call that starts with `--` or an option is a usage error whose fix puts `run` in front
/// and keeps what was typed runnable: `sheepdog -- /usr/bin/true` suggests
/// `sheepdog run -- /usr/bin/true` (never `run -- --`), and the suggestion, run, works.
#[test]
fn a_flag_first_call_suggests_a_fix_that_runs() {
    for (typed, fix) in [
        (&["--", "/usr/bin/true"][..], "sheepdog run -- /usr/bin/true"),
        (&["--timeout", "5m", "--", "/usr/bin/true"][..], "sheepdog run --timeout 5m -- /usr/bin/true"),
    ] {
        let o = sd(typed);
        assert_eq!(o.code, Some(2), "{typed:?}");
        let line = o.err.lines().find(|l| l.contains("sheepdog run")).unwrap_or_else(|| panic!("{typed:?}: no fix:\n{}", o.err));
        assert!(line.ends_with(fix), "{typed:?}: {line}");
        let args: Vec<&str> = fix.split(' ').skip(1).collect();
        assert_eq!(sd(&args).code, Some(0), "the suggested fix does not run: {fix}");
    }
}
