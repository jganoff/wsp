set windows-shell := ["powershell.exe", "-NoLogo", "-Command"]

default: check

# one-time setup after cloning: installs the pre-commit hook
setup: install-hooks

# format code
fmt:
    cargo fmt --all

# format check + clippy
# --all-targets so tests and benches are linted too, not just the library and
# binary: without it ~65 findings accumulated in test code, ungated.
check:
    cargo fmt --all --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo clippy --workspace --all-targets --features wsp/codegen -- -D warnings

# generate SKILL.md from CLI introspection
skill: (build-bin "codegen")
    cargo run --release -p wsp --features codegen -- generate > skills/wsp-manage/SKILL.md


# build a release binary, optionally with extra features
[private]
build-bin features="":
    {{ if features == "" { "cargo build --release -p wsp" } else { "cargo build --release -p wsp --features " + features } }}

# build release binary
build: check build-bin

# run all tests
test:
    cargo test --workspace -- --test-threads=1

# audit dependencies for known vulnerabilities
audit:
    cargo audit

# type-check against Windows and Linux to catch platform-specific errors
check-cross:
    rustup target add x86_64-pc-windows-msvc x86_64-unknown-linux-gnu 2>/dev/null || true
    cargo check --workspace --target x86_64-pc-windows-msvc
    cargo check --workspace --target x86_64-unknown-linux-gnu

# full local CI pipeline (superset of .github/workflows/ci.yml — also runs check-cross and SKILL.md freshness)
ci: check check-cross audit build test
    @echo "Checking SKILL.md freshness..."
    @cargo run --release -p wsp --features codegen -- generate | diff -q - skills/wsp-manage/SKILL.md || (echo "SKILL.md is stale. Run 'just skill' to regenerate." && exit 1)
    @echo "ci: all checks passed"

# auto-fix formatting and lint where possible
fix:
    cargo fmt --all
    cargo clippy --workspace --all-targets --fix --allow-dirty -- -D warnings

# preview unreleased changelog
changelog:
    git cliff --unreleased

# Bumps versions and regenerates CHANGELOG.md into a commit, then stops:
# release.toml sets push=false and tag=false. Push the branch and open a PR.
# release step 1 — prepare the version bump on a branch, for review
release-prep level:
    cargo release {{level}} --execute

# CI creates the tag itself against the merged commit, so no one pushes tags
# from a laptop. With no version it dispatches a dry run that plans only.
# release step 2 — after the PR merges, trigger the release from CI
release-dispatch version="dry-run":
    gh workflow run Release --ref main -f tag={{version}}
    @echo "dispatched Release with tag={{version}} — watch: gh run list --workflow Release"

# install git pre-commit hook
[unix]
install-hooks:
    #!/usr/bin/env sh
    hooks_dir="$(git rev-parse --git-common-dir)/hooks"
    mkdir -p "$hooks_dir"
    cat > "$hooks_dir/pre-commit" <<'HOOK'
    #!/usr/bin/env sh
    just check
    HOOK
    chmod +x "$hooks_dir/pre-commit"
    echo "pre-commit hook installed to $hooks_dir/pre-commit"

[windows]
install-hooks:
    $gitDir = (Resolve-Path ((git rev-parse --git-common-dir).Trim())).Path
    $hooks = Join-Path $gitDir "hooks"
    New-Item -Force -ItemType Directory -Path $hooks | Out-Null
    # WriteAllText writes UTF-8 without a BOM; a BOM or CRLF would break the
    # `#!/usr/bin/env sh` shebang when Git for Windows runs the hook.
    [System.IO.File]::WriteAllText((Join-Path $hooks "pre-commit"), "#!/usr/bin/env sh`njust check`n")
    Write-Host "pre-commit hook installed to $hooks"
