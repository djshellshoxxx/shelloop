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
        resources = (
            "README.md",
            "LICENSE",
            "patterns/example-bassline.json",
            "projects/example-multitrack.json",
            "projects/example-synth-patches.json",
            "projects/example-samples.json",
            "projects/example-performance.json",
            "shelloop.clap",
            "projects/samples/kick.wav",
            "projects/samples/snare.wav",
            "projects/samples/hat.wav",
        )
        for name in resources:
            if not (package / name).is_file():
                raise ValueError(f"missing packaged resource: {name}")
        for name in resources:
            if name.endswith(".json") and name != "shelloop.clap":
                json.loads((package / name).read_text())
        result = subprocess.run([str(binary), "--help"], cwd=package,
                                capture_output=True, text=True, timeout=15, check=True)
        for flag in ("--pattern", "--project", "--tui", "--black-box-seconds"):
            if flag not in result.stdout:
                raise ValueError(f"packaged executable lacks {flag}")
        if (package / "shelloop.clap").stat().st_size < 10_000:
            raise ValueError("packaged CLAP plugin is unexpectedly small")
    print(f"Verified checksum, resources and executable: {archive.name}")


if __name__ == "__main__":
    verify(Path(sys.argv[1]).resolve())
