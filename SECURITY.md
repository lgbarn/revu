# Security Policy

## Supported versions

revu is pre-1.0 and ships from a single line of development. Security fixes land
on the latest release; please upgrade to the most recent
release before reporting.

| Version | Supported |
| ------- | --------- |
| latest release | yes |
| older releases | no — upgrade first |

## Reporting a vulnerability

Please report security issues **privately**, not in a public issue or pull
request.

- Preferred: open a private report via GitHub's
  [private vulnerability reporting](https://github.com/lgbarn/revu/security/advisories/new)
  (the **Security** tab → **Report a vulnerability**).

Include the affected version (`revu --version`), your platform, and the smallest
set of steps that demonstrates the issue. Please give maintainers a reasonable
window to investigate and ship a fix before any public disclosure.

## Scope and threat model

revu is a local command-line tool. Its Rust code contains no network client and
collects **no telemetry**. The optional `--pr` flow delegates fetching to the
user-installed `gh` CLI, which may use the network under the user's existing
authentication.
The security-relevant surfaces are:

- **Parsing untrusted diff/patch input** (`revu patch`, piped `pager` input, two
  arbitrary files). Parsing is pure and must never panic or execute input; a
  crash on malformed input is a valid report.
- **Invoking external programs** — `git`, `$EDITOR`/`$VISUAL`, and
  `$PAGER`/`$HUNK_TEXT_PAGER`. revu builds these as argument vectors (never a
  shell string) and separates user-supplied paths with `--`. A path that escapes
  argument boundaries or reaches a shell is a valid report.
- **Reading repo-local configuration** (`.revu/config.toml`) from a possibly
  untrusted cloned repository. Config is data only; any path that turns config
  into code execution or a traversal write is a valid report.

Out of scope: vulnerabilities in `git` itself, in your terminal emulator, or in
a `$PAGER`/`$EDITOR` you configured.
