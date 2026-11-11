"""Build SHELLOOP.vst3 (the CLAP plugin wrapped by clap-wrapper) and copy it out.

Usage: python scripts/build_vst3.py <output directory>

Needs CMake, a C++ toolchain and network access (CMake fetches clap-wrapper,
the CLAP headers and the VST3 SDK). Builds the Rust static library first.
"""
import plistlib
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
    macos = bundle / "Contents" / "MacOS" / "SHELLOOP"
    if sys.platform == "darwin" and macos.is_file():
        return macos
    raise FileNotFoundError(f"no module binary inside {bundle}")


def ensure_macos_bundle(bundle):
    """Make sure a macOS SHELLOOP.vst3 has Info.plist and an ad-hoc signature."""
    contents = bundle / "Contents"
    info = contents / "Info.plist"
    if not info.is_file():
        contents.mkdir(parents=True, exist_ok=True)
        info.write_bytes(plistlib.dumps({
            "CFBundleName": "SHELLOOP",
            "CFBundleExecutable": "SHELLOOP",
            "CFBundleIdentifier": "com.circuitdriftlabs.shelloop.vst3",
            "CFBundlePackageType": "BNDL",
            "CFBundleSignature": "????",
            "CFBundleVersion": "1.1.0",
            "CFBundleShortVersionString": "1.1.0",
            "CFBundleInfoDictionaryVersion": "6.0",
            "LSMinimumSystemVersion": "11.0",
        }))
    # Ad-hoc signature (no developer identity). Hosts on Apple Silicon need
    # at least this to load the module.
    run("codesign", "--force", "--sign", "-", str(bundle))


def main(out_dir):
    run("cargo", "build", "--release", "-p", "shelloop-clap")
    run("cmake", "-S", "vst3", "-B", str(BUILD), "-DCMAKE_BUILD_TYPE=Release")
    run("cmake", "--build", str(BUILD), "--config", "Release")
    found = sorted(p for p in BUILD.rglob("SHELLOOP.vst3") if p.is_file() or p.is_dir())
    if not found:
        raise FileNotFoundError(f"SHELLOOP.vst3 was not produced under {BUILD}")
    if len(found) != 1:
        raise RuntimeError(
            "ambiguous VST3 build outputs; remove stale build directories: "
            + ", ".join(map(str, found))
        )
    built = found[0]
    if sys.platform == "darwin" and built.is_dir():
        ensure_macos_bundle(built)
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
