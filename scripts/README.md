# scripts

## Smoke tests

`smoke.sh` and `smoke.ps1` run in CI before every release is tagged
(`global-artifacts-jobs` in `dist-workspace.toml` makes `host` wait on them),
and locally via `just smoke`. A failure means no tag and no release.

They are deliberately shallow — "does the published binary work at all". Deep
behavioural coverage belongs in the e2e suite (#69); as that grows, these
should shrink to what only an artifact check can catch.

Two scripts, same checks: `smoke.sh` (macOS/Linux) and `smoke.ps1` (Windows).

    just smoke                       # against ./target/release/wsp
    just smoke path/to/wsp           # against a downloaded artifact

    ./scripts/smoke.sh  --wsp ./wsp     --expect-version 0.19.0-rc.1
    ./scripts/smoke.ps1 -Wsp .\wsp.exe -ExpectVersion 0.19.0-rc.1

Both have run and passed on their target platforms, `smoke.ps1` on the Windows
runner during a real release.

## What they do

Run a real `wsp` binary end to end. Use it to check a release build before
trusting it — especially on Windows, where CI builds the binary but never runs
a workflow with it.

Without cloning (Windows shown; adapt the URL for `smoke.sh`):

```powershell
gh release download v0.19.0-rc.1 -p "wsp-x86_64-pc-windows-msvc.zip"
Expand-Archive wsp-x86_64-pc-windows-msvc.zip -DestinationPath .\wsp-rc
irm https://raw.githubusercontent.com/jganoff/wsp/test/smoke-script/scripts/smoke.ps1 -OutFile smoke.ps1
./smoke.ps1 -Wsp .\wsp-rc\wsp.exe -ExpectVersion 0.19.0-rc.1
```

| Flag | |
|---|---|
| `--wsp` / `-Wsp` | path to the binary (required) |
| `--expect-version` / `-ExpectVersion` | fail unless `--version` and `wsp whatsnew` both mention it |
| `--offline` / `-Offline` | skip the network half |

One `ok`/`FAIL` line per check; non-zero exit if any fail.

**They sandbox themselves.** `XDG_DATA_HOME` points at a temp directory and
`workspaces-dir` is set inside it, so your registry, mirrors, and workspaces
are untouched. Cleanup runs in a `finally`/`trap`, so a mid-run failure still
leaves nothing behind.

`XDG_DATA_HOME` alone is not enough: commands detect the workspace from the
working directory, so the scripts also `cd` somewhere neutral. Without that,
running from inside a real workspace makes `doctor` inspect *that* one.

**Checks.** Offline: `--version`, `--help`, `doctor`, `ls`, and
shell completion — asserting the output contains `wsp` *and* parses
(`bash -n` / `zsh -n`, or the PowerShell parser), since a string match alone
would accept malformed output. `smoke.sh` checks whichever of bash/zsh are
installed and skips the rest. With `--expect-version`, also that
`wsp whatsnew` mentions it, proving the notes were compiled into the binary.

With network: register a repo, `new`, confirm the clone exists on disk, `st`,
`repo add` a second repo (exercising the fetch-before-clone path), `doctor`
inside a real workspace, `rm`.

**The network half uses real remotes on purpose** — that is what a user does,
and it is the point of validating a release. It clones `octocat/Hello-World`
and `octocat/Spoon-Knife`: small, public, no auth.

That is a choice, not a constraint. `registry add` needs a URL with a host, so
it rejects local paths (`file://` gives "host cannot be empty" — see #91), but
a `git daemon` on localhost satisfies it, and the registry can also be seeded
directly. Both are hermetic and both are written up in #69 for the e2e suite,
where flakiness would actually matter.

Start with `--offline` / `-Offline` on a new platform. It is fast and separates
"does this binary run at all" from "does the whole workflow work".

### In CI

`.github/workflows/smoke.yml` runs on ubuntu/macOS/Windows against the
binaries built earlier in the same release run, before anything is tagged.

It is wired in with `global-artifacts-jobs`, not `host-jobs`. That matters:
`host-jobs` generates a job with the same dependencies as `host`, so the two
race and the tag is created regardless of the outcome. `global-artifacts-jobs`
inserts upstream, giving `host: needs [..., custom-smoke]`.

Only three of the five build targets have runners, so `aarch64-unknown-linux-gnu`
and `aarch64-pc-windows-msvc` are built but never smoke-tested.

## Release notes

`whatsnew-draft.sh` collects the ```whatsnew blocks from every PR merged since
a tag, so the release author starts from notes the change authors wrote rather
than re-deriving prose from commit subjects.

    scripts/whatsnew-draft.sh            # since the most recent tag
    scripts/whatsnew-draft.sh v0.19.0    # since a specific tag

It reads `git log` only — no network. Squash is the only merge method here and
the squash body is the PR description verbatim, so the blocks are already in
`main`'s history. Blocks containing `NONE` are dropped.

Output is raw material: newest-first, one bullet per PR. Ordering and merging
related entries stays with whoever writes the release. See `RELEASING.md`.
