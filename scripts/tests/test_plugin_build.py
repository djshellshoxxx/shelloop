"""Build-layout regressions runnable without a DAW or native cross compiler."""
import importlib.util
from pathlib import Path
import tempfile
import plistlib
import subprocess
import unittest

SCRIPTS = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('build_vst3', SCRIPTS / 'build_vst3.py')
build = importlib.util.module_from_spec(spec)
spec.loader.exec_module(build)


class ModuleBinaryTests(unittest.TestCase):
    def test_native_bundle_layouts(self):
        for relative in ('Contents/x86_64-linux/SHELLOOP.so', 'Contents/MacOS/SHELLOOP'):
            with self.subTest(relative=relative), tempfile.TemporaryDirectory() as directory:
                bundle = Path(directory) / 'SHELLOOP.vst3'
                binary = bundle / relative
                binary.parent.mkdir(parents=True)
                binary.write_bytes(b'native module')
                self.assertEqual(build.module_binary(bundle), binary)

    def test_windows_single_file(self):
        with tempfile.TemporaryDirectory() as directory:
            binary = Path(directory) / 'SHELLOOP.vst3'
            binary.write_bytes(b'native module')
            self.assertEqual(build.module_binary(binary), binary)

    def test_empty_bundle_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaises(FileNotFoundError):
                build.module_binary(Path(directory))


class MacPackageTests(unittest.TestCase):
    def test_clap_bundle_metadata_identifies_the_loadable_binary(self):
        spec = importlib.util.spec_from_file_location('package_macos', SCRIPTS / 'package_macos.py')
        package = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(package)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            library = root / 'plugin.dylib'
            library.write_bytes(b'plugin contents')
            destination = root / 'shelloop.clap'
            package.clap_bundle(library, destination, 'v1.1.0-rc.1')
            info = plistlib.loads((destination / 'Contents/Info.plist').read_bytes())
            binary = destination / 'Contents/MacOS' / info['CFBundleExecutable']
            self.assertEqual(binary.read_bytes(), library.read_bytes())
            self.assertEqual(info['CFBundleIdentifier'], 'com.circuitdriftlabs.shelloop')
            self.assertEqual(info['CFBundleVersion'], '1.1.0')


class ReleaseProvenanceTests(unittest.TestCase):
    def test_tag_version_and_commit_must_all_match(self):
        spec = importlib.util.spec_from_file_location('check_release_tag', SCRIPTS / 'check_release_tag.py')
        checker = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(checker)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'clap-plugin').mkdir()
            for name in ('Cargo.toml', 'clap-plugin/Cargo.toml'):
                (root / name).write_text('[package]\nversion = "1.1.0-rc.1"\n')
            def git(*args):
                subprocess.run(['git', *args], cwd=root, check=True, capture_output=True)
            git('init')
            git('add', '.')
            git('-c', 'user.name=Test', '-c', 'user.email=test@example.invalid',
                'commit', '-m', 'candidate')
            git('tag', 'v1.1.0-rc.1')
            checker.check('v1.1.0-rc.1', root)
            with self.assertRaisesRegex(ValueError, 'does not match'):
                checker.check('v1.1.0', root)
            (root / 'later.txt').write_text('different source')
            git('add', '.')
            git('-c', 'user.name=Test', '-c', 'user.email=test@example.invalid',
                'commit', '-m', 'later')
            with self.assertRaisesRegex(ValueError, 'does not match the existing tag'):
                checker.check('v1.1.0-rc.1', root)


if __name__ == '__main__':
    unittest.main()
