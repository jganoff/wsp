#!/bin/sh
set -eu

WSP_DEMO_BINARY=${WSP_DEMO_BINARY:-"$(pwd)/target/release/wsp"}
if [ ! -x "$WSP_DEMO_BINARY" ]; then
    echo "Build wsp first with: just build-bin" >&2
    exit 1
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT HUP INT TERM
real_git=$(command -v git)
bin="$tmp/bin"
data="$tmp/data"
mkdir -p "$bin" "$data/wsp"

for repo in alpha bravo; do
    upstream="$tmp/$repo"
    mkdir -p "$upstream"
    git -C "$upstream" init --quiet --initial-branch=main
    git -C "$upstream" config user.email fixture@example.test
    git -C "$upstream" config user.name Fixture
    git -C "$upstream" config commit.gpgsign false
    git -C "$upstream" commit --quiet --allow-empty -m initial

    mirror="$data/wsp/mirrors/github.com/demo/$repo.git"
    mkdir -p "$(dirname "$mirror")"
    git clone --quiet --bare "$upstream" "$mirror"
done

cat > "$data/wsp/config.yaml" <<EOF
workspaces_dir: "$tmp/workspaces"
hints: false
repos:
  github.com/demo/alpha:
    url: "$tmp/alpha"
    added: "2026-10-08T00:00:00Z"
  github.com/demo/bravo:
    url: "$tmp/bravo"
    added: "2026-10-08T00:00:00Z"
EOF

cat > "$bin/git" <<'EOF'
#!/bin/sh
if [ "$1" = fetch ]; then
    sleep 3
fi
exec "$WSP_DEMO_REAL_GIT" "$@"
EOF
chmod +x "$bin/git"

export HOME="$tmp"
export XDG_DATA_HOME="$data"
export WSP_DEMO_REAL_GIT="$real_git"
export PATH="$bin:$PATH"
"$WSP_DEMO_BINARY" repo fetch --all
