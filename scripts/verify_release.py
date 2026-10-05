"""Check checksum, required resources and executable in an extracted release."""
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import zipfile


def verify(archive):
    expected = Path(str(archive) + ".sha256").read_text(encoding="utf-8-sig").split()[0]
    actual = hashlib.sha256(archive.read_bytes()).hexdigest()
    if actual != expected:
        raise ValueError("archive checksum mismatch")
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        if archive.suffix == ".zip":
            with zipfile.ZipFile(archive) as bundle:
                bundle.extractall(root)
            binary = root / "shelloop.exe"
        else:
            with tarfile.open(archive) as bundle:
                bundle.extractall(root, filter="data")
            binary = root / archive.name.removesuffix(".tar.gz") / "shelloop"
        package = binary.parent
        for name in ("README.md", "LICENSE", "patterns/example-bassline.json"):
            if not (package / name).is_file():
                raise ValueError(f"missing packaged resource: {name}")
        json.loads((package / "patterns/example-bassline.json").read_text())
        result = subprocess.run([str(binary), "--help"], cwd=package,
                                capture_output=True, text=True, timeout=15, check=True)
        if "--pattern" not in result.stdout:
            raise ValueError("packaged executable lacks pattern CLI")
    print(f"Verified checksum, resources and executable: {archive.name}")


if __name__ == "__main__":
    verify(Path(sys.argv[1]).resolve())
