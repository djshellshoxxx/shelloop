# SHELLOOP VST3 plugin

`SHELLOOP.vst3` is the SHELLOOP polyphonic synthesizer and host-synced step
sequencer as a VST3 instrument (vendor Circuit Drift Labs). It is the
[CLAP plugin](../clap-plugin/README.md) wrapped with
[clap-wrapper](https://github.com/free-audio/clap-wrapper); the CLAP code is
linked inside, so the `.vst3` is a single self-contained file/bundle. The
parameters, note input, sequencer and saved state are the same as the CLAP
version.

## Install

**Linux:** copy the `SHELLOOP.vst3` folder into your VST3 folder and rescan:

```sh
mkdir -p ~/.vst3
cp -r SHELLOOP.vst3 ~/.vst3/
```

**macOS:** copy the `SHELLOOP.vst3` bundle to `~/Library/Audio/Plug-Ins/VST3/`
and remove the quarantine attribute of the ad-hoc signed download (the bundle is
not notarized):

```sh
mkdir -p ~/Library/Audio/Plug-Ins/VST3
cp -R SHELLOOP.vst3 ~/Library/Audio/Plug-Ins/VST3/
xattr -dr com.apple.quarantine ~/Library/Audio/Plug-Ins/VST3/SHELLOOP.vst3
```

**Windows:** copy `SHELLOOP.vst3` to `C:\Program Files\Common Files\VST3\` (or
`%LOCALAPPDATA%\Programs\Common\VST3\`) and rescan plugins in your DAW.

## Build from source

Needs Rust, CMake 3.21+ and a C++ toolchain (on macOS, the Xcode Command Line
Tools; `brew install cmake`), plus network access (CMake downloads clap-wrapper,
the CLAP headers and the VST3 SDK):

```sh
python scripts/build_vst3.py vst3-out      # use python3 on macOS
```

The result is `vst3-out/SHELLOOP.vst3`. On macOS the script writes
`Contents/Info.plist` if clap-wrapper did not, and ad-hoc signs the bundle with
`codesign -s -`. The script builds the Rust plugin,
runs CMake on `vst3/CMakeLists.txt`, and loads the finished module the way a
host does to check its factory (`scripts/verify_vst3.py`).
