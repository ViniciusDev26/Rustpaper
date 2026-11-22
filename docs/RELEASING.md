# Releasing

Versions come from the commit history: commits follow
[Conventional Commits](https://www.conventionalcommits.org) and
[release-please](https://github.com/googleapis/release-please) turns them into
versions, a CHANGELOG and GitHub releases. Nobody edits version numbers by
hand.

## Commit messages

```
<type>(<optional scope>): <description>

<optional body>

<optional footers>
```

| type | effect on the version (while on 0.x) |
|---|---|
| `feat` | minor (0.1.0 -> 0.2.0) |
| `fix`, `perf` | patch (0.1.0 -> 0.1.1) |
| `feat!` / `BREAKING CHANGE:` footer | minor on 0.x, major from 1.0 |
| `docs`, `refactor`, `test`, `ci`, `build`, `chore`, `style` | none (in the CHANGELOG only if feat/fix/perf) |

Scopes are free; the usual ones are the crates and folders: `gl`, `script`,
`engine`, `selector`, `core`, `workshop`, `packaging`, `ci`, `docs`.
Examples: `feat(gl): compose layers group their children`,
`fix(script): camera zoom from setCameraTransforms`.

CI (`.github/workflows/ci.yml`, job *Conventional Commits*) runs commitlint
(`commitlint.config.mjs`) on **every commit** of a pull request. Locally:
`npx commitlint --from origin/main`. History before 2026-11-21 used
`area: message` and is ignored (`bootstrap-sha` in
`release-please-config.json`).

## The flow (`.github/workflows/release.yml`)

1. A push to `main` runs release-please. When there are `feat`/`fix`/`perf`
   commits since the last release, it opens (or updates) a **release PR**:
   - the next version in `Cargo.toml` (`[workspace.package]`, shared by every crate),
     `crates/rustpaper-selector/tauri.conf.json` and both PKGBUILDs;
   - `CHANGELOG.md`;
   - `.release-please-manifest.json`.
   The `lockfile` job then commits the refreshed `Cargo.lock` to that PR.
2. **Merging the release PR** tags `vX.Y.Z` and creates the GitHub release.
3. `build` (in an `archlinux:base-devel` container) runs
   `packaging/dist.sh` and attaches to the release:
   - `rustpaper-X.Y.Z-x86_64-linux.tar.gz` (+ `.sha256`): the five binaries,
     `lib/rustpaper/libsteam_api.so`, the `.desktop` entry, the icon,
     LICENSE and README;
   - `rustpaper-bin.PKGBUILD` with that tarball's checksum: `makepkg -si`.
4. `aur` publishes `rustpaper-bin` (the tarball) and `rustpaper` (built from
   the tag's source) to the AUR, with checksums from `updpkgsums`. It is
   skipped (a notice in the run) while the `AUR_SSH_PRIVATE_KEY` secret is
   not set.

## One-time setup

- **Repository settings -> Actions -> General -> Workflow permissions:**
  enable *Allow GitHub Actions to create and approve pull requests*, or
  release-please cannot open its PR.
- **AUR** (when registration is open): create an account at
  <https://aur.archlinux.org>, add an SSH public key to it, and store the
  private key as the repository secret `AUR_SSH_PRIVATE_KEY`
  (`ssh-keygen -t ed25519 -f aur -C rustpaper-ci`, then
  `gh secret set AUR_SSH_PRIVATE_KEY < aur`). The first publish creates the
  packages.
- Pushes made with `GITHUB_TOKEN` (the `Cargo.lock` commit on the release PR)
  do not trigger other workflows, so CI does not run again on that commit.
  The PR's earlier commits were checked; push an empty commit to re-run CI
  if needed.

## Testing the packaging locally

```sh
./packaging/dist.sh /tmp/dist                    # the release tarball
cd packaging/aur/rustpaper-bin && makepkg -f     # needs a published release
```

To try a PKGBUILD before a release exists, copy it next to a local tarball
and point `source=` at the file (that is how both were checked).

## Steamworks library

`rustpaper-workshop` links `libsteam_api.so`, the Steamworks SDK
redistributable that the `steamworks-sys` crate bundles. Its build.rs copies
it next to the binaries and sets RUNPATH `$ORIGIN:$ORIGIN/../lib/rustpaper`;
packages install it as `/usr/lib/rustpaper/libsteam_api.so`. Valve's SDK
license covers redistribution with Steam apps: shipping it in a third-party
package (AUR) is a grey area many packages accept; dropping
`rustpaper-workshop` from a package only removes the Workshop tab.
