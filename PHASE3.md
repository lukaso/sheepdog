# sheepdog: phase 3 build plan ("Ship")

**Status:** draft 1 (2026-09-28). Nothing is built. Not reviewed.

**Phase-3 scope (PLAN.md §7):** the releases (Linux: two static binaries; macOS: `Sheepdog.app`, universal, Developer ID-signed and notarized; checksums), `install.sh`, a Homebrew cask in `lukaso/tap`, npm `@lukaso/sheepdog` (cell 18), the README with the agent snippet (§10.6), the agent first-use eval (§10.9). Gate: a clean container, a clean Mac user, the distribution cell; a grant survives an upgrade; the grant holds through the PATH symlink (its control: an ad-hoc build that must be denied); the first-use eval passes.

**Not in phase 3:** adoption (phase 4); the `treeKill()` entry point and the agent hook (TODOS.md).

## 0. Decisions

**D1. Signing and notarization run on the operator's Mac, not in GitHub CI (operator, 2026-09-28: "copy chiefofstaff").** This replaces PLAN.md §5.1's CI design.
- **What chiefofstaff does** (read from its tracked files only: `scripts/release.sh`, `apps/desktop/electron-builder.json`, `DESKTOP_DISTRIBUTION.md`; its `apps/desktop/.env` was not opened):
  - The operator runs `scripts/release.sh` on the Mac.
  - It refuses a dirty tree and an existing tag, and builds in a fresh detached worktree of the tag.
  - The Developer ID Application certificate comes from the login keychain.
  - electron-builder signs with the hardened runtime and notarizes (`notarize: true`), with `APPLE_ID`, `APPLE_APP_SPECIFIC_PASSWORD` and `APPLE_TEAM_ID` from a gitignored `.env` loaded by `dotenv`.
  - The release is created as a draft, then published with `gh`.
- **What sheepdog copies:** the local script, the clean-tree and tag checks, the build in a fresh worktree of the tag, the keychain certificate, hardened runtime, notarization, a draft release that is published last.
- **What sheepdog changes, and why:**
  - **No `.env` and no password variable.** The notary credential is a **notarytool keychain profile**, `sheepdog-notary`, which the operator makes once (§0, D2). `notarytool submit --keychain-profile sheepdog-notary` reads it from the keychain. So the password is never in a file, an environment, an argument list (visible to `ps`), a log, or this agent's context. chiefofstaff's `.env` route gives the password to the tool through the environment. The same Apple ID and app-specific password can be used: the operator types it into the profile prompt.
  - **No signing secret exists in GitHub.** No certificate export (`.p12`), no `CSC_LINK`, no repository or environment secret. So a fork, a fork PR, or a compromised workflow has no secret to reach. This is stronger than §5.1's protected environment. §5.1's other rules stay: no `pull_request_target`, no `workflow_run` on fork events, every third-party action pinned by commit SHA, forks build ad-hoc with the `.dev` bundle ID.
  - **A CI workflow, if one is added, runs the test matrix only,** with `permissions: contents: read` and no `secrets.` reference.
- **Given up:** §5.1's "the build never sees a secret" (round 7). On the Mac, the build runs as the operator, whose login keychain lets `codesign` and `notarytool` use the key and the profile without a prompt (measured in phase 0: no dialog). A malicious `build.rs` or proc-macro could therefore sign or submit something. This is already true for every build on this Mac today (any `cargo build`, any `npm install`), so the release adds no new exposure. The release build limits it: one dependency (`libc`), `--locked`, `--offline` after a `cargo fetch --locked` whose checksums Cargo checks against `Cargo.lock`. Stated, not closed. The closing option, if ever needed: build in a second macOS user with no signing identity, then sign as the operator.
- **npm:** chiefofstaff publishes nothing to npm. §5.1's OIDC trusted publishing needs GitHub Actions. With local releases, npm is published by the operator's own `npm publish` (npm's 2FA prompt). No npm token is stored.
- **The Homebrew tap** is pushed by the operator's own git credentials. No token is stored.

**D2. The operator's one-time setup** (the operator runs these; this agent never runs them and never reads their output beyond the exit code):
```sh
xcrun notarytool store-credentials sheepdog-notary --apple-id <your Apple ID> --team-id P7UM972E39
# it prompts for the app-specific password (hidden); it validates it with Apple before saving
```
Run it in a normal terminal tab, not through the agent's `!` prefix, so the hidden prompt works. Check (prints no secret): `xcrun notarytool history --keychain-profile sheepdog-notary` exits 0.
- The team ID `P7UM972E39` is not a secret: Apple writes it into every binary it signs, and PLAN.md §4.4 already records it.
- The signing identity is already in the login keychain: `security find-identity -v -p codesigning` lists `Developer ID Application: Lukas Oberhuber (P7UM972E39)` (1 valid identity, 2026-09-28).

**D3. What this agent may and may not do with secrets:**
- It never reads chiefofstaff's `.env`, never runs `store-credentials`, never runs `security find-generic-password` or `security export`, and never prints a keychain item.
- It may run `scripts/release.sh` (which reads the profile only inside `notarytool`) once the operator has made the profile.
- It never pushes, never creates the GitHub repo, never publishes to npm and never pushes the tap. Each is the operator's one-way step (§5).

**D4. Version:** the first release is `v0.1.0`. The contracts of PLAN.md §10.8 start at `1.0.0`. Before then, adoption (phase 4) may still change them. Taste call; the operator may override.

**D5. Bundle IDs:** `com.lukaso.sheepdog` only for a bundle made by `scripts/release.sh` and signed with the Developer ID. Every other bundle (tests, dev, forks) is `com.lukaso.sheepdog.dev` (PLAN.md §4.4: a mismatching build with the release ID switches off the user's grant).

## 1. Walls for this phase (built first, in S0)

1. **No test runs a bundle that has the release ID unless it has the Developer ID signature.** Phase 0 measured that one run of a mismatching build with the release ID switches off the operator's real grant. Every test helper that runs a path inside a `.app` first checks `codesign -v -R '<the release requirement>'` when the bundle's ID is the release ID, and fails the cell if the check fails. Fixture bundles use the `.dev` ID. Cells that must *install* a release-ID bundle with a bad signature (the install refusal cells) never run it: they assert a marker file that the bundle's executable would write is absent. Control: the helper refuses an ad-hoc release-ID fixture (red if it runs it). Mutant: remove the check, and the control goes red on the marker, not on the grant (the fixture executable only writes a marker; it never reads a protected folder).
2. **The release script cannot publish by accident.** It stops after it makes the local artifacts. Publishing is a separate command, `scripts/release.sh publish vX.Y.Z`, that needs the tag pushed and asks for the tag to be typed. Nothing in the test suite calls `publish`. The dry cells run the script with `PATH` shims for `codesign`, `xcrun`, `gh`, `git push` and `npm` that record their argv and environment and never reach the network.
3. **The secret wall, as behaviour.** The dry cells assert, from the shims' records:
   - `notarytool` is called with `--keychain-profile sheepdog-notary` and never with `--password`, `--apple-id`, `--key` or `--key-id`;
   - no recorded environment of any shim holds a variable named `APPLE_*`, `CSC_*`, `*PASSWORD*` or `*TOKEN*` (a control cell sets `APPLE_APP_SPECIFIC_PASSWORD=decoy` in the caller, and it must not reach any shim: the script runs its tools with a cleared environment plus a named list);
   - the script's own stdout and stderr, and every file under the output directory, do not contain the decoy value.
   Mutant: pass the caller's environment through; the decoy cell goes red.
4. **The artifact holds only what it should.** The macOS archive's file list is exactly the bundle (`Contents/Info.plist`, `Contents/MacOS/sheepdog`, `Contents/_CodeSignature/CodeResources`, and the staple ticket `Contents/CodeResources` after stapling). The Linux artifacts are the single binaries. `SHA256SUMS` lists exactly the artifacts. A cell lists each archive and compares. Control: a stray file planted in the build worktree is refused (the script also refuses a worktree with any untracked or ignored file before it builds).

## 2. Steps (each one: write the failing cell, see it red, build, see it green, a mutant per fix)

**S0. The walls of §1, and the bundle builder.**
- Failing cells first: the §1.1 helper cells, the §1.2 no-publish cell, the §1.3 secret-wall cells, the §1.4 manifest cells, all against a `scripts/bundle.sh` that does not exist yet.
- `scripts/bundle.sh <binary> <out-dir> [--release-id]`: writes `Sheepdog.app` with `Info.plist` (`CFBundleIdentifier`, `CFBundleExecutable=sheepdog`, `CFBundleShortVersionString` and `CFBundleVersion` from `Cargo.toml`, `LSUIElement=true`, `CFBundlePackageType=APPL`) and the binary at `Contents/MacOS/sheepdog`. The `.dev` ID unless `--release-id`.
- Cells: `plutil -lint` passes; the ID is `.dev` without the flag (control: with it); `lipo -archs` is `x86_64 arm64` for a universal input.

**S1. The universal Mac binary under the hardened runtime.**
- `rustup target add x86_64-apple-darwin` (the operator's toolchain; recorded in the step's commit).
- `cargo build --release --locked --target aarch64-apple-darwin` and `--target x86_64-apple-darwin`, then `lipo -create`.
- **The risk this step retires:** notarization needs the hardened runtime (`codesign -o runtime`). sheepdog re-execs itself with `POSIX_SPAWN_SETEXEC`, calls the responsibility SPI through `dlsym`, reads other processes' `KERN_PROCARGS2` and `proc_pidinfo`, and signals same-uid processes. None of these is known to need an entitlement, but none is measured under the hardened runtime.
- The failing cell: the macOS integration suite's supervised cells (cells 1–3, 12, 24, the disclaim cells, `doctor`) run against a **debug** build that is ad-hoc signed with `-o runtime` and bundled with the `.dev` ID. No secret is needed. If a cell goes red, the entitlement it needs is found here, with its evidence, before any real signing.
- The x86_64 slice runs under Rosetta on this Mac for a smoke run (`arch -x86_64 …/sheepdog run -- true`, and one escapee cell). Stated: the x86_64 slice is not run on a real Intel Mac.

**S2. `scripts/release.sh` (local): build, sign, notarize, staple, checksum.**
- Refuses: a dirty tree; HEAD is not the tag `vX.Y.Z`; the tag does not match `Cargo.toml`'s version; no Developer ID identity for team `P7UM972E39`; the profile `sheepdog-notary` is missing (checked with `notarytool history`, output discarded).
- Builds in a fresh detached worktree of the tag, with its own `CARGO_TARGET_DIR`, `cargo fetch --locked`, then `--offline --locked`.
- Mac: the S1 universal build, `scripts/bundle.sh --release-id`, then
  `codesign --force --options runtime --timestamp -s "Developer ID Application: Lukas Oberhuber (P7UM972E39)" Sheepdog.app`,
  `ditto -c -k --keepParent Sheepdog.app submit.zip`,
  `xcrun notarytool submit submit.zip --keychain-profile sheepdog-notary --wait --output-format json`,
  status must be `Accepted` (otherwise fetch `notarytool log <id>` into the output directory and exit 1),
  `xcrun stapler staple Sheepdog.app`, then the release archive `sheepdog-macos-universal.tar.gz` (a tar keeps the bundle as is).
- Checks after signing, each a hard failure: `codesign --verify --strict --deep`; `codesign -v -R 'identifier "com.lukaso.sheepdog" and anchor apple generic and certificate leaf[subject.OU] = "P7UM972E39"'`; the hardened-runtime flag is set; `spctl --assess --type execute` accepts it; `stapler validate`.
- Linux: `aarch64` and `x86_64` musl static binaries built in the existing Alpine containers from the same worktree (`--locked --offline`), checked static (`file` says statically linked; the binary runs in a `scratch`-like container with no libc), named `sheepdog-linux-aarch64` and `sheepdog-linux-x86_64` (`uname -m` names, as the PLAN.md §10.8 Dockerfile uses).
- `SHA256SUMS` over the three artifacts.
- The output directory is outside the repo. Nothing is uploaded.
- Dry cells (shims, §1.2 and §1.3): each refusal (red first); a notary status of `Invalid` stops before stapling and writes the log; a failed check stops before checksums.
- **One real run**, after the operator's D2 setup: `v0.1.0-rc.1`, a local tag only. It is a real Apple submission; it publishes nothing.

**S3. `install.sh`.**
- Reads the base URL from `SHEEPDOG_INSTALL_BASE` (default the GitHub release of the pinned version; the cells use a local directory served with `python3 -m http.server` on 127.0.0.1).
- Downloads the platform artifact and `SHA256SUMS`, checks the sum, then:
  - macOS: unpacks to a temp dir, checks the signature requirement of S2 (the release ID and the team) and `spctl`, then moves `Sheepdog.app` to `~/Applications` (replacing an older one) and links `~/.local/bin/sheepdog` → `~/Applications/Sheepdog.app/Contents/MacOS/sheepdog` (a symlink: §4.4 says a hardlink keeps its own path);
  - Linux: installs the binary to `~/.local/bin` (or `/usr/local/bin` as root).
- Runs `sheepdog --version` only after every check passed.
- Cells (temp `HOME`): a sum mismatch refuses and installs nothing; an ad-hoc bundle with the release ID refuses and never runs (the marker of §1.1); a `.dev` bundle refuses; a Linux happy path in a clean Alpine and a clean Debian container (the distribution cell's Linux half); the Mac happy path with the real rc bundle (the symlink resolves into the bundle; `proc_pidpath` of the running job's supervisor is inside the bundle, read from `sheepdog ps --json`); a second install over the first replaces it and the link still works.

**S4. The Homebrew cask.**
- `Casks/sheepdog.rb` for `lukaso/tap`: `url` of the macOS archive, `sha256`, `app "Sheepdog.app"`, `binary "#{appdir}/Sheepdog.app/Contents/MacOS/sheepdog"`, `zap` that names `tccutil reset All com.lukaso.sheepdog` in `caveats` (the grant is not removed by uninstall).
- Cells: `brew style` and `brew audit --cask --strict` against a local tap dir. The install itself is in the clean-Mac-user leg (§3), because it writes `/Applications`.

**S5. npm `@lukaso/sheepdog`.**
- The esbuild pattern: `@lukaso/sheepdog` has `optionalDependencies` on `@lukaso/sheepdog-darwin-universal`, `@lukaso/sheepdog-linux-arm64`, `@lukaso/sheepdog-linux-x64`, each with `os`/`cpu` set. The darwin package contains the stapled `Sheepdog.app`. No `postinstall` (works with `--ignore-scripts`).
- The `bin` is not a JS launcher: cell 18 requires that the PATH link's process is the bundle executable. So the platform package's `bin` names the bundle executable itself (`Sheepdog.app/Contents/MacOS/sheepdog`), and the main package only depends on the platform packages. **Open, to measure first in S5:** whether npm links a `bin` from an optional dependency onto PATH (if not, the main package's `bin` must reach it another way, and the cell says which); whether `npm pack` keeps a symlink `bin`, the exec bit, and the bundle's signature and staple ticket; and what `pnpm` and `bun` make of a non-JS bin (pnpm writes a shell shim that execs the target: the process is still the bundle executable). The cell decides; the design follows the measurement.
- Cell 18: `npm i -g` into a temp prefix from the locally packed tarballs; the running supervisor's `proc_pidpath` is inside `Sheepdog.app`; `codesign -v -R` of that bundle passes. Also with `pnpm add -g` and `--ignore-scripts`.

**S6. README and the agent snippet** (PLAN.md §10.6 order). A documentation step: reviewed, no test.

**S7. The manual legs (the operator, once the rc exists).**
- **The grant through the PATH symlink** (moved here from phase 1): install rc.1 by `install.sh`, grant Full Disk Access to `~/Applications/Sheepdog.app`, run `sheepdog run -- ls ~/Documents` through `~/.local/bin/sheepdog`: allowed. Control: an ad-hoc `.dev` bundle through a symlink, not granted: denied. (Not an ad-hoc bundle with the release ID: that would switch off the grant, §1.1.)
- **The grant survives an upgrade:** tag and build rc.2 (a different binary), install over rc.1, the same protected read: allowed.
- **A clean Mac user:** a new macOS user with no Homebrew state: `install.sh`, then the cask from a local tap, then npm; each runs cells 1 and 3.
- **The agent first-use eval** (PLAN.md §10.9): the operator runs it with a bounded token budget.

## 3. The full-matrix legs

The ten legs of `./test-all` stay. Phase 3 adds:
- `bundle` (Mac, automatic): S0 and S1 cells, S2 and S3 dry cells, S5's pack cells.
- `dist-linux` (automatic): S3's Linux cells in clean Alpine and Debian containers, from locally built static binaries.
- `rc` (Mac, manual; needs the D2 profile): S2's real run, S3's Mac happy path, S5's cell 18.
- `grant` and `clean-user` (manual, the operator): S7.

## 4. Order and checkpoints

S0 → S1 → review → S2 (dry) → S3 → S5's measurement → review → the operator's D2 setup → S2's real rc run → S4, S5 → S6 → review → S7 (the operator) → final review → the full matrix.

## 5. The operator's one-way steps (held; this plan does not do them)

1. D2: make the notary profile.
2. Create the public GitHub repo `lukaso/sheepdog` and push. First, TODOS.md becomes issues (operator, 2026-09-25).
3. Push the `v0.1.0` tag, then `scripts/release.sh publish v0.1.0` (a draft release with the four files, then published).
4. Push the cask to `lukaso/tap`.
5. `npm publish` the four packages.
