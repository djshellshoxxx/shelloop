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
    for relative in ("Contents/x86_64-linux/SHELLOOP.so", "Contents/MacOS/SHELLOOP"):
        binary = bundle / relative
        if binary.is_file():
            return binary
    raise FileNotFoundError(f"no module binary inside {bundle}")


def main(out_dir):
    run("cargo", "build", "--locked", "--release", "-p", "shelloop-clap")
    run("cmake", "-S", "vst3", "-B", str(BUILD), "-DCMAKE_BUILD_TYPE=Release")
    run("cmake", "--build", str(BUILD), "--config", "Release", "--parallel", "2")
    found = sorted(p for p in BUILD.rglob("SHELLOOP.vst3") if p.is_file() or p.is_dir())
    if not found:
        raise FileNotFoundError(f"SHELLOOP.vst3 was not produced under {BUILD}")
    if len(found) != 1:
        raise RuntimeError(
            "ambiguous VST3 build outputs; remove stale build directories: "
            + ", ".join(map(str, found))
        )
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
