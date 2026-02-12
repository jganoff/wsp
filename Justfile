default: check

# format code
fmt:
    cargo fmt

# format check + clippy
check:
    cargo fmt --check
    cargo clippy -- -D warnings

# build release binary
build: check
    cargo build --release

# run all tests
test:
    cargo test -- --test-threads=1

# full CI pipeline (mirrors .github/workflows/ci.yml)
ci: check build test

# auto-fix formatting and lint where possible
fix:
    cargo fmt
    cargo clippy --fix --allow-dirty -- -D warnings

# preview unreleased changelog
changelog:
    git cliff --unreleased

# dry-run a release (patch, minor, or major)
release level:
    cargo release {{level}}

# execute a release (patch, minor, or major)
release-execute level:
    cargo release {{level}} --execute

# install git pre-commit hook (works with worktrees)
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
