# sheepdog: phase 3 build plan ("Ship")

**Status:** draft 7 (2026-09-28). Nothing is built. Review 1 (draft 1, b03ce12): 4 P1, 15 P2, 6 P3. Review 2 (draft 2, 0722bbd): 4 P1, 9 P2, 12 P3. Review 3 (draft 3, e6aac5f): 2 P1, 9 P2, 8 P3. Review 4 (draft 4, b4409ea): 1 P1, 8 P2, 6 P3. Review 5 (draft 5, 8220376): 0 P1, 6 P2, 1 P3. Review 6 (draft 6, 2c7dd1d): 0 P1, 4 P2, 2 P3; one fix declined in favour of a stated limit, one check declined (§6). The rest taken in; §6 maps each finding to where it went.

**Phase-3 scope (PLAN.md §7):** the releases (Linux: two static binaries; macOS: `Sheepdog.app`, universal, Developer ID-signed and notarized; checksums), `install.sh`, a Homebrew cask in `lukaso/tap`, npm `@lukaso/sheepdog` (cell 18), the README with the agent snippet (§10.6), the agent first-use eval (§10.9). Gate: a clean container, a clean Mac user, the distribution cell (PLAN §6: a browser download of the release runs, with Gatekeeper quarantine); a grant survives an upgrade; the grant holds through the PATH symlink (its control: the same bundle is denied when the grant is off); the first-use eval passes; the published release is checked from its public URLs (§5.6).

**Not in phase 3:** adoption (phase 4); the `treeKill()` entry point and the agent hook (TODOS.md); a signed `SHA256SUMS` (D6).

## 0. Decisions

**D1. Signing and notarization run on the operator's Mac, not in GitHub CI (operator, 2026-09-28: "copy chiefofstaff").** This replaces PLAN.md §5.1's CI design.
- **What chiefofstaff does** (read from its tracked files only: `scripts/release.sh`, `apps/desktop/electron-builder.json`, `DESKTOP_DISTRIBUTION.md`; its `apps/desktop/.env` was not opened): the operator runs `scripts/release.sh` on the Mac; it refuses a dirty tree and an existing tag and builds in a fresh detached worktree of the tag; the Developer ID Application certificate comes from the login keychain; electron-builder signs with the hardened runtime and notarizes with `APPLE_ID`, `APPLE_APP_SPECIFIC_PASSWORD` and `APPLE_TEAM_ID` from a gitignored `.env`; the release is made as a draft, then published with `gh`.
- **What sheepdog copies:** the local script, the clean-tree and tag checks, the fresh worktree of the tag, the keychain certificate, the hardened runtime, notarization, a draft release published last.
- **What sheepdog changes:**
  - **No `.env` and no password variable.** The notary credential is a notarytool keychain profile, `sheepdog-notary` (D2). `notarytool submit --keychain-profile sheepdog-notary` reads it from the keychain. So the password is not in a file, an environment, an argument list, a log, or an agent's context.
  - **A new app-specific password, used only by sheepdog** (D2). chiefofstaff's is in plain text in its `.env`, which any process running as the operator can read.
  - **No signing secret exists in GitHub.** No certificate export, no `CSC_LINK`, no repository or environment secret; a fork, a fork PR or a compromised workflow has nothing to reach. §5.1's other rules stay: no `pull_request_target`, no `workflow_run` on fork events, third-party actions pinned by commit SHA, forks build ad-hoc with the `.dev` bundle ID. A CI workflow, if added, runs the test matrix only, with `permissions: contents: read` and no `secrets.` reference.
  - **The real signed build needs a human action at the Mac** (D2 step 3, D3): it uploads a binary to Apple under the operator's identity. The gate is the keychain's "Confirm before allowing access" dialog on the notary item, which only a person at the screen can answer. §1.2's terminal check is only a guard against mistakes (review 3 measured that `script -q /dev/null` with a piped answer passes it).
- **Given up, stated plainly:** **every process that runs as the operator can sign and notarize as the operator.** The login keychain lets `codesign` use the key with no prompt (measured in phase 0), and, until D2 step 3 is done, `notarytool --keychain-profile sheepdog-notary` works for any caller; after it, each use shows a dialog (unmeasured until the operator's first `build --sign`; §S2's per-build check proves it each time). That includes a malicious `build.rs` or proc-macro, an npm `postinstall` anywhere on this Mac, and an agent. It is already true for every build on this Mac. What the release does to limit it: one crate dependency (`libc`); `--locked`; a fresh `CARGO_HOME` per release and per toolchain, filled by that toolchain's own `cargo fetch --locked` (Cargo checks each download against `Cargo.lock`), then `--offline` (the shared `~/.cargo/registry/src` is not re-checked by Cargo, so it is not used); the toolchain and the Linux image pinned (D7). Still trusted, and named: the rustup toolchain, Xcode's `codesign`/`notarytool`, Docker Desktop, and every tool the test legs run as the operator (npm, pnpm, bun, Homebrew). With D2 step 3's dialog none of them can submit to Apple without the operator; each can still sign with the key, whose access list lets `codesign` use it without a prompt (phase 0). The closing option, if ever needed: build as a second macOS user with no signing identity, then sign as the operator.
- **npm:** published by the operator's own `npm publish --access public <file>.tgz` of the files the build made (npm's 2FA prompt). `npm login` writes an `_authToken` line to `~/.npmrc`, a plain file any process running as the operator can read; so §5 step 5 ends with `npm logout`, then a check that counts `//registry.npmjs.org/:_authToken` lines in the file `npm config get userconfig` names (the count only, never the line; tokens for other registries are the operator's business) and requires 0; and after the first publish, each package is set to "require two-factor authentication and disallow tokens". §5.1's OIDC route needs GitHub Actions and is dropped.
- **The Homebrew tap:** pushed with the operator's own git credentials. No token is stored.

**D2. The operator's one-time setup.** In a normal terminal tab (not the agent's `!` prefix, so the hidden prompts work). The agent never runs these and never reads the keychain item.
1. Make a new app-specific password at account.apple.com (Sign-In and Security > App-Specific Passwords), named `sheepdog-notary`.
2. Store it: `xcrun notarytool store-credentials sheepdog-notary --apple-id <your Apple ID> --team-id P7UM972E39`. It asks for the password (hidden) and checks it with Apple before it saves it.
3. In Keychain Access, find the `com.apple.gke.notary.tool` item for `sheepdog-notary`, Access Control tab, and select **"Confirm before allowing access"** (remove `notarytool` from the "always allow" list if it is there). Then every use of the profile, by any process, shows a dialog that the operator must answer at the screen. This is the one gate against an unattended submission (D3) and against a process that reads the item directly. It touches only this item; chiefofstaff's credentials are not changed. **Never click "Always Allow"** in that dialog (it puts `notarytool` back on the list for good), and **repeat this step after any `store-credentials`** (it replaces the item, and a new item gets the default access list). If the Access Control tab is missing, or no dialog appears at the first check below, stop: the fallback is a dedicated keychain file (`store-credentials --keychain ~/Library/Keychains/sheepdog-notary.keychain-db`, with its own password, locked after each build), and this plan is amended before any real build. Record the result in PHASE3.md. **Check:** the operator runs `scripts/release.sh build --sign` for rc.1 and sees the dialog; clicking Deny stops the build before `notarytool submit` (the script stops at the profile check; its output is saved as the recorded "Deny" outcome, S2).

- The team ID `P7UM972E39` is not a secret: Apple writes it into every signed binary.
- The signing identity is in the login keychain: `security find-identity -v -p codesigning` lists one, `Developer ID Application: Lukas Oberhuber (P7UM972E39)` (2026-09-28). `scripts/release.conf` names it by its SHA-1 hash (the hash of a public certificate is not a secret; a renewed certificate makes the name ambiguous, never the hash).

**D3. What the agent may and may not do:**
- It never reads chiefofstaff's `.env`, never runs `store-credentials`, never runs `security find-generic-password`, `security export` or `security dump-keychain`, never runs `notarytool` in any form, and never prints a keychain item.
- It builds and tests everything that runs without the key or the profile: every automatic cell, the dry cells, the unsigned local builds. The dry signing cells call the signing library (`scripts/lib/sign.sh`) directly, with shims and a temp HOME; they never go through `release.sh build --sign` and never use a pty. **A direct call of `sign.sh` outside a dry cell is forbidden** (it can sign with the Developer ID without a prompt) (§1.2's refusal cells are the one place a cell drives `build --sign`, and there it must be refused).
- **It never runs `scripts/release.sh build --sign` or `publish`, and never drives either through a pty** (`script`, `expect`, a `pty.fork()` helper, or a piped confirmation). §1.2's terminal check does not enforce this (review 3 passed it with `script -q /dev/null`); the rule is enforced for the Apple submission by D2 step 3's dialog, and for `publish` by nothing but this rule, because `gh` and `git` work with the operator's credentials in the agent's shell. Stated. It never pushes, never creates the GitHub repo, never publishes to npm, never pushes the tap (§5). The operator runs the signed builds and pastes the output directory's path; the agent then runs the automatic cells against those artifacts.

**D4. Version:** the first release is `v0.1.0`; PLAN.md §10.8's contracts start at `1.0.0` (adoption may still change them). Taste call; the operator may override. The rc tags are `v0.1.0-rc.N`. **Bundle versions are dotted integers** (`plutil -lint` does not check it, measured): `CFBundleShortVersionString` is the tag's `X.Y.Z` (so `0.1.0` for `v0.1.0-rc.N` and `v0.1.0`); `CFBundleVersion` is a build counter kept in `scripts/release.conf`; **the operator raises it by one and commits before tagging**; the build refuses a counter not above the one in the previous tag's `release.conf`, where "previous" is the highest tag below this one under `git -c versionsort.suffix=-rc tag --sort=v:refname` (without the setting, `v0.1.0` sorts before `v0.1.0-rc.1`, measured in review 3); the first tag has no previous, and any counter ≥ 1 passes. A cell checks both strings against `^[0-9]+(\.[0-9]+){0,2}$`.

**D5. Bundle IDs:** `com.lukaso.sheepdog` only for a bundle that `scripts/release.sh build --sign` signs with the Developer ID. Every other bundle (tests, dev, forks, dry runs, unsigned builds) is `com.lukaso.sheepdog.dev` (PLAN.md §4.4: a mismatching build with the release ID switches off the user's grant). `bundle.sh` writes the release ID only when called by the signing path, and that path signs it in the same step (§1.1).

**D6. What the checksum proves.** On macOS, install.sh checks the Developer ID signature, which proves origin. On Linux, `SHA256SUMS` comes from the same release as the binary, so it proves the download is intact, not where it came from. The README and install.sh's header say so. A signed `SHA256SUMS` is a later option. install.sh accepts a `SHEEPDOG_INSTALL_BASE` only if its scheme is `https`, or its scheme is `http` and its host, parsed exactly (no userinfo, host compared whole), is `127.0.0.1`, `::1` or `localhost`. Cells: `http://127.0.0.1.nip.io/`, `http://localhost@evil.example/`, `http://127.0.0.1:80@evil/`, `http://evil.example#@127.0.0.1/`, `http://evil.example?@127.0.0.1/`, `http://evil.example\@127.0.0.1/` and `ftp://127.0.0.1/` are refused (any `@`, `#`, `?` or `\` before the end of the host is refused); `http://127.0.0.1:8123/` and `http://[::1]:8123/` are accepted.

**D7. Toolchains and the Linux build.**
- **One Rust version for every artifact**, pinned in `rust-toolchain.toml` (the version of the pinned image, below). Measured in review 2: the Mac's cargo is 1.75.0 and `rust:1-alpine` is 1.98.1. S1 moves the Mac to the pinned version, and the full matrix runs green on it before any release step. Each artifact's `rustc -V` goes into the build manifest.
- **The Linux images** are `rust:<pinned>-alpine` and `rust:<pinned>-slim`, pinned by digest (the multi-arch **index** digest, so each platform resolves from it) in `scripts/release.conf`, and **the test legs use the same digests** (a floating `rust:1` with a `rust-toolchain.toml` in the mounted tree would make rustup download the pinned toolchain on every leg, or fail offline). `rust-toolchain.toml` has no `targets` line (it would download inside the `--network none` build); targets are added explicitly outside it. `sheepdog-linux-x86_64` is built in the `linux/amd64` image under emulation (as the amd64 test leg does), not by a cross target.
- **`RUSTUP_TOOLCHAIN`** is never passed from the caller (it overrides `rust-toolchain.toml`, measured in review 3). `rustc -V` is recorded by running it in the same environment and directory as that artifact's build, and the manifest check fails unless every recorded version equals the pin. Mutant: pass the caller's `RUSTUP_TOOLCHAIN=<other>` through, where the cell first shows that `<other>` reports a different `rustc -V` (else the mutant cannot go red); the version check goes red.
- **Its `CARGO_HOME`** is its own: fetched by that image (`cargo fetch --locked`, with network), into a fresh host directory mounted at `/sdhome` (never over `/usr/local/cargo`, which holds `cargo` itself, measured), then the build runs with `--network none`, the home mounted read-only, `CARGO_HOME=/sdhome`, `--offline --locked`. Measured in review 2: a home fetched on the host with cargo 1.75 fails in the 1.98 image ("no matching package named libc"); one fetched in the image builds offline and read-only. Cell: the offline build of the repo's real tree (its real `rust-toolchain.toml`) succeeds; control: the same build with an empty home fails.
- **The commit:** in the container the worktree's `.git` names a host path, so the release passes the commit in: `build.rs` uses `SHEEPDOG_COMMIT_OVERRIDE` when it is set (cell: without it, a build outside a repo says `unknown`). A release cell asserts every artifact's `--version` names the tag's commit.
- **`readelf`** is not on macOS (measured); the static check runs in the container.

**D8. The macOS minimum:** `LSMinimumSystemVersion` and `MACOSX_DEPLOYMENT_TARGET` are `12.0`; the cask says `depends_on macos: ">= :monterey"`. The responsibility SPI is measured only on 15 and 27. On 12–14 sheepdog uses the SPI unmeasured and falls back to §4.3's tracking only if its self-check fails. Stated. Cell: `vtool -show-build` reports `minos 12.0` for both slices of every Mac build.

**D9. The npm launcher is a POSIX `sh` script that `exec`s the executable.** Measured in reviews 2 and 3 on node 24.9: a node launcher (`process.execve`) hands the job node's signal state, not the caller's. Node sets every disposition to default at start, then ignores SIGPIPE and SIGXFSZ, and clears the signal mask; libuv also sets `O_NONBLOCK` on a pipe it holds as stdout. So a caller's `nohup`-style ignores (HUP, INT, TERM) and its blocked signals are lost, and no repair inside sheepdog can know them. A `sh` launcher that `exec`s keeps the caller's ignores and mask exactly (measured with `/bin/sh`, bash, dash and zsh), keeps the pid (cell 18), and needs no node version.
- The launcher (`bin/sheepdog`, `#!/bin/sh`) resolves its own real path (a `readlink` loop; `readlink -f` is not on macOS 12), finds the platform package next to it or under its `node_modules` (hoisted and nested layouts), and `exec`s `…/Sheepdog.app/Contents/MacOS/sheepdog "$@"` (Linux: the binary). No platform package: a one-line error naming the missing package, exit 1. The launcher adds no variable. **pnpm's shim exports `NODE_PATH`** (measured in reviews 4–6: every ancestor `node_modules` of the package's real path, then `<PNPM_HOME>/global/<n>/.pnpm/node_modules` as `PNPM_HOME` was spelled, then the caller's value). **Stated limit, not repaired:** a job started through a pnpm-global sheepdog gets that `NODE_PATH`, as a job started through any pnpm-global bin does. Two drafts of a repair were wrong (the prefix's length and spelling depend on the path depth and on symlinks in `PNPM_HOME`), and the cost of the variable is small: a node job may resolve a pnpm-global package it did not declare. The README says so.
- **bun installs the launcher world-writable** (`-rwxrwxrwx`, measured in review 4; npm and pnpm install 0755). Stated as a limit: the README tells users to install with npm or pnpm, and never with bun as root or into a shared prefix. The cell records bun's modes; for npm and pnpm, no installed file in either package is group- or world-writable.
- **`npx`, `pnpm dlx` and `bunx` do not keep the contract** (measured in review 4 for npx: a new pid, a child of npm; the caller's ignores gone; about 20 `npm_*` variables added). The README says: install globally; do not run sheepdog through npx, dlx or bunx, because then TERM to the pid you hold does not reach sheepdog.
- **Cell (cell 23 extended; the npm, pnpm and bun PATH entries):** the root's whole disposition table (signals 1–31), its mask and its whole environment equal a direct run (for pnpm, the environment except `NODE_PATH`, the stated limit), for three callers (default; HUP, INT and TERM ignored; TERM and USR1 blocked), with stdout a pipe and in a pty; `O_NONBLOCK` on fds 0–2 equals a direct run. **Mutant:** a node `process.execve` launcher; red for all three callers.
- Draft 3's `SHEEPDOG_LAUNCHER` repair in sheepdog is dropped.

## 1. Walls for this phase (built first, in S0)

1. **No test runs a release-ID executable that lacks the Developer ID signature.** Phase 0 measured that running a mismatching build with the release ID switched off the operator's real grant; which key tccd uses (the signature identifier or the bundle ID) is not measured, so the door checks both.
   - **One exec door**, one implementation: `scripts/lib/exec-guard.sh` (POSIX sh, sourced by the shell cells and scripts; the Rust cells call it as a subprocess before each exec). A shared fixture table in `tests/fixtures/exec-guard.tsv` lists each fixture and its verdict for each caller (the door allows a `.dev` bundle; install.sh runs the same door and then a separate "must be the release requirement" check, so it refuses one), and every caller's cell runs the whole table.
   - **Before it runs anything**, it resolves the real path (symlinks followed), then reads (a) the file's signature `Identifier` (`/usr/bin/codesign -d`: the door calls `codesign` and `csreq` by absolute path, a stated exception to §1.3's by-name rule, so a shim on `PATH` cannot answer for it; cell: with a lying `codesign` shim first on `PATH`, the door still refuses an ad-hoc release-ID bundle and the recording stays empty), (b) the `CFBundleIdentifier` of any enclosing bundle (walking up from `Contents/MacOS/`), (c) an embedded `__info_plist` section. If any of them is `com.lukaso.sheepdog`, it requires the recorded requirement (below) to hold for the file and for the bundle; otherwise it refuses and the cell fails. A file with none of the three passes (it cannot be matched to the release ID).
   - **The requirement** is a constant in `scripts/release.conf`, written before rc.1: `identifier "com.lukaso.sheepdog" and anchor apple generic and certificate 1[field.1.2.840.113635.100.6.2.6] and certificate leaf[field.1.2.840.113635.100.6.1.13] and certificate leaf[subject.OU] = "P7UM972E39"` (the Developer ID markers measured in review 1 on the operator's chiefofstaff app). After signing, the build takes `codesign -d -r-` of the signed bundle, strips the `designated => ` prefix, and compares it with the constant **canonicalized by `/usr/bin/csreq -r="<constant>" -t`** (the requirement text needs exactly one leading `=` in the option value: measured in reviews 4 and 5, `-r="=<constant>"` fails with "unexpected token: =", and `-r "<constant>"` as a separate argument is read as a file path; `codesign -R="<requirement>"` follows the same rule, and every place in this plan uses these two spellings) (codesign prints `/* exists */` comments and an unquoted OU; review 3 measured that `csreq -t` prints the same canonical form). Any difference fails. Dry cell: the real `csreq` canonicalizes the real constant from `release.conf`, and the codesign shim replays the real `codesign -d -r-` output of the operator's Developer ID app (chiefofstaff) with the identifier changed; the comparison passes, with `csreq` at rc 0 and its output equal to the canonical string; mutants: one changed marker in the constant is refused; draft 4's and draft 5's spellings go red with `csreq`'s own error. **Which `release.conf`:** the one in the tag's worktree, so the build uses the tagged constant.
   - **It checks before each exec**, including cell 18's npm-installed bundle, the S3 install cells, and install.sh's own `--version` run (install.sh cannot source a file when it runs from `curl | sh`, so it carries the same function inline; the shared fixture table runs against install.sh's copy too, so the two cannot differ unseen).
   - **Its cells never exec a forbidden file.** The door takes an exec seam; the refusal cells use a recording seam that writes "would exec <path>". Refused (the recording stays empty): a bare copy, a hardlink and a symlink of an ad-hoc release-ID executable; an ad-hoc release-ID bundle; a linker-signed executable (Identifier not the release ID) inside a bundle whose Info.plist says the release ID; an unsigned executable in such a bundle; a bundle signed with a requirement that lacks the Developer ID markers. Allowed (the control): a `.dev` bundle. Mutant: remove each check in turn; the recording then has the forbidden path. No mutant ever runs a forbidden file.
   - **Where release-ID fixtures live:** only under `/private/tmp/sd-p3-fixtures.XXXXXX` (the one place measured as not indexed by Launch Services, PLAN §4.4), deleted when the cell ends. The signing path deletes an unsigned release-ID bundle on any failure (a trap).
   - **Dry runs exec nothing they build.** The unsigned build calls `bundle.sh` without `--release-id` (D5). The dry signing cells must make a release-ID bundle to test the signing path; they make it only under `/private/tmp/sd-p3-fixtures.*`. Their end state is the door's refusal: the real `/usr/bin/codesign` sees an unsigned release-ID bundle, so `sign.sh` exits non-zero at the `--version` step with a named reason, and the recording is empty. The steps before it are proved by the shim records. The door's codesign path has no override, not even under `SHEEPDOG_TEST_*` (cell: a lying `codesign` shim first on `PATH` and a test variable set; the door still refuses; mutant: a by-name `codesign` in the door puts the path in the recording). No dry cell execs a dry Mac artifact.
2. **The release script cannot sign or publish from a test, a pipe or an agent.**
   - `scripts/release.sh build vX.Y.Z` makes the unsigned local artifacts: the Linux binaries, a `.dev` bundle, the npm tarballs with a `.dev` bundle. It never touches the key or the profile.
   - `scripts/release.sh build --sign vX.Y.Z` makes the signed release; `scripts/release.sh publish vX.Y.Z` publishes. Both refuse when any `SHEEPDOG_TEST_*` variable is set, and when stdin or `/dev/tty` is not a terminal; both read a typed confirmation (the tag) only from `/dev/tty`.
   - **`publish` is a planner plus an executor.** `scripts/release-plan.sh <out-dir> <tag>` is pure: it reads the output directory's manifest and returns the exact `gh` and `git` argv, or refuses. The cells drive it with the test environment set. It refuses: a file other than `SHA256SUMS` whose hash is not in `SHA256SUMS`; a `SHA256SUMS` that does not have exactly four lines (the three artifacts and `install.sh`; it cannot list itself); a missing or an extra file **in the upload set** (exactly the five of §5 step 3; other files in the output directory, such as the notary log, the `.tgz` files and `MANIFEST.json`, are allowed there and never uploaded); a manifest commit different from the tag's commit (`git rev-parse <tag>^{commit}` locally; from `git ls-remote`, given as input, only the peeled `refs/tags/<tag>^{}` line counts for an annotated tag); an absent remote tag; a POST body (for `gh api -X POST repos/lukaso/sheepdog/releases`) whose `draft` is not the JSON boolean `true` (`-F draft=true`; `-f` sends the string, per `gh api --help`), whose `tag_name` is not the tag, or that has a `target_commitish` (the REST API has no verify-tag: with the tag missing, GitHub would make it from the default branch at publish); a manifest marked as a control (`--no-notarize`); another release or draft that already uses the tag (from `gh release list`, given as input). A cell and a mutant for each, including an annotated-tag cell, an empty `ls-remote` cell, and an extra file in the directory (allowed) against one in the upload set (refused). The executor: (1) works out the three facts from the archive itself with the real tools called by absolute path, never from the manifest (unpacks it under `/private/tmp/sd-p3-fixtures.*`, then `/usr/bin/codesign -v -R="<requirement>"`, `/usr/bin/xcrun stapler validate`, `/usr/sbin/spctl -a -t exec`), and refuses if any fails. Cells, through a seam that stops before the terminal check: a dry output directory is refused by codesign (automatic; a lying-shim variant as for the door); and in the `rc` leg, rc.1's output directory is accepted, and the `--no-notarize` control directory (§4: made next to rc.1, deleted after S7) passes codesign and is refused by stapler and spctl; (2) runs the planner's argv behind the terminal check; the draft is made with `gh api -X POST repos/lukaso/sheepdog/releases` (draft true, the tag, `target_commitish` unset so the existing tag is used) and its numeric `.id` is read from the JSON reply (`gh release create` prints only a URL, `…/untagged-<hex>` for a draft); assets are uploaded to that id; the shim's reply fixture is the real API's shape, saved with its source (GitHub's REST docs); (3) checks the draft by that id: the asset names are exactly the five, and each asset's downloaded digest equals the local file's (`SHA256SUMS` included); (4) checks the remote peeled tag again (it is checked before the POST and before the PATCH that publishes); (5) asks again, then makes that release id public.
   - Cells: `build --sign` and `publish` under the test environment are refused; with stdin from a pipe and no controlling terminal (perl: fork, then a checked `POSIX::setsid()`, then exec; macOS has no `setsid`, measured) they are refused; the `gh`, `git`, `codesign` and `xcrun` shims record nothing. A mutant for each.
   - **Stated: this check is a guard against mistakes only.** Review 3 passed it with `(sleep 1; echo v0.1.0) | script -q /dev/null ./release.sh …` and with a `pty.fork()` helper; §1.4's own dry signing cell passes it the same way. The human gate for the Apple submission is D2 step 3's keychain dialog; for `publish` there is none beyond D3's rule. A process that runs as the operator can publish with the operator's `gh` and `git` credentials.
3. **Dry cells cannot reach the real key, the real profile, or the network by accident.**
   - Dry cells run with `HOME` set to a temp dir. Measured in review 1: with a HOME that is not the operator's, `security find-identity -v -p codesigning` finds 0 identities. Each dry cell first runs the real `security find-identity -v -p codesigning` in its own environment and requires 0.
   - Each dry cell also requires, in its own environment: `security list-keychains` does not list the real login keychain; `command -v` finds no real `xcrun`, `notarytool`, `codesign` or `security` on its `PATH` (only the shims). A full-path call is still possible; the shim-record check below turns it red, and the temp HOME leaves it no default keychain.
   - The bound, measured in review 2: naming the real login keychain explicitly (`--keychain <real home>/Library/Keychains/login.keychain-db`) still reaches the key. So the shim records are also checked: no tool's argv holds `--keychain` or a path under the real home's `Library/Keychains`.
   - Shims on `PATH` for every external tool the script calls, except these read-only calls by absolute path, each with its only subcommand: `/usr/bin/codesign -d` and `-v -R=` and `/usr/bin/csreq -r= -t` (the door and the executor), `/usr/bin/xcrun stapler validate` and `/usr/sbin/spctl -a -t exec` (the executor and `npm-check`). Each lives in one named function in `scripts/lib/realtools.sh` that takes only a path, so no caller can pass another subcommand (a full-path `xcrun` is the call shape that could run `notarytool`; review checks that no other full-path call exists — a source-scanning cell is declined under the operator's no-pinned-code rule): `codesign`, `xcrun`, `spctl`, `security`, `ditto`, `lipo`, `vtool`, `cargo`, `docker`, `gh`, `git`, `npm`, `curl`. Each records argv and environment. The script calls every tool by name, never by a full path; each dry cell asserts that the set of shims that recorded equals the set the step needs (mutant: `/usr/bin/codesign` in the script; red on the missing record).
   - The `git` shim passes read-only subcommands (`status`, `rev-parse`, `worktree`, `tag -l`, `ls-remote` answered from a fixture) to the real git on a throwaway repo copy, and records `push` without running it. `cargo` and `docker` shims build nothing; they copy fixture binaries into place. Stated: the dry cells do not prove the real build's network behaviour; D7's offline cell does that.
4. **The secret wall, as behaviour.** From the shims' records:
   - `notarytool` is called with `--keychain-profile sheepdog-notary` and never with `--password`, `--apple-id`, `--key`, `--key-id` or `--issuer` (the signing library's dry cell, called directly with shims and a temp HOME; no pty, no `build --sign`);
   - **each tool gets its own named list, and nothing else:** base = `HOME`, `PATH`, `TMPDIR`, `USER`, `LOGNAME` (+ `DEVELOPER_DIR` if set); `cargo` = base + `CARGO_HOME`, `CARGO_TARGET_DIR`, `MACOSX_DEPLOYMENT_TARGET`, `SHEEPDOG_COMMIT_OVERRIDE`, `RUSTUP_HOME` (never `RUSTUP_TOOLCHAIN`, D7); `git` and `gh` = base + `SSH_AUTH_SOCK`, `GIT_SSH_COMMAND`, `GH_CONFIG_DIR` if set (never `GH_TOKEN`: gh uses its own stored login), with a dry cell over an ssh-remote fixture; `docker` = base + `DOCKER_HOST`, `DOCKER_CONFIG` if set; the rest = base. Cells read the cargo shim's record and require `CARGO_HOME` to be the fresh directory and `MACOSX_DEPLOYMENT_TARGET=12.0`;
   - a decoy `APPLE_APP_SPECIFIC_PASSWORD=decoy-<random>` (and `CSC_KEY_PASSWORD`, `GH_TOKEN`, `NPM_TOKEN`) in the caller's environment reaches no tool, no line of the script's output, and no file under the output directory. Mutant: pass the caller's environment through; the decoy cell goes red.
   - The real password never takes these channels (D1 keeps it in the keychain). This wall guards the channels a later edit could open. The keychain itself is D1's stated limit and D2 step 3's check.
5. **The artifact holds only what it should.**
   - The build refuses a worktree with any untracked or ignored file before it builds.
   - The macOS archive is `tar --no-xattrs --no-mac-metadata --no-acls --uid 0 --gid 0 --uname '' --gname '' -czf` (measured in review 1: the default tar stores `com.apple.provenance` and `com.apple.quarantine` in pax headers `tar -t` does not list; measured in review 2: it stores the operator's user name).
   - The manifest cell extracts each archive into a temp dir and asserts: the list of regular files (bsdtar also lists directory entries; they must be exactly the parents of these files) is exactly `Sheepdog.app/Contents/Info.plist`, `Contents/MacOS/sheepdog`, `Contents/_CodeSignature/CodeResources`, and (signed builds) the staple ticket `Contents/CodeResources`; `xattr -lr` is empty; no `SCHILY.xattr` or `LIBARCHIVE.xattr` pax header; every owner field is `0 0` with empty names. Control: an archive made without the flags from a tree with a planted xattr is refused.
   - **The build manifest** (`MANIFEST.json` in the output dir): the tag, its commit, each file's name, sha256 and `rustc -V`, and whether the build was a control. `SHA256SUMS` lists the four other release files (three artifacts and `install.sh`); the release uploads five.

## 2. Steps (each: write the failing cell, see it red, build, see it green, a mutant per fix)

**S0. The walls of §1, and the bundle builder.**
- Failing cells first: §1.1's door cells and fixture table, §1.2's refusal and planner cells, §1.3's isolation controls, §1.4's secret and allowlist cells, §1.5's manifest cells, all against scripts that do not exist yet.
- `scripts/bundle.sh <binary> <out-dir> <version> <build-number> [--release-id]` writes `Sheepdog.app` with `Info.plist`: `CFBundleIdentifier`, `CFBundleName=Sheepdog`, `CFBundleExecutable=sheepdog`, `CFBundlePackageType=APPL`, `CFBundleInfoDictionaryVersion=6.0`, `CFBundleShortVersionString`, `CFBundleVersion` (D4), `LSMinimumSystemVersion=12.0`, `LSUIElement=true`; and the binary at `Contents/MacOS/sheepdog`. The `.dev` ID unless `--release-id`.
- Cells: `plutil -lint` passes; the ID is `.dev` without the flag (control: with it, in a fixture dir only); both versions match D4's pattern (control: `0.1.0-rc.1` refused); `lipo -archs` is `x86_64 arm64` for a universal input.
- `build.rs` honours `SHEEPDOG_COMMIT_OVERRIDE` (D7), with its cell.
- D9's `sh` launcher and its signal-state cell against a fixture package (the npm, pnpm and bun runs are in S5).

**S1. The pinned toolchain, and the universal Mac binary under the hardened runtime.**
- `rust-toolchain.toml` pins D7's version; `rustup target add x86_64-apple-darwin`. The full `./test-all` matrix runs green on it before anything below.
- `MACOSX_DEPLOYMENT_TARGET=12.0`; `cargo build --release --locked --target aarch64-apple-darwin` and `--target x86_64-apple-darwin`; `lipo -create`; `vtool -show-build` shows `minos 12.0` on both slices (D8).
- **The risk this step retires:** notarization needs the hardened runtime. Measured in review 1 on a debug build: an ad-hoc `-o runtime` `.dev` bundle, run through a symlink, resolves the responsibility API, passes the disclaim self-check, and reports `"tracking":"responsibility","degraded":null`. Still to measure: the escapee cells, and the release build.
- The cell: the macOS integration suite's supervised cells (cells 1–3, 12, 24, the disclaim cells, `doctor`) against a debug build ad-hoc signed with `-o runtime`, bundled with the `.dev` ID. Then the cells that need no test seam against the release universal build, ad-hoc `-o runtime`, `.dev` ID, under the rules below.
- The x86_64 slice runs cells 1 and 3 under Rosetta (`arch -x86_64`). Stated: not run on a real Intel Mac.

**Rules for every cell that runs a release or rc binary** (PHASE2 §0.5, carried over; a release build has no tag latch and no state wall). The one helper that runs a release binary:
- strips every `SHEEPDOG_TEST_*` variable (a release build exits 125 when it sees one);
- sets `HOME`, `XDG_STATE_HOME` and `SHEEPDOG_STATE` to a fresh temp dir; passes `--no-sweep`;
- refuses the `kill`, `sweep` and `strays --kill` subcommands;
- refuses `--inherit-terminal-permissions` unless the cell runs under PHASE2's test-made terminal T with T's preconditions (in inherit mode the responsible process is the terminal app, whose other processes a defect could reach);
- runs no registration listener another test's inner run could reach: `SHEEPDOG_OUTER` is removed, and the cell's release job runs with nothing nested inside it except its own fixtures.
Cells: the helper refuses each forbidden form (a cell per form, a mutant per refusal); every process it builds is killed by the cell's recorded identity.

**S2. `scripts/release.sh`: `build` (unsigned, the agent runs it), `build --sign` (the operator runs it).**
- Both refuse, each with a dry cell (red first): a dirty tree; HEAD is not the tag; the tag's `X.Y.Z` does not match `Cargo.toml`'s version; the build counter is not above the previous tag's (D4). `--sign` also refuses: the identity hash in `release.conf` is not in `security find-identity -v -p codesigning` (run in the named environment, §1.4); the profile check fails.
- **The profile check:** `notarytool history --keychain-profile sheepdog-notary`; exit 0 is present; otherwise the output (written only to the output directory, never echoed) is classified as "profile not found", "credentials rejected" (an HTTP 401 or an authentication error), or "cannot reach Apple" (a network error), and anything else as "unknown, see <file>". The outputs are recorded by the operator during D2 (a Deny in the dialog, a wrong profile name, the network off), each saved with where it came from; until they exist, every failure is "unknown" and stops the build. A dry cell for each recorded outcome.
- **The dialog check, each `build --sign`:** before the profile check, the script prints how many keychain dialogs to expect and in which order (the profile check, the submit, and the log fetch on failure), and after the submit asks on `/dev/tty` whether each appeared. "No" stops the build before stapling and says to redo D2 step 3.
- Builds in a fresh detached worktree of the tag, with its own `CARGO_TARGET_DIR`, and D7's per-toolchain `CARGO_HOME`s.
- **Mac, unsigned:** the S1 universal build, `bundle.sh` (`.dev` ID).
- **Mac, `--sign`:** the S1 universal build, then in one step with a trap that deletes the bundle on any failure: `bundle.sh --release-id`, `codesign --force --options runtime --timestamp -s <hash> Sheepdog.app`, the requirement comparison (§1.1). Then `ditto -c -k --keepParent Sheepdog.app submit.zip`; `xcrun notarytool submit submit.zip --keychain-profile sheepdog-notary --wait --output-format json` (status must be `Accepted`; otherwise `notarytool log <id>` goes to the output directory and the script exits 1 before stapling); `xcrun stapler staple Sheepdog.app`. Checks, each a hard failure: `codesign --verify --strict --deep`; `codesign -v -R` of the recorded requirement; the runtime flag; `spctl --assess --type execute`; `xcrun stapler validate`; then `--version` through the §1.1 door and the release-binary helper names the tag's commit. These steps live in `scripts/lib/sign.sh` (called by `build --sign`, and directly by the dry cells). **`sign.sh` guards itself**, before any `codesign`, `xcrun` or `security` call: it runs in real mode only when `release.sh build --sign` called it after its own refusals (release.sh passes `--real <nonce>` and writes the nonce to a 0600 file it created in its own temp dir; `sign.sh` checks that its parent is that `release.sh` process and that the file holds the nonce); otherwise it runs in dry mode, which refuses unless §1.3's preconditions hold in its environment (0 identities from `security find-identity`, and `codesign`, `xcrun` and `security` resolve to shims). Cells: `sign.sh` called directly with a `security` shim that reports 1 identity is refused, with every shim record empty (mutant: the precondition check removed; the cell records a `codesign -s`, still only the shim); `sign.sh` called directly with a forged `--real <nonce>` and a matching 0600 nonce file, and the same shim, is refused by the parent check, with every record empty (mutant: the parent check removed; the cell records a `codesign -s`, the shim only). The dialog check of §S2 moves into `sign.sh`, so it runs on every real signing. Then the archive (§1.5).
- **Linux:** `sheepdog-linux-aarch64` and `sheepdog-linux-x86_64` (`uname -m` names, as PLAN §10.8's Dockerfile), built per D7. **The static check** (in the container): `readelf -l` shows no `PT_INTERP` (review 1: `file` says `static-pie linked` for x86_64, so its wording cannot be the check), and the binary runs cell 1 in a `FROM scratch` container. Control: the existing glibc build is refused. `--version` names the tag's commit.
- **npm:** the four `.tgz` files, at the tag's version, with exact optional-dependency versions; the darwin one holds the bundle this build made (`.dev` unsigned, or the stapled release). Listed in the manifest (not in `SHA256SUMS`: npm checks its own integrity).
- `SHA256SUMS` over `sheepdog-macos-universal.tar.gz`, the two Linux binaries and `install.sh`; `MANIFEST.json` (§1.5).
- The output directory is outside the repo. Nothing is uploaded.
- **The operator's real runs** (D3), after D2: `v0.1.0-rc.1`, a local tag. They are real Apple submissions; they publish nothing.

**S3. `install.sh`.**
- POSIX sh. Downloads with `curl`, else `wget`, else it stops and names both. Checks with `sha256sum`, else `shasum -a 256`. D6's base-URL rule. It is a release asset, so PLAN §10.8's `releases/latest/download/install.sh` resolves.
- **Its version:** the tag's version is written into it at build time (`SHEEPDOG_VERSION=`). So every install cell and the ship smoke run the copy from the output directory, never the source file.
- Downloads the platform artifact and `SHA256SUMS`, checks the sum, then:
  - **macOS:** unpacks into a temp dir next to the target; checks the §1.1 requirement (its inline copy of the door) and `spctl`; then replaces `~/Applications/Sheepdog.app` by two `mv`s (old aside, new in), then removes the old one. Stated: between the two `mv`s there is a moment with no bundle; a job that starts then fails its re-exec and falls back as PLAN §4.4 says. Links `~/.local/bin/sheepdog` → `~/Applications/Sheepdog.app/Contents/MacOS/sheepdog` (a symlink, never a hardlink, §4.4).
  - **Linux:** installs the binary to `~/.local/bin` (or `/usr/local/bin` as root), through a temp file and `mv`.
- If the link dir is not on `PATH`, it prints the one line that adds it.
- Runs `sheepdog --version` only after every check passed, through its door.
- **Automatic cells** (temp `HOME`):
  - Mac, served by `python3 -m http.server --bind 127.0.0.1`, whose log must show install.sh's GETs: a sum mismatch refuses and installs nothing; D6's URL cells; the §1.1 fixture table run against install.sh's inline door with the recording seam (the ad-hoc release-ID bundle, the linker-signed one in a release-ID bundle, and so on: refused, never run); the `.dev` bundle refused (install.sh accepts only the release requirement). Every Mac cell that needs a successful install (a second install over the first replaces it, and the link still resolves) is in the `rc` leg: install.sh has no test-only acceptance.
  - Linux, the distribution cell's Linux half: the server runs inside the same container on 127.0.0.1, its log checked (busybox `httpd` on Alpine; on Debian a static busybox copied in). Images: `alpine:<pinned>` as is (busybox `wget` only, measured) and `debian:<pinned>-slim` with `curl` added (measured: the slim image has neither tool; stated as "not clean"). Each installs, then runs cell 1 and cell 3 through `scripts/smoke.sh`.
- **After the operator's rc run** (§4): the Mac happy path with the real rc bundle; the link resolves into the bundle; the running supervisor's path (`proc_pidpath` of the pid in `--status-fd`'s start line) is inside `Sheepdog.app`.

**S4. The Homebrew cask.**
- `Casks/sheepdog.rb`: `url` of the macOS archive, `sha256`, `depends_on macos: ">= :monterey"` (D8), `app "Sheepdog.app"`, `binary "#{appdir}/Sheepdog.app/Contents/MacOS/sheepdog"`, `zap trash: "~/.local/state/sheepdog"`, and `caveats` naming `tccutil reset SystemPolicyAllFiles com.lukaso.sheepdog` (uninstall does not remove the grant).
- Cells: `brew style` on the file (no tap needed). `brew audit --cask --strict` and the install run in the clean-user leg (§S7), because they write Homebrew state.

**S5. npm `@lukaso/sheepdog`.**
- **Measured in review 1** (npm 11.6.0): a `bin` in an optional dependency is not linked onto PATH; a main-package `bin` with `../` is dropped; `npm pack` keeps 0755 and sets mtimes to 1985. **Measured in review 2:** pnpm installs by clone on APFS (link count 1) and `proc_pidpath` is inside the bundle; a local install whose optional platform package is not published exits 0 with no platform package; with two tarballs on the command line npm retried the registry for over 60 s.
- **The mechanism:** the esbuild pattern for the payload (`@lukaso/sheepdog` with `optionalDependencies` on `@lukaso/sheepdog-darwin-universal`, `-linux-arm64`, `-linux-x64`, each with `os`/`cpu`; the darwin one holds `Sheepdog.app`; no `postinstall`), and D9's `sh` launcher as the main package's `bin`. The process becomes the executable, same pid (cell 18).
- **bun:** 1.3.11 is already installed (`/opt/homebrew/bin/bun`). Its lack of `process.execve` (measured) does not matter with an `sh` launcher; the cell checks what `bun -g` makes of an `sh` bin.
- **Cell 18** (npm `-g`, pnpm `-g`, bun `-g` if supported, each with `--ignore-scripts`, into temp prefixes):
  - installed from the build's `.tgz` files through a loopback registry that is a **static file server** (`python3 -m http.server --bind 127.0.0.1` over packument JSON files and tarballs that the cell writes; no `npm publish`, no registry software), with a temp `HOME` and `XDG_CONFIG_HOME` (pnpm and bun read global config outside `--userconfig`), **pnpm pinned** (this Mac's `pnpm` is a corepack shim; with a fresh HOME corepack fetches the newest pnpm, measured in review 6: so `COREPACK_HOME` is a copy of the operator's cache holding the pinned version, with `COREPACK_ENABLE_NETWORK=0`; the cell records `pnpm -v` and requires the pin; control: a fresh HOME without the pin shows the download attempt or fails), a fresh cache, and a userconfig whose only registry setting is that URL (a cell checks it holds no token and no non-loopback URL); each installed package's integrity must equal the local tarball's, so it cannot be a public one;
  - the §1.1 door checks the installed bundle **before** the first run;
  - the PATH entry's running process is the bundle executable (`proc_pidpath` of the pid in `--status-fd`'s start line is inside `Sheepdog.app`); TERM to that pid ends the whole tree (cell 3's escapee included);
  - D9's cell (the full disposition table, mask and `O_NONBLOCK` equal to a direct run, three callers, pipe and pty), with its node-launcher mutant;
  - **control (PLAN cell 18's own):** a wrapper that spawns the executable instead of exec'ing it fails the cell (the pid on PATH is the wrapper's);
  - **control:** the platform tarball left out of the registry gives no bundle, and the launcher's error names the package.
- Before the rc: all of this with the `.dev` bundle (the staple not covered). After the operator's rc run: again with the stapled rc bundle.

**S6. README, CHANGELOG, the agent snippet** (PLAN §10.6 order; §10.8's CHANGELOG entry per release). The Dockerfile example uses the shipped version (D4). The README states D6 and the npm limits (D9's cost; bun per S5). Uninstall docs per channel, following S7's index result. A documentation step: reviewed, no test.

**S7. The manual legs (the operator, after the rc runs).**
- **The first read of S7 is the control** (grant off: must be denied). Review 3 read `~/Library/Safari` with rc=0 from the agent's shell (iTerm has Full Disk Access, PLAN §2, or the path is not protected on macOS 27); the control settles which. If it is not denied, the leg picks another Full Disk Access-only path (for example `~/Library/Mail`) and repeats the control.
- **Every protected read** uses a path that only Full Disk Access protects (`~/Library/Safari`, so no Files-and-Folders consent prompt can answer instead), runs as `sheepdog run --timeout 20s --status-fd 3 -- ls ~/Library/Safari`, and requires `"tracking":"responsibility"` and `degraded:null` (otherwise an allowed read may be iTerm's grant, PLAN §2). Before the first read, Files and Folders shows no Sheepdog row; after the last, still none.
- **The grant through the PATH symlink** (from phase 1): install rc.1 with `install.sh`; grant Full Disk Access to `~/Applications/Sheepdog.app`; the read through `~/.local/bin/sheepdog`: allowed. **Control:** the same bundle through the same symlink with Full Disk Access switched off: denied, with no prompt. Then on again: allowed.
- **The grant survives an upgrade:** the operator builds rc.2 (a different binary; the counter and the commit differ), installs over rc.1, the same read: allowed, with the same control.
- **The distribution cell's Mac half** (PLAN §6): download rc.1's `sheepdog-macos-universal.tar.gz` with Safari from a local page (so it gets the quarantine attribute), unpack in Finder, run the CLI through a symlink: it runs, with no Gatekeeper block. **Control:** a Developer ID-signed, not notarized copy, made by `build --sign --no-notarize` next to rc.1 (§4; a mode used only for controls, its directory deleted after this leg), downloaded the same way: Gatekeeper blocks it.
- **A clean Mac user** (a standard user made for this leg, deleted after): Homebrew in that user's own prefix (`~/homebrew`, the untar install; it has no bottles), node from the official tarball unpacked in that home, pnpm by `corepack`. Before and after, a listing of the operator's `/opt/homebrew/Caskroom` and `/opt/homebrew/bin` must match. Then `install.sh`, the cask from a local tap whose `url` points at the local rc archive (stated: not the shipped cask text, which §5 step 6 checks; `brew audit --cask --strict` here, `--appdir=~/Applications`), and npm. Each runs `scripts/smoke.sh`. For each channel it records whether the bundle shows in the Full Disk Access list, and whether `tccutil reset SystemPolicyAllFiles com.lukaso.sheepdog` succeeds (PLAN §4.4's remaining check 3); S6's docs follow the result.
- **`scripts/smoke.sh`** (needs no repo and no cargo): cell 1, and cell 3 with an escapee made by perl (macOS has no `setsid`, measured): it forks first, then calls `POSIX::setsid()` and dies if that returns -1 (a group leader's `setsid` fails silently otherwise, measured in review 3), then writes its pid and start time. The smoke asserts the escapee's session id equals its pid. The smoke requires it alive before the kill (read by that identity), and gone after. **Control:** the same tree without sheepdog leaves it alive (and the smoke kills it by its identity).
- **The shipped build's smoke:** `v0.1.0` is a new binary and a new notarization. Before `publish`, it gets `install.sh` from the output dir, the granted read through the symlink with its control, and `scripts/smoke.sh`.
- **The agent first-use eval** (PLAN §10.9): `eval/hang.sh` starts an escapee (perl: fork, then a checked `setsid`) and writes its identity to a file; `eval/check.sh` passes if that identity is gone. **Control:** `./hang.sh` run alone, then `eval/check.sh`: FAIL. The operator judges the other two rules (the final message states what was killed; wall time under 2 min) and runs it with a bounded token budget.

## 3. The full-matrix legs

The ten legs of `./test-all` stay (on the pinned toolchain from S1). Phase 3 adds:
- `bundle` (Mac, automatic): S0 and S1 cells, the S2 dry cells and planner cells, S3's Mac cells, S5's cells with the `.dev` bundle.
- `dist-linux` (automatic): D7's offline-build cell, S2's static check, S3's Linux cells, and the Linux npm packages: cell 18 and D9's cell with `npm -g` from the static registry in Alpine (busybox ash) and Debian (dash); the running pid is the binary's, and `SigIgn`/`SigBlk` from `/proc` equal a direct run.
- `rc` (Mac; runs on the operator's rc output dir): the manifest cells, S3's Mac happy path, S5's cell 18 with the stapled bundle.
- `grant`, `quarantine`, `clean-user`, `ship-smoke`, `eval`, `post-publish` (manual, the operator): S7 and §5.6.

## 4. Order and checkpoints

S0 → S1 (the toolchain move, then the matrix green) → **review** → S2 unsigned and dry → S3 automatic → S5 with `.dev` → **review** → the operator's D2 → the operator's `build --sign v0.1.0-rc.1` and `build --sign --no-notarize v0.1.0-rc.1` (the control directory, kept until S7's quarantine control has used it) → the `rc` leg (the executor's refusal of the control, named as stapler and spctl, and `npm-check`'s refusal, both recorded) → S4 → S6 → **review** → S7 (the operator; rc.2 inside it) → **final review** → the full matrix → the operator's `build --sign v0.1.0` → the ship smoke → the operator's §5 steps.

## 5. The operator's one-way steps (held; this plan does not do them)

1. D2: the new app-specific password and the notary profile; the access-list check.
2. Create the public GitHub repo `lukaso/sheepdog` and push. First, TODOS.md becomes issues (operator, 2026-09-25).
3. Push the `v0.1.0` tag, then `scripts/release.sh publish v0.1.0`: the planner's checks, a draft release with the five files (the three artifacts, `install.sh`, `SHA256SUMS`), the draft checked to list exactly those five, then published after a second confirmation.
4. Push the cask to `lukaso/tap`.
5. `scripts/release.sh npm-check <out-dir>`, then `npm publish --access public` the four `.tgz` files from the output dir, platform packages first. `npm-check` requires: each `.tgz` sha256 equals its `MANIFEST.json` entry, in a manifest that is not a control; the darwin tarball's bundle, unpacked under `/private/tmp/sd-p3-fixtures.*`, passes the executor's real-tool checks; and its cdhash equals the GitHub archive bundle's. Cells: the unsigned build's directory and the `--no-notarize` directory are refused. After publishing: `npm logout`, then the token count check (D1), and "require 2FA and disallow tokens" on each package.
6. **Post-publish check:** download each published asset and compare it with `SHA256SUMS` and the output dir; run PLAN §10.8's Linux Dockerfile in a clean container; in the clean user, `curl -fsSL …/releases/latest/download/install.sh | sh`, `brew install --cask lukaso/tap/sheepdog` and `npm i -g @lukaso/sheepdog`, each followed by `scripts/smoke.sh`.

## 6. Review findings → where they went

**Review 1 (draft 1):**

| # | Sev | Finding | Where |
|---|---|---|---|
| 1 | P1 | the wall checked the Info.plist ID, not the executable; cell 18 checked after the run | §1.1 |
| 2 | P1 | a dry cell could reach the real key by a full-path call | §1.3 |
| 3 | P1 | the symlink leg's control; iTerm's grant could make it pass | S7 |
| 4 | P1 | the npm design could not put a bin on PATH | S5, D9 |
| 5 | P2 | the requirement lacked the Developer ID markers | §1.1 |
| 6 | P2 | the secret wall guarded no real channel; a reused password in a file | D1, D2, §1.4 |
| 7 | P2 | a cleared environment breaks codesign | §1.4 |
| 8 | P2 | tar carries xattrs | §1.5 |
| 9 | P2 | `file` wording rejects static-pie | S2 |
| 10 | P2 | the Mac distribution cell dropped | S7 |
| 11 | P2 | install.sh not a release asset | S2, S3, §5.3 |
| 12 | P2 | the Linux checksum is not origin | D6 |
| 13 | P2 | cells needed the rc before it existed | §4, S3, S5 |
| 14 | P2 | the shipped build unproved; rc bundle versions | D4, S7 |
| 15 | P2 | the clean-user leg could not run | S7 |
| 16 | P2 | PHASE2 §0.5 rules not carried | S1's rules |
| 17 | P2 | the publish wall did not stop a pipe or a test | §1.2 |
| 18 | P2 | the wall's mutant would run a forbidden file | §1.1 |
| 19 | P2 | D1's trust list; Linux offline; the commit | D1, D7 |
| 20–25 | P3 | cask text; shim gaps; profile check; Info.plist; install.sh; eval, CHANGELOG | S4, §1.3, S2, S0, S3, S6, S7 |

**Review 2 (draft 2):**

| # | Sev | Finding | Where |
|---|---|---|---|
| 1 | P1 | npm route: SIGPIPE ignored and a non-blocking stdout reach the job | D9, S5 |
| 2 | P1 | the door missed a release-ID bundle whose executable is not signed with that ID | §1.1 (three keys), D5 |
| 3 | P1 | one host-fetched `CARGO_HOME` cannot build in the image | D7 |
| 4 | P1 | one env list for all tools drops `CARGO_HOME` and the deployment target without error | §1.4, D8 |
| 5 | P2 | `publish` untestable; files not tied to the tag | §1.2 (planner) |
| 6 | P2 | no npm tarballs from the build; local installs miss the platform package | S2, S5, §5.5 |
| 7 | P2 | the Full Disk Access control could meet the Documents prompt | S7 |
| 8 | P2 | the Linux install cells: no curl, no host loopback | S3 |
| 9 | P2 | release cells: inherit mode and registration | S1's rules |
| 10 | P2 | the requirement's chicken-and-egg; which `release.conf` | §1.1 |
| 11 | P2 | PLAN §4.4 check 3 dropped | S7 |
| 12 | P2 | the smoke checked by `ps`; no `setsid` on macOS | S7 |
| 13 | P2 | nothing checks the published release | §5.6 |
| 14 | P3 | an explicit `--keychain` bypasses the temp HOME | §1.3 |
| 15 | P3 | two doors, three spellings | §1.1 (one implementation, a shared table) |
| 16 | P3 | fixture dirs not measured; unsigned leftovers | §1.1 |
| 17 | P3 | the swap is not atomic | S3 |
| 18 | P3 | D8 wording; no minimum-OS cell | D8 |
| 19 | P3 | the build counter falls back | D4 |
| 20 | P3 | the archive stores the user name | §1.5 |
| 21 | P3 | the loopback rule needs host-parsing cells | D6 |
| 22 | P3 | which install.sh is tested | S3 |
| 23 | P3 | readelf, bun, PLAN §5/§7 wording | D7, S5, PLAN amended |
| 24 | P3 | the profile check's labels | S2 |
| 25 | P3 | the distribution control | S7 |
| 26 | P3 | the clean-user leg wrote the operator's Homebrew | S7 |
| — | Q | may the agent run the real signed build? | D3: no; the operator runs `build --sign` |

**Review 3 (draft 3):**

| # | Sev | Finding | Where |
|---|---|---|---|
| 1 | P1 | node resets every disposition and the mask; a SIGPIPE repair cannot restore the caller's state | D9 (`sh` launcher; full-table cell, three callers) |
| 2 | P1 | the terminal wall is passed by `script -q /dev/null`; D3 claimed it stops the agent | D1, D2 step 3 (keychain dialog), D3, §1.2 (stated as a mistake guard) |
| 3 | P2 | the requirement comparison could never match codesign's output | §1.1 (`csreq -t` canonical form; real-output fixture) |
| 4 | P2 | a `--no-notarize` output was publishable | §1.2 (signed, notarized, stapled recorded apart; control marked) |
| 5 | P2 | automatic install cells needed the rc | S3 (moved to the `rc` leg) |
| 6 | P2 | `RUSTUP_TOOLCHAIN` overrides the pin; `rustc -V` never checked | D7, §1.4 |
| 7 | P2 | the toolchain file breaks floating test images | D7 (test legs use the digests; no `targets` line) |
| 8 | P2 | planner holes: section ref, upload set, annotated tags, `--verify-tag` | §1.2 |
| 9 | P2 | verdaccio: `npm publish` risk, a large dependency tree | S5 (static server), D1 |
| 10 | P2 | dry signing cell could reach the notary profile | §1.3 (`list-keychains`, `command -v` preconditions) |
| 11 | P2 | no `SSH_AUTH_SOCK` for git | §1.4 |
| 12 | P3 | the build counter could not be committed; tag order | D4 |
| 13 | P3 | one fixture table, two verdicts | §1.1 |
| 14 | P3 | perl `setsid` fails silently | §1.2, S7 |
| 15 | P3 | directory entries in the manifest | §1.5 |
| 16 | P3 | how x86_64 is built; which digest | D7 |
| 17 | P3 | bun already installed; lacks `process.execve` | S5 (moot with `sh`) |
| 18 | P3 | Debian server; the cask's local URL; brew bottles; `[::1]` | S3, S7, D6 |
| 19 | P3 | `~/Library/Safari` read with rc=0 from the agent shell | S7 (control first) |

**Review 4 (draft 4):**

| # | Sev | Finding | Where |
|---|---|---|---|
| 1 | P1 | the dry signing cell could exec an unsigned release-ID bundle; a lying `codesign` shim would pass the door | §1.1 (absolute `/usr/bin/codesign`), §1.1 (dry runs exec nothing), D3 (signing library called directly, no pty) |
| 2 | P2 | the `csreq` spelling always fails | §1.1 |
| 3 | P2 | the keychain gate can decay unseen | D2 step 3 (no Always Allow; repeat after store; fallback), S2 (per-build dialog check) |
| 4 | P2 | `publish` trusted the manifest's own flags | §1.2 (executor recomputes from the archive) |
| 5 | P2 | the draft's exact set lost; `SHA256SUMS` self-reference; stale drafts | §1.2, §1.5 |
| 6 | P2 | pnpm adds `NODE_PATH` | D9 |
| 7 | P2 | bun installs world-writable | D9 (stated limit) |
| 8 | P2 | the Linux npm packages untested | §3 `dist-linux` |
| 9 | P2 | `npm login` stores a token | D1, §5 step 5 |
| 10 | P3 | the `RUSTUP_TOOLCHAIN` mutant may stay green | D7 |
| 11 | P3 | npx breaks the contract | D9 (README) |
| 12 | P3 | `--access public` | D1, §5 step 5 |
| 13 | P3 | stale D1 text; invented notary fixtures | D1, S2 |
| 14 | P3 | two URL forms | D6 |
| 15 | P3 | pnpm and bun global config | S5 |

**Review 5 (draft 5):**

| # | Sev | Finding | Where |
|---|---|---|---|
| 1 | P2 | the `csreq` spelling still failed | §1.1 (`-r="<constant>"`; spelling mutants) |
| 2 | P2 | the dry signing cell could not go green without an override | §1.1 (end state is the door's refusal; no override) |
| 3 | P2 | `sign.sh` was an unguarded signing entry | S2 (nonce from `release.sh`, or dry preconditions), D3 |
| 4 | P2 | the `NODE_PATH` rule was wrong both ways | D9 (the exact leading prefix) |
| 5 | P2 | no check before `npm publish` | §5 step 5 (`npm-check`) |
| 6 | P2 | the executor's checks had no accepting control; by-name tools | §1.2 (absolute paths; the `rc` leg controls) |
| 7 | P3 | `gh release create` returns no id | §1.2 (`gh api` POST, `.id`) |
| — | note | status line; Deny wording; the token count's scope | status, D2, D1 |

**Review 6 (draft 6):**

| # | Sev | Finding | Where |
|---|---|---|---|
| 1 | P2 | the `NODE_PATH` prefix rule is still wrong (depth, symlinks) | **declined as a repair**: D9 states pnpm's `NODE_PATH` as a limit (two wrong drafts; small cost) |
| 2 | P2 | with a temp HOME, corepack fetches an unpinned pnpm | S5 (pinned `COREPACK_HOME`, no network, `pnpm -v` checked) |
| 3 | P2 | `--draft`/`--verify-tag` do not exist on the POST | §1.2 (the POST body checked; the tag checked before POST and PATCH) |
| 4 | P2 | the `--no-notarize` control made after the leg that needs it | §4 |
| 5 | P3 | full-path calls beyond the stated exception | §1.3 (named functions, one subcommand each); **the source-scan check declined** (operator: no tests that pin code) |
| 6 | P3 | no cell for a forged `--real` call | S2 |
| — | note | review 5's counts | status line |
