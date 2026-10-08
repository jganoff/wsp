#!/usr/bin/env python3
"""Record with asciinema; pass the release wsp binary as the first argument."""
import fcntl
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

binary = str(Path(sys.argv[1] if len(sys.argv) > 1 else "target/release/wsp").resolve())
env = dict(os.environ, XDG_DATA_HOME="data", TERM="xterm-256color")
for key in ("WSP_PWD", "WSP_CD_FILE", "WSP_SHELL"):
    env.pop(key, None)


def quiet(*args):
    return subprocess.run([binary, *args], env=env, check=True, text=True,
                          stdout=subprocess.PIPE, stderr=subprocess.PIPE).stdout


def show(*args):
    print("$ wsp " + " ".join(args), flush=True)
    subprocess.run([binary, *args], env=env, check=True)


with tempfile.TemporaryDirectory(prefix="wsp-lifecycle-demo-") as fixture:
    os.chdir(fixture)
    Path("data/wsp").mkdir(parents=True)
    Path("data/wsp/config.yaml").write_text(
        "workspaces_dir: workspaces\nhints: false\nagent_md: false\n"
    )
    quiet("new", "draft", "--empty")
    print("Slow workspace operations stay visible", flush=True)
    print("\nAnother process holds draft's metadata lock.", flush=True)
    time.sleep(0.6)
    with Path("workspaces/draft/.wsp.yaml.lock").open("w+") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        lock.write(str(os.getpid()))
        lock.flush()
        print("$ wsp rename draft review", flush=True)
        child = subprocess.Popen([binary, "rename", "draft", "review"], env=env)
        time.sleep(2.8)
        fcntl.flock(lock, fcntl.LOCK_UN)
        if child.wait() != 0:
            raise RuntimeError("rename failed")
    metadata = Path("workspaces/review/.wsp.yaml").read_text()
    assert "name: review" in metadata
    assert not Path("workspaces/draft").exists()
    time.sleep(0.6)
    show("rm", "review", "--yes")
    assert not Path("workspaces/review").exists()
    time.sleep(0.6)
    show("recover", "review")
    assert Path("workspaces/review/.wsp.yaml").exists()
    print("\nLock released, rename completed, removal recovered.", flush=True)
    time.sleep(1.4)
