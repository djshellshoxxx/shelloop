"""Refuse release artifacts unless both crate versions and HEAD match the tag."""
from pathlib import Path
import subprocess
import sys
import tomllib


def check(tag, root=Path('.')):
    for manifest in ('Cargo.toml', 'clap-plugin/Cargo.toml'):
        version = tomllib.loads((root / manifest).read_text())['package']['version']
        if tag != f'v{version}':
            raise ValueError(f'{tag!r} does not match {manifest} version {version!r}')
    def revision(ref):
        return subprocess.check_output(['git', 'rev-parse', '--verify', ref],
                                       cwd=root, text=True).strip()
    if revision('HEAD^{commit}') != revision(f'refs/tags/{tag}^{{commit}}'):
        raise ValueError('release checkout does not match the existing tag')
    print(f'Verified release source: {tag}')


if __name__ == '__main__':
    check(sys.argv[1])
