#!/usr/bin/env python3
"""Create a native experimental binary bundle, not a cross-compiled/signed release."""
import argparse
import hashlib
from pathlib import Path
import platform
import subprocess
import tarfile

ROOT = Path(__file__).resolve().parent
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--binary", type=Path, default=ROOT / "target/release/omarchy-kanban")
parser.add_argument("--output", type=Path, default=ROOT / "dist")
args = parser.parse_args()
binary = args.binary.resolve()
version = subprocess.check_output([binary, "--version"], text=True).strip().split()[-1]
system = {"Darwin": "macos", "Linux": "linux"}.get(platform.system(), platform.system().lower())
name = f"agent-kanban-{version}-{system}-{platform.machine()}"
args.output.mkdir(parents=True, exist_ok=True)
destination = args.output / f"{name}.tar.gz"
# Exclusive creation: never silently replace an existing release artifact.
with destination.open("xb") as handle, tarfile.open(fileobj=handle, mode="w:gz") as archive:
    archive.add(binary, arcname=f"{name}/bin/omarchy-kanban")
    for item in ("install.py", "README.md", "README-legacy.md", "plugin"):
        archive.add(ROOT / item, arcname=f"{name}/{item}")
digest = hashlib.sha256(destination.read_bytes()).hexdigest()
destination.with_suffix(destination.suffix + ".sha256").write_text(f"{digest}  {destination.name}\n")
print(destination)
print("Install on a compatible host: python3 install.py --yes (no Rust required).")
print("This bundle is native to the build host; the checksum is not a release signature.")
