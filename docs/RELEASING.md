# Release procedure

Release artifacts must come from the tagged commit. The release workflow builds
Linux x86_64, Windows x86_64, macOS arm64 and macOS x86_64 on native runners,
tests all features, checks formatting and strict Clippy, builds standalone,
CLAP and VST3, loads the VST3 factory, and verifies each application archive.

1. Pass the pull request CI matrix and review source changes.
2. Keep root and CLAP crate versions identical. Use a prerelease version while
   hardware/DAW acceptance in `HARDWARE-ACCEPTANCE.md` remains open.
3. Create a new `v<crate version>` tag at the reviewed commit and push it. The
   workflow refuses mismatched versions or a checkout different from that tag.
   A manual dispatch must name that same existing tag.
4. Wait for every native build and package verification job. The publish job
   depends on all builds. Verify the release lists standalone, CLAP and VST3
   assets and their SHA-256 files for all four architectures/platforms.
5. Download a candidate on matching hardware and record real audio, MIDI and
   DAW acceptance separately. Factory loading is not a DAW listening test.

Existing release assets are never silently overwritten. Fixes get a new version
and tag. macOS bundles are ad-hoc signed; Apple Developer signing/notarization is
not configured. Linux does not establish Windows/macOS behavior.

Local Linux validation:

```sh
cargo fmt --all -- --check
cargo test --workspace --all-targets --all-features --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
python3 -m unittest discover -s scripts/tests -v
cargo build --workspace --release --all-features --locked
python3 scripts/build_vst3.py vst3-out
```
