# sheepdog: phase 3 build plan ("Ship")

**Status:** draft 2 (2026-09-28). Nothing is built. Review 1 (draft 1, b03ce12): 4 P1, 15 P2, 6 P3; all taken in (§6 maps each finding to where it went).

**Phase-3 scope (PLAN.md §7):** the releases (Linux: two static binaries; macOS: `Sheepdog.app`, universal, Developer ID-signed and notarized; checksums), `install.sh`, a Homebrew cask in `lukaso/tap`, npm `@lukaso/sheepdog` (cell 18), the README with the agent snippet (§10.6), the agent first-use eval (§10.9). Gate: a clean container, a clean Mac user, the distribution cell (PLAN §6: a browser download of the release runs, with Gatekeeper quarantine); a grant survives an upgrade; the grant holds through the PATH symlink (its control: denied when the grant is off); the first-use eval passes.

**Not in phase 3:** adoption (phase 4); the `treeKill()` entry point and the agent hook (TODOS.md); a signed `SHA256SUMS` (D6).

## 0. Decisions

**D1. Signing and notarization run on the operator's Mac, not in GitHub CI (operator, 2026-09-28: "copy chiefofstaff").** This replaces PLAN.md §5.1's CI design.
- **What chiefofstaff does** (read from its tracked files only: `scripts/release.sh`, `apps/desktop/electron-builder.json`, `DESKTOP_DISTRIBUTION.md`; its `apps/desktop/.env` was not opened):
  - The operator runs `scripts/release.sh` on the Mac.
  - It refuses a dirty tree and an existing tag, and builds in a fresh detached worktree of the tag.
  - The Developer ID Application certificate comes from the login keychain.
  - electron-builder signs with the hardened runtime and notarizes, with `APPLE_ID`, `APPLE_APP_SPECIFIC_PASSWORD` and `APPLE_TEAM_ID` from a gitignored `.env`.
  - The release is created as a draft, then published with `gh`.
- **What sheepdog copies:** the local script, the clean-tree and tag checks, the fresh worktree of the tag, the keychain certificate, the hardened runtime, notarization, a draft release published last.
- **What sheepdog changes:**
  - **No `.env` and no password variable.** The notary credential is a notarytool keychain profile, `sheepdog-notary`, made once by the operator (D2). `notarytool submit --keychain-profile sheepdog-notary` reads it from the keychain. So the password is not in a file, an environment, an argument list (visible to `ps`), a log, or an agent's context.
  - **A new app-specific password, used only by sheepdog** (D2). chiefofstaff's password is in plain text in its `.env`, which any process running as the operator can read; reusing it would put sheepdog's notary credential in a file after all.
  - **No signing secret exists in GitHub.** No certificate export (`.p12`), no `CSC_LINK`, no repository or environment secret. A fork, a fork PR or a compromised workflow has nothing to reach. §5.1's other rules stay: no `pull_request_target`, no `workflow_run` on fork events, every third-party action pinned by commit SHA, forks build ad-hoc with the `.dev` bundle ID. A CI workflow, if added, runs the test matrix only, with `permissions: contents: read` and no `secrets.` reference.
- **Given up, stated plainly:** **every process that runs as the operator can sign and notarize as the operator.** The login keychain lets `codesign` use the key with no prompt (measured in phase 0), and `notarytool --keychain-profile sheepdog-notary` works for any caller of `notarytool`. That includes a malicious `build.rs` or proc-macro, an npm `postinstall` anywhere on this Mac, and this agent. It is already true for every build on this Mac today. What the release does to limit it:
  - one crate dependency (`libc`); `--locked`;
  - a fresh `CARGO_HOME` per release, filled by `cargo fetch --locked` (Cargo checks each download against `Cargo.lock`'s checksum; an unpacked source in the shared `~/.cargo/registry/src` is not re-checked, so the shared one is not used), then `--offline`;
  - the Linux images pinned by digest (D7);
  - still trusted, and named: the rustup toolchain, Xcode's `codesign`/`notarytool`, Docker Desktop.
  The closing option, if ever needed: build as a second macOS user with no signing identity, then sign as the operator.
- **npm:** published by the operator's own `npm publish` (npm's 2FA prompt). No npm token is stored. §5.1's OIDC route needs GitHub Actions and is dropped.
- **The Homebrew tap:** pushed with the operator's own git credentials. No token is stored.

**D2. The operator's one-time setup.** The operator runs these in a normal terminal tab (not the agent's `!` prefix, so the hidden prompts work). The agent never runs them and never reads the keychain item.
1. Make a new app-specific password at account.apple.com (Sign-In and Security > App-Specific Passwords), named `sheepdog-notary`.
2. Store it:
   ```sh
   xcrun notarytool store-credentials sheepdog-notary --apple-id <your Apple ID> --team-id P7UM972E39
   ```
   It asks for the password (hidden) and checks it with Apple before it saves it.
3. Check the item's access list: open Keychain Access, find the `com.apple.gke.notary.tool` item for `sheepdog-notary`, Access Control tab. Record in PHASE3.md whether it says "Confirm before allowing access" or lists apps that may read it without a prompt. This is the only barrier against a process that reads the item directly. If it allows reading without a prompt, that is stated in D1's "given up".

- The team ID `P7UM972E39` is not a secret: Apple writes it into every signed binary, and PLAN.md §4.4 records it.
- The signing identity is in the login keychain: `security find-identity -v -p codesigning` lists one, `Developer ID Application: Lukas Oberhuber (P7UM972E39)` (2026-09-28). `scripts/release.sh` selects it by its SHA-1 hash, which the operator pastes into `scripts/release.conf` (tracked; the hash of a public certificate is not a secret). A renewed certificate makes the name ambiguous; the hash never is.

**D3. What the agent may and may not do with secrets:**
- It never reads chiefofstaff's `.env`, never runs `store-credentials`, never runs `security find-generic-password`, `security export` or `security dump-keychain`, and never prints a keychain item.
- It may run `scripts/release.sh build` (which uses the profile only inside `notarytool`) after D2.
- It never runs `scripts/release.sh publish`, never pushes, never creates the GitHub repo, never publishes to npm, never pushes the tap (§5).

**D4. Version:** the first release is `v0.1.0`. PLAN.md §10.8's contracts start at `1.0.0`; before that, adoption (phase 4) may change them. Taste call; the operator may override. The rc tags are `v0.1.0-rc.N`. **Bundle versions are dotted integers** (Apple's format; `plutil -lint` does not check it, measured): `CFBundleShortVersionString` = `0.1.0` for both `v0.1.0-rc.N` and `v0.1.0`; `CFBundleVersion` = a build number that only rises (`N` for rc.N, and `100` for the final). A cell checks both against `^[0-9]+(\.[0-9]+){0,2}$`.

**D5. Bundle IDs:** `com.lukaso.sheepdog` only for a bundle made by `scripts/release.sh build` and signed with the Developer ID. Every other bundle (tests, dev, forks) is `com.lukaso.sheepdog.dev` (PLAN.md §4.4: a mismatching build with the release ID switches off the user's grant).

**D6. What the checksum proves.** On macOS, install.sh checks the Developer ID signature, which proves origin. On Linux, `SHA256SUMS` comes from the same release as the binary, so it proves the download is intact, not where it came from: anyone who can replace the binary can replace the sums. The README and install.sh's header say so. A signed `SHA256SUMS` (a key published outside GitHub) is a later option, not in v0.1.0. install.sh refuses a `SHEEPDOG_INSTALL_BASE` that is not `https://` or loopback `http://`.

**D7. The Linux release build** runs in the `rust:1-alpine` image **pinned by digest** in `scripts/release.conf` (the test legs keep their floating tags). Its inputs: the release worktree and the fresh `CARGO_HOME` mounted read-only, no network (`--network none`) during the build, so `--offline` is enforced. `build.rs` runs `git` to name the commit; in the container the worktree's `.git` file names a host path. So the release passes the commit to the build instead: `build.rs` uses `SHEEPDOG_COMMIT_OVERRIDE` when it is set (a cell: without it, a build outside a repo still says `unknown`). A release cell asserts that every artifact's `--version` names the tag's commit.

**D8. The macOS minimum:** `LSMinimumSystemVersion` and `MACOSX_DEPLOYMENT_TARGET` are both `12.0`, and the cask says `depends_on macos: ">= :monterey"`. The responsibility SPI is measured only on 15 and 27. Older versions get the §4.3 fallback and its warning. Stated.

## 1. Walls for this phase (built first, in S0)

1. **No test runs a release-ID executable that lacks the Developer ID signature.** Phase 0 measured that running a mismatching build with the release ID switched off the operator's real grant. The identity is in the executable's own signature and travels with every copy of the file (measured in review 1: a bare copy of a bundle's executable keeps its `Identifier`). So:
   - **One exec door for the phase-3 cells** (`tests/common` for Rust cells; `scripts/lib/exec-guard.sh` for shell cells). Before it runs anything, it resolves the real path (symlinks followed), reads the signature `Identifier` of that file (`codesign -d`), and, if the identifier is `com.lukaso.sheepdog`, requires the exact designated requirement of D1's identity (below). It refuses otherwise, and the cell fails. An unsigned file passes (it cannot be matched to the release ID).
   - **The requirement** is the one the signed rc gets from `codesign -d -r-`, recorded in `scripts/release.conf` at the first real run. It must include the Developer ID markers (`certificate 1[field.1.2.840.113635.100.6.2.6]` and `certificate leaf[field.1.2.840.113635.100.6.1.13]`) and the team (measured in review 1 on the operator's chiefofstaff app: without the markers, an "Apple Development" certificate of the same team would pass). Before the first real run, the door refuses every release-ID executable.
   - **It checks before each exec**, including cell 18's npm-installed bundle and the S3 install cells (a check after a run is too late).
   - **Its cells never exec a forbidden file.** The door takes an exec seam. The refusal cells run with a recording seam that writes "would exec <path>" instead of running it. Cells: a bare copy, a hardlink and a symlink of an ad-hoc release-ID executable, and an ad-hoc release-ID bundle, are all refused (the recording is empty); a `.dev` bundle is allowed (the recording has it, the control). Mutant: remove the check; the recording has the forbidden path. No mutant ever runs a forbidden file.
   - **Fixture release-ID files live only in a temp dir** that Launch Services does not index (PLAN §4.4) and are deleted when the cell ends.
   - The release scripts use the same door for any exec of what they built (the post-sign checks run `codesign`/`spctl` on the bundle; they never run it; install.sh's `--version` check runs only after its own signature check, §S3).
2. **The release script cannot publish from a test or a pipe.**
   - `scripts/release.sh build vX.Y.Z` makes the local artifacts and stops. `scripts/release.sh publish vX.Y.Z` is a separate command.
   - `publish` refuses when any `SHEEPDOG_TEST_*` variable is set, and when stdin or `/dev/tty` is not a terminal. It reads the typed tag only from `/dev/tty`. A piped `echo v0.1.0 |` is refused.
   - Cell: `publish` under the test environment is refused, and the `gh` and `git` shims record nothing. Cell: `publish` with stdin from a pipe and no tty (`setsid`, no controlling terminal) is refused. Mutant for each.
   - Stated: a process that runs as the operator can still publish by hand with the operator's `gh` and `git` credentials. This wall stops accidents, not an attacker (D1).
3. **Dry cells cannot reach the real key, the real profile, or the network.**
   - Dry cells run `scripts/release.sh build` with `HOME` set to a temp dir. Measured in review 1: with a HOME that is not the operator's, `security find-identity -v -p codesigning` finds **0** identities; with HOME unset it still finds the real one. Each dry cell first runs the real `security find-identity -v -p codesigning` in its own environment and requires 0 (a control that the cell cannot sign).
   - Shims on `PATH` for every external tool the script calls: `codesign`, `xcrun` (covering `notarytool` and `stapler`), `spctl`, `security`, `ditto`, `lipo`, `cargo`, `docker`, `gh`, `git`, `npm`. Each records its argv and environment. The script calls every tool by name, never by a full path; each dry cell asserts that the set of shims that recorded equals the set the step needs, so a tool called by full path leaves a missing record and the cell goes red (mutant: `/usr/bin/codesign` in the script; red on the missing record, and HOME already stops it from signing).
   - The `git` shim passes read-only subcommands (`status`, `rev-parse`, `worktree`, `tag -l`) to the real git on a throwaway repo copy, and records `push` without running it. `cargo` and `docker` shims build nothing; they copy a fixture binary into place.
4. **The secret wall, as behaviour.** From the shims' records:
   - `notarytool` is called with `--keychain-profile sheepdog-notary` and never with `--password`, `--apple-id`, `--key`, `--key-id` or `--issuer`;
   - the environment each tool gets is exactly the named list: `HOME`, `PATH`, `TMPDIR`, `USER`, `LOGNAME`, plus `DEVELOPER_DIR` if set (measured: codesign needs the real HOME to find the keychain, so the real run passes it; the dry run's HOME is the temp dir);
   - a decoy `APPLE_APP_SPECIFIC_PASSWORD=decoy-<random>` (and `CSC_KEY_PASSWORD`, `GH_TOKEN`) in the caller's environment reaches no tool, no line of the script's output, and no file under the output directory.
   Mutant: pass the caller's environment through; the decoy cell goes red.
   The real password never takes these channels (D1 keeps it in the keychain). This wall guards the channels a later edit could open: a flag, an environment variable, a log line. The keychain channel itself is D1's stated limit and D2 step 3's check.
5. **The artifact holds only what it should.**
   - The build refuses a worktree with any untracked or ignored file before it builds.
   - The macOS archive is `tar --no-xattrs --no-mac-metadata --no-acls -czf`. Measured in review 1: the default macOS tar stores `com.apple.provenance` and any `com.apple.quarantine` in pax headers that `tar -t` does not list, and extraction restores them.
   - The manifest cell extracts each archive into a temp dir and asserts: the file list is exactly `Sheepdog.app/Contents/Info.plist`, `Contents/MacOS/sheepdog`, `Contents/_CodeSignature/CodeResources`, and the staple ticket `Contents/CodeResources` (its place is measured in review 1 on stapled apps in /Applications); `xattr -lr` of the tree is empty; the archive holds no `SCHILY.xattr` or `LIBARCHIVE.xattr` pax header. Control: an archive made without the flags from a tree with a planted xattr is refused.
   - `SHA256SUMS` lists exactly the release files, `install.sh` included (§S3).

## 2. Steps (each: write the failing cell, see it red, build, see it green, a mutant per fix)

**S0. The walls of §1, and the bundle builder.**
- Failing cells first: the §1.1 door cells, the §1.2 publish cells, the §1.3 isolation controls, the §1.4 secret cells, the §1.5 manifest cells, all against scripts that do not exist yet.
- `scripts/bundle.sh <binary> <out-dir> <version> <build-number> [--release-id]` writes `Sheepdog.app` with `Info.plist`: `CFBundleIdentifier`, `CFBundleName=Sheepdog`, `CFBundleExecutable=sheepdog`, `CFBundlePackageType=APPL`, `CFBundleInfoDictionaryVersion=6.0`, `CFBundleShortVersionString`, `CFBundleVersion` (D4), `LSMinimumSystemVersion=12.0` (D8), `LSUIElement=true`; and the binary at `Contents/MacOS/sheepdog`. The `.dev` ID unless `--release-id`.
- Cells: `plutil -lint` passes; the ID is `.dev` without the flag (control: with it); both versions match D4's pattern (control: `0.1.0-rc.1` is refused); `lipo -archs` is `x86_64 arm64` for a universal input.
- `build.rs` honours `SHEEPDOG_COMMIT_OVERRIDE` (D7), with its cell.

**S1. The universal Mac binary under the hardened runtime.**
- `rustup target add x86_64-apple-darwin`; `MACOSX_DEPLOYMENT_TARGET=12.0`; `cargo build --release --locked --target aarch64-apple-darwin` and `--target x86_64-apple-darwin`; `lipo -create`.
- **The risk this step retires:** notarization needs the hardened runtime. Measured in review 1: an ad-hoc `-o runtime` `.dev` bundle of a debug build, run through a symlink, gives `doctor`'s responsibility API resolved and the disclaim self-check OK, and `run --status-fd` shows `"tracking":"responsibility","degraded":null`. Not yet measured: the escapee cells, and a release (optimized) build.
- The cell: the macOS integration suite's supervised cells (cells 1–3, 12, 24, the disclaim cells, `doctor`) run against a debug build ad-hoc signed with `-o runtime` and bundled with the `.dev` ID. Then the same cells that need no test seam run against the **release** universal build, ad-hoc `-o runtime`, `.dev` ID, under PHASE2 §0.5's rules (below).
- The x86_64 slice runs under Rosetta (`arch -x86_64`) for cell 1 and cell 3. Stated: not run on a real Intel Mac.

**Rules for every cell that runs a release or rc binary** (PHASE2 §0.5, carried over): it strips every `SHEEPDOG_TEST_*` variable (a release build exits 125 when it sees one); it sets `HOME`, `XDG_STATE_HOME` and `SHEEPDOG_STATE` to a fresh temp dir; it passes `--no-sweep`; it never runs `kill`, `sweep` or `strays --kill` with that binary (a release build has no tag latch). Every process it builds is killed by the cell's recorded identity. The helper that runs a release binary enforces the first three; a cell asserts the helper refuses a `kill` subcommand.

**S2. `scripts/release.sh build` (local): build, sign, notarize, staple, checksum.**
- Refuses, each with a dry cell (red first): a dirty tree; HEAD is not the tag; the tag does not match `Cargo.toml`'s version (the rc suffix aside); the identity hash in `release.conf` is not in `security find-identity -v -p codesigning` (run in the same named environment, §1.4); the profile check fails.
- **The profile check** separates "missing" from "offline": `notarytool history --keychain-profile sheepdog-notary`; exit 0 is present; a failure whose output names the profile as not found is "missing"; any other failure is "cannot reach Apple" (the output is written only to the output directory, never echoed). A dry cell for each outcome.
- Builds in a fresh detached worktree of the tag, with its own `CARGO_TARGET_DIR` and a fresh `CARGO_HOME` (D1).
- Mac: the S1 universal build, `bundle.sh --release-id`, then:
  - `codesign --force --options runtime --timestamp -s <hash> Sheepdog.app`
  - `ditto -c -k --keepParent Sheepdog.app submit.zip`
  - `xcrun notarytool submit submit.zip --keychain-profile sheepdog-notary --wait --output-format json`: status must be `Accepted`; otherwise `notarytool log <id>` goes to the output directory and the script exits 1 before stapling
  - `xcrun stapler staple Sheepdog.app`
  - the archive `sheepdog-macos-universal.tar.gz` (§1.5).
- Checks after signing, each a hard failure: `codesign --verify --strict --deep`; `codesign -v -R` with the recorded requirement (§1.1; the first run records it and the operator confirms it holds both Developer ID markers and the team); the runtime flag is set; `spctl --assess --type execute`; `xcrun stapler validate`. None of them runs the bundle.
- Linux: `sheepdog-linux-aarch64` and `sheepdog-linux-x86_64` (`uname -m` names, as PLAN §10.8's Dockerfile), built per D7. **The static check:** `readelf -l` shows no `PT_INTERP` (measured in review 1: `file` says `static-pie linked` for x86_64 and `statically linked` for aarch64, so the words cannot be the check), and the binary runs cell 1 in a `FROM scratch` container. Control: the existing glibc build (`interpreter /lib/ld-linux-aarch64.so.1`, measured) is refused.
- Every artifact's `--version` names the tag's commit (D7), run through the release-binary helper.
- `SHA256SUMS` over `sheepdog-macos-universal.tar.gz`, the two Linux binaries and `install.sh`.
- The output directory is outside the repo. Nothing is uploaded.
- **One real run**, after the operator's D2 setup: `v0.1.0-rc.1`, a local tag. It is a real Apple submission; it publishes nothing.

**S3. `install.sh`.**
- POSIX sh. Downloads with `curl`, else `wget`, else it stops and names both. Checks with `sha256sum`, else `shasum -a 256`.
- Reads `SHEEPDOG_INSTALL_BASE` (default: the pinned version's GitHub release; D6's scheme rule). The cells serve a local directory with `python3 -m http.server --bind 127.0.0.1`.
- Downloads the platform artifact and `SHA256SUMS`, checks the sum, then:
  - **macOS:** unpacks into a temp dir next to the target, checks the §1.1 requirement and `spctl`, then swaps it in atomically (`mv` of the old bundle aside, `mv` of the new one in, remove the old). Target `~/Applications/Sheepdog.app` (Launch Services indexes it, PLAN §4.4). Links `~/.local/bin/sheepdog` → `~/Applications/Sheepdog.app/Contents/MacOS/sheepdog` (a symlink, never a hardlink, §4.4).
  - **Linux:** installs the binary to `~/.local/bin` (or `/usr/local/bin` as root), through a temp file and `mv`.
- If the link dir is not on `PATH`, it prints the one line that adds it.
- Runs `sheepdog --version` only after every check passed.
- Automatic cells (temp `HOME`, served locally):
  - a sum mismatch refuses and installs nothing;
  - a non-https, non-loopback base is refused;
  - an ad-hoc bundle with the release ID is refused and never runs (§1.1's recording seam: install.sh's final exec goes through the shell door);
  - a `.dev` bundle is refused;
  - the Linux happy path in clean `alpine` and `debian` images (no toolchain; `wget`-only on Alpine, `curl` on Debian), the distribution cell's Linux half;
  - a second install over the first replaces it, and the link still resolves.
- **After the rc run** (§4): the Mac happy path with the real rc bundle. The link resolves into the bundle; the running supervisor's path (`proc_pidpath` of the pid in `--status-fd`'s start line) is inside `Sheepdog.app`.
- `install.sh` is a release asset (§S2's `SHA256SUMS`, §5.3), so PLAN §10.8's `releases/latest/download/install.sh` resolves.

**S4. The Homebrew cask.**
- `Casks/sheepdog.rb`: `url` of the macOS archive, `sha256`, `depends_on macos: ">= :monterey"` (D8), `app "Sheepdog.app"`, `binary "#{appdir}/Sheepdog.app/Contents/MacOS/sheepdog"`, `zap trash: "~/.local/state/sheepdog"`, and `caveats` naming `tccutil reset SystemPolicyAllFiles com.lukaso.sheepdog` (uninstall does not remove the grant).
- Cells: `brew style` on the file (no tap needed). `brew audit --cask` needs a tapped tap, which writes the operator's Homebrew state; it runs in the clean-Mac-user leg (§S7).

**S5. npm `@lukaso/sheepdog`.**
- **Measured in review 1** (npm 11.6.0, local tarballs, `npm i -g --prefix <tmp> --ignore-scripts`): a `bin` declared in an optional dependency is not linked onto PATH; a main-package `bin` that climbs out with `../` is dropped; a main-package `bin` of `node_modules/<platform>/…` links but cannot name three platforms and breaks under hoisting. `npm pack` keeps the 0755 exec bit and sets every mtime to 1985. Node 24.9 has `process.execve`.
- **The mechanism:** the esbuild pattern for the payload (`@lukaso/sheepdog` has `optionalDependencies` on `@lukaso/sheepdog-darwin-universal`, `-linux-arm64`, `-linux-x64`, each with `os`/`cpu`; the darwin one holds the stapled `Sheepdog.app`; no `postinstall`), and a JS `bin` in the main package that finds its platform package with `require.resolve` and calls `process.execve(<bundle executable>, argv, env)`. The process becomes the bundle executable, same pid (PLAN cell 18). `engines.node` is the first release with `process.execve` (to measure in S5: 23.11 and 22.15 are the candidates). An older node gets a one-line error naming the executable's path to run directly, and exit 1.
- **To measure first in S5, with the `.dev` bundle, ad-hoc signed** (the staple is not covered until after the rc run): whether pack and install keep the bundle's signature valid (`codesign --verify --strict` of the installed bundle, through the §1.1 door); what pnpm `-g` and bun `-g` make of the JS `bin` (their own shim then node then `execve`: the final process must still be the bundle executable); whether bun has `process.execve` (if not, bun is stated as unsupported for the guarantee).
- **Cell 18** (npm `-g`, pnpm `-g`, bun `-g`, each with `--ignore-scripts`, into temp prefixes): the §1.1 door checks the installed bundle **before** the first run; the PATH entry's running process is the bundle executable (`proc_pidpath` of the pid in `--status-fd`'s start line is inside `Sheepdog.app`); TERM to that pid ends the whole tree (cell 3's escapee included). **Control (PLAN cell 18's own):** a node wrapper that spawns the executable instead of exec'ing it fails the cell (the pid on PATH is node's).
- **After the rc run:** cell 18 again with the stapled rc bundle packed in.

**S6. README, CHANGELOG, the agent snippet** (PLAN §10.6 order; §10.8 needs a CHANGELOG entry per release). The Dockerfile example uses the shipped version (D4), not `1.0.0`. The README states D6 (what the Linux checksum proves). A documentation step: reviewed, no test.

**S7. The manual legs (the operator; after the rc runs).** For each protected read below, the run uses `--status-fd` and the leg requires `"tracking":"responsibility"` and `degraded:null` (otherwise an allowed read may be iTerm's grant, PLAN §2, and proves nothing).
- **The grant through the PATH symlink** (from phase 1): install rc.1 with `install.sh`; grant Full Disk Access to `~/Applications/Sheepdog.app`; `sheepdog run --status-fd 3 -- ls ~/Documents` through `~/.local/bin/sheepdog`: allowed. **Control:** the same bundle, the same symlink, Full Disk Access switched off: denied. Then on again: allowed.
- **The grant survives an upgrade:** build rc.2 (a different binary: the rc number is in `CFBundleVersion`, and the commit differs), install over rc.1, the same read: allowed, with the same control.
- **The distribution cell's Mac half** (PLAN §6): download the rc's `sheepdog-macos-universal.tar.gz` with Safari from a local page (so it gets the quarantine attribute), unpack it in Finder, run the CLI through a symlink: it runs with no Gatekeeper block. The control is `spctl`'s verdict on an unstapled, unnotarized copy: rejected.
- **A clean Mac user** (an admin user made for this leg, deleted after): Homebrew installed into its own prefix, or the cask run by the operator with `--appdir=/Users/<that user>/Applications`; node from the official `.pkg` (it writes `/usr/local`, stated) or `nvm` in that user's home. Then `install.sh`, the cask from a local tap (`brew audit --cask --strict` here), and npm. Each runs cells 1 and 3 from a packaged script (`scripts/smoke.sh`, which needs no repo and no cargo: it uses a `sh` escapee and checks with `ps`), through the §1.1 door.
- **The shipped build's smoke:** `v0.1.0` is a new binary and a new notarization (the version changes). Before `publish`, it gets `install.sh` from the local output dir, then the granted protected read through the symlink with its control, then `scripts/smoke.sh`.
- **The agent first-use eval** (PLAN §10.9): the fixture `eval/hang.sh` starts a `setsid` escapee and writes its identity (pid and start time) to a file; `eval/check.sh` passes if that identity is gone, and the operator judges the other two rules (the final message states what was killed; wall time under 2 min). The operator runs it with a bounded token budget.

## 3. The full-matrix legs

The ten legs of `./test-all` stay. Phase 3 adds:
- `bundle` (Mac, automatic): S0 and S1 cells, S2 and S3 dry cells, S5's measurement cells.
- `dist-linux` (automatic): S2's static check and S3's Linux cells, in clean containers, from locally built static binaries.
- `rc` (Mac, manual; needs D2): S2's real run, S3's Mac happy path, S5's cell 18 with the stapled bundle.
- `grant`, `quarantine`, `clean-user`, `ship-smoke`, `eval` (manual, the operator): S7.

## 4. Order and checkpoints

S0 → S1 → **review** → S2 (dry cells) → S3 (automatic cells) → S5 (measurement and cell 18 with `.dev`) → **review** → the operator's D2 → S2's real rc.1 run → S3's Mac happy path → S5's cell 18 with the stapled rc → S4 → S6 → **review** → S7 (the operator; rc.2 inside it) → **final review** → the full matrix → `v0.1.0` build → its smoke (S7) → the operator's §5 steps.

## 5. The operator's one-way steps (held; this plan does not do them)

1. D2: the new app-specific password and the notary profile; the access-list check.
2. Create the public GitHub repo `lukaso/sheepdog` and push. First, TODOS.md becomes issues (operator, 2026-09-25).
3. Push the `v0.1.0` tag, then `scripts/release.sh publish v0.1.0`: a draft release with the five files (the three artifacts, `install.sh`, `SHA256SUMS`); check that the draft lists all five (`gh release view`); then publish.
4. Push the cask to `lukaso/tap`.
5. `npm publish` the four packages.

## 6. Review 1 findings → where they went

| # | Sev | Finding | Where |
|---|---|---|---|
| 1 | P1 | the wall checked the Info.plist ID, not the executable's signature; cell 18 checked after the run | §1.1 (door on the file's own Identifier, before each exec) |
| 2 | P1 | a dry cell could reach the real key through a full-path call | §1.3 (temp HOME + 0-identity control; shim records) |
| 3 | P1 | the symlink leg's control did not test the symlink; iTerm's grant could make it pass | S7 (tracking check; same bundle, grant off: denied) |
| 4 | P1 | the npm design could not put a bin on PATH | S5 (JS `bin` + `process.execve`; cell 18's node-wrapper control) |
| 5 | P2 | the requirement lacked the Developer ID markers | §1.1, S2 (recorded designated requirement) |
| 6 | P2 | the secret wall guarded no real channel; reused password in a file | D1, D2 (new password; access-list check); §1.4 states its scope |
| 7 | P2 | a cleared environment breaks codesign | §1.4 (named list with the real HOME); D2 (identity by hash) |
| 8 | P2 | tar carries xattrs the manifest could not see | §1.5 |
| 9 | P2 | `file` wording rejects static-pie | S2 (`PT_INTERP`, `FROM scratch`, glibc control) |
| 10 | P2 | the Mac distribution cell was dropped | S7 (quarantine leg) |
| 11 | P2 | install.sh was not a release asset | S2, S3, §5.3 |
| 12 | P2 | Linux checksum is not origin | D6 |
| 13 | P2 | order: S3/S5 cells needed the rc before it existed | §4; S3, S5 split into before/after the rc |
| 14 | P2 | rc builds proved, v0.1.0 shipped unproved; rc bundle versions | D4, S7 (ship smoke) |
| 15 | P2 | the clean-user leg could not run | S7 |
| 16 | P2 | PHASE2 §0.5 release-binary rules not carried | S1 (the rules paragraph) |
| 17 | P2 | the publish wall did not stop a pipe or a test | §1.2 |
| 18 | P2 | the wall's mutant would run a forbidden file | §1.1 (recording seam; no mutant execs) |
| 19 | P2 | D1's trust list; Linux `--offline`; commit in the container | D1, D7 |
| 20 | P3 | cask text | S4 |
| 21 | P3 | shim gaps | §1.3 |
| 22 | P3 | profile check offline | S2 |
| 23 | P3 | Info.plist keys; fixture bundles in indexed dirs | S0, D8, §1.1 |
| 24 | P3 | install.sh portability, PATH hint, atomic swap | S3 |
| 25 | P3 | eval fixture, CHANGELOG, Dockerfile version | S6, S7 |
