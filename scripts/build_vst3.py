"""Build SHELLOOP.vst3 (the CLAP plugin wrapped by clap-wrapper) and copy it out.

Usage: python scripts/build_vst3.py <output directory>

Needs CMake, a C++ toolchain and network access (CMake fetches clap-wrapper,
the CLAP headers and the VST3 SDK). Builds the Rust static library first.
"""
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BUILD = ROOT / "vst3" / "build"


def run(*command):
    print("+", " ".join(map(str, command)), flush=True)
    subprocess.run(command, cwd=ROOT, check=True)


def module_binary(bundle):
    """The loadable file inside a built SHELLOOP.vst3."""
    if bundle.is_file():
        return bundle  # Windows single-file module
    linux = bundle / "Contents" / "x86_64-linux" / "SHELLOOP.so"
    if linux.is_file():
        return linux
    raise FileNotFoundError(f"no module binary inside {bundle}")


def main(out_dir):
    run("cargo", "build", "--release", "-p", "shelloop-clap")
    run("cmake", "-S", "vst3", "-B", str(BUILD), "-DCMAKE_BUILD_TYPE=Release")
    run("cmake", "--build", str(BUILD), "--config", "Release")
    found = [p for p in BUILD.rglob("SHELLOOP.vst3")]
    if not found:
        raise FileNotFoundError(f"SHELLOOP.vst3 was not produced under {BUILD}")
    built = found[0]
    run(sys.executable, "-I", "scripts/verify_vst3.py", str(module_binary(built)))
    out = Path(out_dir).resolve()
    out.mkdir(parents=True, exist_ok=True)
    target = out / "SHELLOOP.vst3"
    if target.is_dir():
        shutil.rmtree(target)
    elif target.exists():
        target.unlink()
    if built.is_dir():
        shutil.copytree(built, target)
    else:
        shutil.copy2(built, target)
    print(f"VST3 ready: {target}")


if __name__ == "__main__":
    main(sys.argv[1])
