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

fn fixture() -> &'static str {
    common::test_env();
    env!("CARGO_BIN_EXE_sd-fixture")
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
    assert_eq!(sd(&["run", "--timeout", "5m", "--", "true"]).code, Some(0));
    assert_eq!(sd(&["run", "--timeout", "5x", "--", "true"]).code, Some(125));
    let strays = |v: &str| Command::new(sheepdog()).args(["strays", "--older-than", v]).env("SHEEPDOG_TEST_INERT", "1").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().unwrap().code();
    assert_eq!(strays("1h"), Some(0));
    assert_eq!(strays("2d"), Some(0));
    assert_eq!(strays("5x"), Some(2));
}

/// The JSON error message is the error itself, recorded where it happens, not the last line
/// said: `kill --json` with no target says so, then prints kill's usage and ps's; the message is
/// the first line, kill's.
#[test]
fn the_json_error_message_is_the_error_line() {
    let o = sd(&["kill", "--json"]);
    assert_eq!(o.code, Some(2));
    let j = json::parse(o.out.lines().last().unwrap_or("")).unwrap_or_else(|e| panic!("not JSON ({e:?}): {}", o.out));
    let m = j.get("error").and_then(|e| e.get("message")).and_then(Json::str).unwrap_or("").to_string();
    assert!(m.starts_with("sheepdog: kill: no target"), "message: {m:?}");
}

/// A call that starts with `--` or an option is a usage error whose fix puts `run` in front
/// and keeps what was typed runnable: `sheepdog -- true` suggests `sheepdog run -- true` (never
/// `run -- --`), and the suggestion, run, works.
#[test]
fn a_flag_first_call_suggests_a_fix_that_runs() {
    for (typed, fix) in [
        (&["--", "true"][..], "sheepdog run -- true"),
        (&["--timeout", "5m", "--", "true"][..], "sheepdog run --timeout 5m -- true"),
    ] {
        let o = sd(typed);
        assert_eq!(o.code, Some(2), "{typed:?}");
        let line = o.err.lines().find(|l| l.contains("sheepdog run")).unwrap_or_else(|| panic!("{typed:?}: no fix:\n{}", o.err));
        assert!(line.ends_with(fix), "{typed:?}: {line}");
        let args: Vec<&str> = fix.split(' ').skip(1).collect();
        assert_eq!(sd(&args).code, Some(0), "the suggested fix does not run: {fix}");
    }
}

/// JSON mode is the parsed `--json` flag, not the word anywhere in the arguments: in
/// `strays --cmd --json --bogus` it is `--cmd`'s pattern, so the usage error is text only
/// (stdout empty); the control, `strays --json --bogus`, writes the JSON error.
#[test]
fn json_mode_is_the_parsed_flag() {
    let o = sd(&["strays", "--cmd", "--json", "--bogus"]);
    assert_eq!(o.code, Some(2));
    assert!(o.out.trim().is_empty(), "a JSON error in text mode: {}", o.out);
    let c = sd(&["strays", "--json", "--bogus"]);
    assert_eq!(c.code, Some(2));
    assert!(json::parse(c.out.trim()).is_ok_and(|j| j.get("error").is_some()), "control: no JSON error: {}", c.out);
}

/// JSON mode does not depend on where `--json` stands: a usage error before it still writes the
/// JSON error (`kill --bogus --json`, `strays --older-than 5x --json`, `doctor --bogus --json`,
/// and `ps --json --include-suspects 5`, which ps refuses before it parses). The control: as
/// another option's value (`strays --cmd --json --bogus`) it is not the flag.
#[test]
fn json_mode_does_not_depend_on_argument_order() {
    for args in [&["kill", "--bogus", "--json"][..], &["strays", "--older-than", "5x", "--json"][..], &["doctor", "--bogus", "--json"][..], &["ps", "--json", "--include-suspects", "5"][..]] {
        let o = sd(args);
        assert_eq!(o.code, Some(2), "{args:?}");
        let j = json::parse(o.out.trim()).unwrap_or_else(|e| panic!("{args:?}: no JSON error ({e:?}): {:?}", o.out));
        assert_eq!(j.get("error").and_then(|e| e.get("code")).and_then(Json::str), Some("usage"), "{args:?}");
    }
    assert!(sd(&["strays", "--cmd", "--json", "--bogus"]).out.trim().is_empty(), "control: --cmd's value");
}

/// A `ps` usage error's JSON message names ps, from its parse and from its refusal of kill's own
/// options; the control, `kill`'s, names kill.
#[test]
fn a_ps_usage_error_names_ps() {
    // ps refuses kill's own options itself, before its parse
    for (args, want) in [(&["ps", "--json"][..], "sheepdog: ps: "), (&["ps", "--json", "--grace", "1", "5"][..], "sheepdog: ps: "), (&["kill", "--json"][..], "sheepdog: kill: ")] {
        let sub = args[0];
        let o = sd(args);
        let j = json::parse(o.out.trim()).unwrap_or_else(|e| panic!("{sub}: ({e:?}) {:?}", o.out));
        let m = j.get("error").and_then(|e| e.get("message")).and_then(Json::str).unwrap_or("").to_string();
        assert!(m.starts_with(want), "{sub}: {m:?}");
    }
}

/// The suggested fix, pasted into a shell, runs the command that was typed: arguments with
/// spaces or shell characters are quoted (`sheepdog -- sh -c 'exit 3'` suggests a line that
/// exits 3 through `sh -c`); with no command after `--` it names COMMAND; a subcommand typed
/// after an option is suggested as that subcommand.
#[test]
fn the_suggested_fix_runs_in_a_shell() {
    let fix_of = |typed: &[&str]| -> String {
        let o = sd(typed);
        assert_eq!(o.code, Some(2), "{typed:?}");
        let l = o.err.lines().find(|l| l.contains("To run it under sheepdog:")).unwrap_or_else(|| panic!("{typed:?}: {}", o.err)).to_string();
        l.split("To run it under sheepdog: ").nth(1).unwrap().to_string()
    };
    let sh = |fix: &str| -> Option<i32> {
        let line = fix.replacen("sheepdog", sheepdog(), 1);
        Command::new("/bin/sh").args(["-c", &line]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().unwrap().code()
    };
    assert_eq!(sh(&fix_of(&["--", "sh", "-c", "exit 3"])), Some(3), "the fix does not run the typed command");
    assert_eq!(sh(&fix_of(&["--", "sh", "-c", "exit 4 # it's"])), Some(4), "a quote inside an argument");
    assert_eq!(sh(&fix_of(&["--", "true"])), Some(0), "control");
    assert!(fix_of(&["--"]).ends_with("-- COMMAND"), "{}", fix_of(&["--"]));
    assert!(fix_of(&["--json", "kill", "5"]).starts_with("sheepdog kill "), "{}", fix_of(&["--json", "kill", "5"]));
}

/// Each duration flag has its own cap: `--kill-deadline` and `kill --grace` at most one day,
/// `--timeout` at most 365 days (controls: one day, 365 days).
#[test]
fn each_duration_flag_keeps_its_cap() {
    assert_eq!(sd(&["run", "--kill-deadline", "86401", "--", "true"]).code, Some(125));
    assert_eq!(sd(&["run", "--kill-deadline", "1d", "--", "true"]).code, Some(0));
    assert_eq!(sd(&["run", "--timeout", "366d", "--", "true"]).code, Some(125));
    assert_eq!(sd(&["run", "--timeout", "365d", "--", "true"]).code, Some(0));
    assert_eq!(sd(&["kill", "--grace", "2d", "1"]).code, Some(2));
    let mut c = Command::new(fixture()).arg("sigcount").arg(std::env::temp_dir().join(format!("sd-cap-{}", std::process::id()))).spawn().unwrap();
    let ok = sd(&["kill", "--dry-run", "--grace", "1d", &c.id().to_string()]).code;
    common::send_child(&mut c, libc::SIGKILL);
    let _ = c.wait();
    assert_eq!(ok, Some(0), "control: kill --grace 1d");
}

/// `sweep --owner` takes a value: in `sweep --owner --json --bogus` the word is the owner, not the
/// flag, so the usage error is text; the control, `sweep --json --bogus`, is JSON.
#[test]
fn sweep_owner_takes_a_value_not_the_json_flag() {
    assert!(sd(&["sweep", "--owner", "--json", "--bogus"]).out.trim().is_empty(), "--owner's value taken as --json");
    let c = sd(&["sweep", "--json", "--bogus"]);
    assert!(json::parse(c.out.trim()).is_ok_and(|j| j.get("error").is_some()), "control: {}", c.out);
}

/// A word that starts with `=` or `~` is quoted in the suggested fix (zsh expands `=ls` to a path,
/// a shell expands `~`): the fix, run through `sh -c` (and `zsh -fc` where there is one), prints
/// the words as typed. An argument that is not printable text gets no runnable line (COMMAND);
/// the control, printable words, stays runnable.
#[test]
fn the_suggested_fix_keeps_every_word_as_typed() {
    let fix_of = |typed: &[&std::ffi::OsStr]| -> String {
        let o = Command::new(sheepdog()).args(typed).stdin(Stdio::null()).output().unwrap();
        let err = String::from_utf8_lossy(&o.stderr).into_owned();
        let l = err.lines().find(|l| l.contains("To run it under sheepdog:")).unwrap_or_else(|| panic!("{typed:?}: {err}")).to_string();
        l.split("To run it under sheepdog: ").nth(1).unwrap().to_string()
    };
    let os = |v: &[&str]| -> Vec<std::ffi::OsString> { v.iter().map(|s| s.into()).collect() };
    let words = os(&["--", "printf", "%s|", "=ls", "~", "~/x", "plain"]);
    let fix = fix_of(&words.iter().map(|w| w.as_os_str()).collect::<Vec<_>>());
    let line = fix.replacen("sheepdog", sheepdog(), 1);
    for shell in ["/bin/sh", "/bin/zsh"] {
        if !std::path::Path::new(shell).exists() {
            continue;
        }
        let flag = if shell.ends_with("zsh") { "-fc" } else { "-c" };
        let o = Command::new(shell).args([flag, &line]).stdin(Stdio::null()).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&o.stdout), "=ls|~|~/x|plain|", "{shell}: {fix}");
    }
    use std::os::unix::ffi::OsStrExt;
    let nl = std::ffi::OsStr::new("a\nb");
    let bad = std::ffi::OsStr::from_bytes(b"\xff");
    for w in [nl, bad] {
        let f = fix_of(&[std::ffi::OsStr::new("--"), std::ffi::OsStr::new("printf"), w]);
        assert!(f.ends_with("-- COMMAND"), "{w:?}: a runnable line for a word it cannot show: {f}");
    }
}

/// A `run` usage error names what it rejected, with the rule it broke, before the usage line
/// (exit 125): a value with its grammar, an option with no value, an unknown option, no command.
#[test]
fn run_usage_errors_name_the_word_and_its_rule() {
    for (args, want) in [
        (&["run", "--max-mem", "2GB", "--", "/usr/bin/true"][..], &["--max-mem 2GB", "K, M or G"][..]),
        (&["run", "--max-mem", "0", "--", "/usr/bin/true"], &["--max-mem 0", "above 0"]),
        (&["run", "--timeout", "5min", "--", "/usr/bin/true"], &["--timeout 5min", "s, m, h or d", "bare number is seconds", "whole number with ms"]),
        (&["run", "--timeout", "0", "--", "/usr/bin/true"], &["--timeout 0", "above 0", "365d"]),
        (&["run", "--kill-deadline", "2d", "--", "/usr/bin/true"], &["--kill-deadline 2d", "1d"]),
        (&["run", "--grace", "2d", "--", "/usr/bin/true"], &["--grace 2d", "1d"]),
        (&["run", "--max-procs", "0", "--", "/usr/bin/true"], &["--max-procs 0", "whole number above 0"]),
        (&["run", "--status-fd", "1", "--", "/usr/bin/true"], &["--status-fd 1", "3 or more"]),
        (&["run", "--timout", "5m", "--", "/usr/bin/true"], &["unknown option --timout"]),
        (&["run", "--timeout", "--", "/usr/bin/true"], &["--timeout needs a value"]),
        (&["run", "", "--", "/usr/bin/true"], &["''"]),
        // the rule a message gives is the parser's: bytes and k pass, ms takes whole numbers
        (&["run", "--max-mem", "2GB", "--", "/usr/bin/true"], &["in bytes or with K, M or G"]),
        (&["run", "--timeout", "1.5ms", "--", "/usr/bin/true"], &["whole number with ms"]),
        (&["run", "--timeout", "5m", "--"], &["no command after --"]),
    ] {
        let o = sd(args);
        assert_eq!(o.code, Some(125), "{args:?}: {}", o.err);
        let first = o.err.lines().next().unwrap_or("");
        assert!(first.starts_with("sheepdog: run: "), "{args:?}: the first line does not start with sheepdog: run::\n{}", o.err);
        for w in want {
            assert!(first.contains(w), "{args:?}: the first line does not say {w:?}:\n{}", o.err);
        }
        assert!(o.err.contains("usage: sheepdog run"), "{args:?}: no usage line:\n{}", o.err);
    }
}

/// `run` with no `--` runs nothing and prints the corrected command, shell-quoted, so the line
/// pasted runs what was meant: the options stay in front, `--` goes before the first word that is
/// not an option. A bad value is named first; no command at all shows the shape.
#[test]
fn run_without_the_separator_prints_the_corrected_command() {
    let d = std::env::temp_dir().join(format!("sd-sep-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let f = d.join("ran");
    let fs = f.to_str().unwrap();
    let o = sd(&["run", "--timeout", "5m", "--quiet", "/usr/bin/touch", fs]);
    assert_eq!(o.code, Some(125), "{}", o.err);
    assert!(!f.exists(), "the command ran without --");
    let o2 = sd(&["run", "/usr/bin/touch", fs, "--", "x"]);
    assert_eq!(o2.code, Some(125), "{}", o2.err);
    assert!(!f.exists(), "the command ran with its own -- taken for sheepdog's");
    let dash = sd(&["run", "\u{2014}", "/usr/bin/touch", fs]);
    assert!(!f.exists() && dash.err.lines().next().unwrap_or("").contains('\u{2014}'), "an em dash: not named, or the command ran:\n{}", dash.err);
    assert!(o.err.contains(&format!("sheepdog run --timeout 5m --quiet -- /usr/bin/touch {fs}")), "no corrected command:\n{}", o.err);
    for (args, want) in [
        (&["run", "npm", "test"][..], "sheepdog run -- npm test"),
        (&["run", "--timeout", "5m", "sh", "-c", "echo a b"], "sheepdog run --timeout 5m -- sh -c 'echo a b'"),
        (&["run", "--timeout", "5m"], "sheepdog run --timeout 5m -- COMMAND"),
        (&["run", "--max-mem", "2GB", "npm", "test"], "--max-mem 2GB"),
        // the command's own `--` is the command's: sheepdog's goes before the first word that is not an option
        (&["run", "npm", "test", "--", "--watch"], "sheepdog run -- npm test -- --watch"),
        (&["run", "--timeout", "5m", "cargo", "test", "--", "--nocapture"], "sheepdog run --timeout 5m -- cargo test -- --nocapture"),
        (&["run", "npm", "--", "test"], "sheepdog run -- npm -- test"),
        // an editor's dash in place of `--` is named, and the line uses two hyphens
        (&["run", "--timeout", "5m", "\u{2014}", "npm", "test"], "sheepdog run --timeout 5m -- npm test"),
        // a word with `=` is quoted (zsh's magic_equal_subst would expand `a==ls`)
        (&["run", "x", "a==ls"], "sheepdog run -- x 'a==ls'"),
    ] {
        let o = sd(args);
        assert_eq!(o.code, Some(125), "{args:?}: {}", o.err);
        assert!(o.err.lines().next().unwrap_or("").contains(want), "{args:?}: the first line does not say {want:?}:\n{}", o.err);
    }
    let _ = std::fs::remove_dir_all(&d);
}

/// Every other subcommand's usage error (exit 2) names the command typed and what it rejected,
/// with the rule it broke, on its first line, then its usage line. The first line keeps the
/// `sheepdog:` prefix every stderr message has (a stable interface): `sheepdog: kill: ...`.
#[test]
fn subcommand_usage_errors_name_the_word_and_its_rule() {
    for (args, want) in [
        (&["kill", "12ab"][..], &["12ab", "PID, PID:ID or j-JOBID"][..]),
        (&["kill"], &["no target"]),
        (&["kill", "5", "6"], &["one target", "6"]),
        (&["kill", "--grace", "2d", "5"], &["--grace 2d", "1d"]),
        (&["kill", "--grace"], &["--grace needs a value"]),
        (&["kill", "--bogus", "5"], &["unknown option --bogus"]),
        (&["ps", "12ab"], &["12ab", "PID, PID:ID or j-JOBID"]),
        (&["ps", "--grace", "1", "5"], &["--grace", "kill", "signals nothing"]),
        (&["strays", "--pid", "1247"], &["--pid 1247", "PID:ID", "--json"]),
        (&["strays", "--min-mem", "2GB"], &["--min-mem 2GB", "K, M or G"]),
        (&["strays", "--older-than", "5x"], &["--older-than 5x", "s, m, h or d"]),
        (&["strays", "--cmd"], &["--cmd needs a value"]),
        (&["strays", "--bogus"], &["unknown option --bogus"]),
        (&["sweep", "--owner"], &["--owner needs a value"]),
        (&["sweep", "--bogus"], &["unknown option --bogus"]),
        (&["doctor", "--bogus"], &["unknown option --bogus"]),
        (&["doctor", ""], &["''"]),
        (&["kill", ""], &["''", "not a target"]),
        (&["kill", "j-abc"], &["j-abc", "j- and 4 to 8 hex digits"]),
        (&["strays", "1234"], &["unexpected argument 1234"]),
        (&["sweep", "foo"], &["unexpected argument foo"]),
        (&["strays", "--kill"], &["--kill needs a filter"]),
        (&["strays", "--cmd", "["], &["--cmd", "not a valid regular expression"]),
        (&["strays", "--cmd", ""], &["empty --cmd"]),
    ] {
        let sub = args[0];
        let o = sd(args);
        assert_eq!(o.code, Some(2), "{args:?}: {}", o.err);
        let first = o.err.lines().next().unwrap_or("");
        assert!(first.starts_with(&format!("sheepdog: {sub}: ")), "{args:?}: the first line does not start with sheepdog: {sub}:\n{}", o.err);
        for w in want {
            assert!(first.contains(w), "{args:?}: the first line does not say {w:?}:\n{}", o.err);
        }
        assert!(o.err.contains(&format!("usage: sheepdog {sub}")), "{args:?}: no usage line:\n{}", o.err);
    }
}

/// `--mode` exists so the cells can compare tracking methods (one loses escapees): the help never
/// shows it, so it is outside the stable flags (the README's "the flags `sheepdog help <command>` shows"),
/// and it still parses.
#[test]
fn the_test_only_mode_flag_is_not_in_the_help() {
    let o = sd(&["help", "run"]);
    assert_eq!(o.code, Some(0));
    assert!(o.out.contains("--forward-int-to-root") && !o.out.contains("--mode"), "help run:\n{}", o.out);
    let bad = sd(&["run", "--bogus", "--", "/usr/bin/true"]);
    assert!(!bad.err.contains("--mode"), "a usage error shows --mode:\n{}", bad.err);
    let m = sd(&["run", "--mode", "--", "/usr/bin/true"]);
    assert!(m.err.contains("--mode needs a value"), "--mode no longer parses:\n{}", m.err);
}

/// A word a usage error cannot show as typed (a control character, bytes that are not UTF-8) gets
/// only the corrected command's shape, never a line that would pass other bytes.
#[test]
fn a_word_that_cannot_be_shown_gets_only_the_shape() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    for args in [&[OsStr::new("run"), OsStr::new("a\nb")][..], &[OsStr::new("run"), OsStr::from_bytes(b"\xff")], &[OsStr::new("run"), OsStr::new("--owner"), OsStr::new("\u{1}"), OsStr::new("npm")]] {
        let o = Command::new(sheepdog()).args(args).stdin(Stdio::null()).output().unwrap();
        let err = String::from_utf8_lossy(&o.stderr).into_owned();
        let first = err.lines().next().unwrap_or("");
        assert_eq!(o.status.code(), Some(125), "{args:?}: {err}");
        assert!(first.ends_with("sheepdog run [options] -- COMMAND") && !first.contains("\\x"), "{args:?}: {first}");
    }
}

/// Values the parser takes are within the rule its message gives: a bare size is bytes, `k` is
/// K, and a fraction of a second is written in s.
#[test]
fn the_rules_said_are_the_rules_parsed() {
    for v in ["4096", "512k", "2G"] {
        assert_eq!(sd(&["run", "--max-mem", v, "--", "/usr/bin/true"]).code, Some(0), "--max-mem {v}");
    }
    assert_eq!(sd(&["run", "--timeout", "1.5s", "--", "/usr/bin/true"]).code, Some(0));
    assert_eq!(sd(&["run", "--timeout", "1500ms", "--", "/usr/bin/true"]).code, Some(0));
}

/// `sheepdog help` with a command that does not exist names the word it rejected (exit 2).
#[test]
fn help_for_a_command_that_does_not_exist_names_it() {
    let o = sd(&["help", "frob"]);
    assert_eq!(o.code, Some(2), "{}", o.err);
    let first = o.err.lines().next().unwrap_or("");
    assert!(first.starts_with("sheepdog: ") && first.contains("frob"), "{}", o.err);
}

/// A call that starts with run's options and no `run`: the suggested line puts `--` before the
/// first word that is not a run option or an option's value, so pasted into a shell it runs what
/// was meant. An option run does not have gets no run line; options with no command, the shape.
#[test]
fn an_options_first_call_suggests_the_separator_before_the_command() {
    let o = sd(&["--timeout", "5m", "sh", "-c", "exit 3"]);
    assert_eq!(o.code, Some(2), "{}", o.err);
    let want = "sheepdog run --timeout 5m -- sh -c 'exit 3'";
    assert!(o.err.lines().next().unwrap_or("").ends_with(want), "{}", o.err);
    let line = want.replacen("sheepdog", &format!("'{}'", sheepdog()), 1);
    let st = Command::new("/bin/sh").arg("-c").arg(&line).stderr(Stdio::null()).status().unwrap();
    assert_eq!(st.code(), Some(3), "the suggested line did not run the command: {line}");
    let u = sd(&["--frob", "x"]);
    assert_eq!(u.code, Some(2), "{}", u.err);
    assert!(!u.err.contains("sheepdog run") && u.err.lines().next().unwrap_or("").contains("--frob"), "{}", u.err);
    let n = sd(&["--timeout", "5m"]);
    assert!(n.err.lines().next().unwrap_or("").ends_with("sheepdog run --timeout 5m -- COMMAND"), "{}", n.err);
}

/// Every line sheepdog suggests for a missing `run` or `--`, pasted into a shell, runs the typed
/// command and exits with its code (never 125, a usage error, or 127, a command that is not one):
/// one reading of where run's options end serves `run`'s parser and the top-level suggestion.
#[test]
fn every_suggested_line_runs_the_typed_command() {
    for (args, code) in [
        (&["--quiet", "/usr/bin/true"][..], 0),
        (&["--timeout", "5m", "sh", "-c", "exit 3", "--", "x"], 3),
        (&["\u{2014}", "/usr/bin/true"], 0),
        (&["--timeout", "5m", "\u{2014}", "sh", "-c", "exit 4"], 4),
        (&["run", "\u{2014}", "--", "/usr/bin/true"], 0),
        (&["run", "--quiet", "sh", "-c", "exit 5", "--", "y"], 5),
        (&["--timeout", "5m", "--", "sh", "-c", "exit 6"], 6),
    ] {
        let o = sd(args);
        let first = o.err.lines().next().unwrap_or("").to_string();
        let at = first.find("sheepdog run ").unwrap_or_else(|| panic!("{args:?}: no suggested line: {first}"));
        let line = first[at..].replacen("sheepdog", &format!("'{}'", sheepdog()), 1);
        let st = Command::new("/bin/sh").arg("-c").arg(&line).stderr(Stdio::null()).status().unwrap();
        assert_eq!(st.code(), Some(code), "{args:?}: the suggested line {line} exits {:?}", st.code());
    }
    // an empty word before `--` is never offered as the command
    let e = sd(&["run", "", "--", "/usr/bin/true"]);
    assert!(!e.err.contains("sheepdog run -- ''"), "{}", e.err);
}

/// The command's own `--status-fd` (after the first word that is not an option) is never
/// sheepdog's: nothing is written to that fd; control: sheepdog's own still gets the line.
#[test]
fn status_fd_after_the_command_is_the_commands() {
    use std::os::unix::process::CommandExt;
    let d = std::env::temp_dir().join(format!("sd-sfd-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    for (args, want_line) in [(&["run", "mytool", "--status-fd", "3", "--", "x"][..], false), (&["run", "--status-fd", "3", "mytool"], true)] {
        let f = d.join("fd3");
        let file = std::fs::File::create(&f).unwrap();
        let fd = std::os::unix::io::AsRawFd::as_raw_fd(&file);
        let mut c = Command::new(sheepdog());
        c.args(args).stdin(Stdio::null()).stderr(Stdio::null());
        // fd 3 in the child, open across exec (dup2 onto itself would keep close-on-exec)
        unsafe {
            c.pre_exec(move || {
                let r = if fd == 3 { libc::fcntl(3, libc::F_SETFD, 0) } else { libc::dup2(fd, 3) };
                if r < 0 { return Err(std::io::Error::last_os_error()); }
                Ok(())
            });
        }
        let st = c.status().unwrap();
        assert_eq!(st.code(), Some(125), "{args:?}");
        let got = std::fs::read_to_string(&f).unwrap();
        assert_eq!(!got.is_empty(), want_line, "{args:?}: fd 3 got {got:?}");
    }
    let _ = std::fs::remove_dir_all(&d);
}

/// A command that cannot run is named as a usage error shows a word: '' for an empty one,
/// control characters escaped (never sent raw to the terminal).
#[test]
fn a_command_that_cannot_run_is_shown_escaped() {
    let e = sd(&["run", "--", ""]);
    assert_eq!(e.code, Some(127), "{}", e.err);
    assert!(e.err.contains("cannot run ''"), "{}", e.err);
    let c = sd(&["run", "--", "zz\u{1b}[31mred\nx"]);
    assert_eq!(c.code, Some(127), "{}", c.err);
    assert!(!c.err.contains('\u{1b}') && c.err.contains("\\x1b"), "{:?}", c.err);
}
