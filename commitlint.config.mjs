// Conventional Commits (https://www.conventionalcommits.org), checked on every
// commit of a pull request by .github/workflows/ci.yml, and on the PR title
// (the squash commit on main) by .github/workflows/pr-title.yml.
// release-please reads them to pick the next version: feat -> minor, fix/perf -> patch,
// `!` or a BREAKING CHANGE footer -> major (minor while we are on 0.x).
// Scopes are free: gl, script, engine, selector, core, harness, ci, docs...
export default {
  extends: ["@commitlint/config-conventional"],
};
