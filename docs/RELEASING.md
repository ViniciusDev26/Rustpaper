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

Scopes are free; the usual ones are the crates and folders: `engine`,
`selector`, `core`, `workshop`, `packaging`, `ci`, `docs`.
Examples: `feat(engine): private stage, Rustpaper's own compositor for WE's
windows`, `fix(selector): ...`.

The subject must start lowercase (commitlint's `subject-case`), even with a
proper noun: `fix(engine): sway adapter ...`, not `Sway adapter`. A subject
that slips through is only fixable by rewriting the branch, so check before
pushing.

**PRs are squash-merged** (the repository allows nothing else): the PR
title becomes the one commit on main, and the squash commit's body is left
blank. So the **PR title** is what release-please reads and must be a
Conventional Commit too (`.github/workflows/pr-title.yml` checks it, also on
edits). Merge commits used to carry the PR title in their body, which
release-please read as a second commit: every change of 0.5.0 and 0.6.0
was listed twice (2026-11-23).

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
   - `rustpaper-X.Y.Z-x86_64-linux.tar.gz` (+ `.sha256`): the three binaries
     (`rustpaper`, `rustpaper-selector`, `rustpaper-workshop`),
     `lib/rustpaper/libsteam_api.so`, the `.desktop` entry, the icon,
     the lock screen's PAM service (`etc/pam.d/rustpaper-lock`), LICENSE
     and README;
   - `rustpaper-bin.PKGBUILD` with that tarball's checksum: download it
     (`releases/latest/download/rustpaper-bin.PKGBUILD`) and run
     `makepkg -si`, which fetches the tarball.
4. `pacman-repo` (Arch container) builds `rustpaper-bin` from the release's
   tarball and PKGBUILD (`packaging/pacman-repo.sh`), signs the package and
   a one-package repository database with the `PACMAN_GPG_KEY` key, and
   uploads them, with the public key `rustpaper.gpg`, to the `pacman-repo`
   release: a prerelease that is never "latest", so `releases/latest`
   stays the version. The new package goes up first, then the database,
   then the old package is removed. The release notes carry the key's
   fingerprint. Skipped (a notice in the run) while the secret is not set.
5. `aur` publishes `rustpaper-bin` (the tarball) and `rustpaper` (built from
   the tag's source) to the AUR, with checksums from `updpkgsums`. It is
   skipped (a notice in the run) while the `AUR_SSH_PRIVATE_KEY` secret is
   not set.

## One-time setup

- **Repository settings -> Actions -> General -> Workflow permissions:**
  enable *Allow GitHub Actions to create and approve pull requests*, or
  release-please cannot open its PR.
- **pacman repository:** a signing key without passphrase (GitHub keeps
  the secret), stored as `PACMAN_GPG_KEY`:

  ```sh
  export GNUPGHOME=$(mktemp -d)
  gpg --batch --passphrase '' --quick-gen-key "Rustpaper packages" ed25519 sign never
  gpg --armor --export-secret-keys | gh secret set PACMAN_GPG_KEY
  gpg --list-secret-keys --with-colons | awk -F: '/^fpr/{print $10; exit}'  # the fingerprint
  ```

  Keep a backup of `$GNUPGHOME` somewhere safe: a new key means every user
  imports and signs it again. The next release creates the `pacman-repo`
  release.
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

The pacman repository: put a release's tarball and `rustpaper-bin.PKGBUILD`
in a folder and run `packaging/pacman-repo.sh <folder> <out> <fingerprint>`
with a throwaway key in `GNUPGHOME`. Checked that way (2026-11-23): pacman,
with a keyring of its own and `SigLevel = Required DatabaseRequired`,
verified the database signature against the imported key (only the local
trust step, `pacman-key --lsign-key`, needs root; the rest of the check
used `TrustAll`), then listed and downloaded `rustpaper-bin` with its
signature checked, and refused a modified package.

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
