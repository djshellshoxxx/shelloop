"""Package native macOS standalone, CLAP bundle and VST3 release archives."""
import hashlib
from pathlib import Path
import plistlib
import shutil
import sys
import tarfile


def clap_bundle(library, destination, version):
    contents = destination / 'Contents'
    (contents / 'MacOS').mkdir(parents=True)
    shutil.copy2(library, contents / 'MacOS' / 'shelloop')
    (contents / 'Info.plist').write_bytes(plistlib.dumps({
        'CFBundleExecutable': 'shelloop',
        'CFBundleIdentifier': 'com.circuitdriftlabs.shelloop',
        'CFBundleName': 'SHELLOOP',
        'CFBundlePackageType': 'BNDL',
        'CFBundleVersion': version.removeprefix('v').split('-')[0],
        'CFBundleShortVersionString': version.removeprefix('v').split('-')[0],
    }))


def package(tag, platform, root=Path('.')):
    if platform not in ('macos-arm64', 'macos-x86_64'):
        raise ValueError('expected a native macOS platform name')
    if not tag.startswith('v') or any(c not in 'abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789.-' for c in tag):
        raise ValueError('invalid release tag')
    dist = root / 'dist'
    dist.mkdir(exist_ok=True)
    names = [f'shelloop-{tag}-{platform}', f'shelloop-clap-{tag}-{platform}',
             f'shelloop-vst3-{tag}-{platform}']
    for name in names:
        (dist / name).mkdir(exist_ok=False)
    app, clap, vst3 = [dist / name for name in names]
    shutil.copy2(root / 'target/release/shelloop', app)
    for name in ('README.md', 'LICENSE', 'COPYRIGHT-TRADEMARK.md'):
        shutil.copy2(root / name, app)
    for name in ('patterns', 'projects'):
        shutil.copytree(root / name, app / name)
    clap_bundle(root / 'target/release/libshelloop_clap.dylib',
                clap / 'shelloop.clap', tag)
    shutil.copytree(clap / 'shelloop.clap', app / 'shelloop.clap')
    shutil.copy2(root / 'clap-plugin/README.md', clap)
    shutil.copy2(root / 'LICENSE', clap)
    shutil.copytree(root / 'vst3-out/SHELLOOP.vst3', vst3 / 'SHELLOOP.vst3')
    shutil.copy2(root / 'vst3/README.md', vst3)
    shutil.copy2(root / 'LICENSE', vst3)
    for name in names:
        archive = dist / f'{name}.tar.gz'
        with tarfile.open(archive, 'w:gz') as bundle:
            bundle.add(dist / name, arcname=name)
        digest = hashlib.sha256(archive.read_bytes()).hexdigest()
        Path(str(archive) + '.sha256').write_text(f'{digest}  {archive.name}\n')
    return dist / f'{names[0]}.tar.gz'


if __name__ == '__main__':
    print(package(sys.argv[1], sys.argv[2]))
