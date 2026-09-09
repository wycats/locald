import copy
import fnmatch
import json
from pathlib import Path
import stat
import tarfile
import tempfile
import unittest
from unittest.mock import patch

import package_macos as package

ROOT = Path(__file__).resolve().parents[2]
CHECKOUT = {'commit': 'a' * 40, 'tree': 'b' * 40}
METADATA = {'schema_version': 1, 'checkout': CHECKOUT, 'channel': 'stable',
            'rust_host': 'aarch64-apple-darwin', 'binary_architecture': 'arm64'}


class Packaging(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.binary = self.root / 'input-locald'
        # Fixture bytes are deliberately not an executable program; never invoked.
        self.binary.write_bytes(b'stable smoke-tested fixture bytes')
        self.binary.chmod(0o751)
        self.stage = self.root / 'stage'
        self.output = self.root / 'artifact'
        self.provenance = patch.object(package, 'provenance', return_value=copy.deepcopy(METADATA))
        self.provenance.start()
        self.addCleanup(self.provenance.stop)
        self.checkout = patch.object(package, 'checkout', return_value=CHECKOUT)
        self.checkout.start()
        self.addCleanup(self.checkout.stop)

    def stage_and_package(self):
        package.stage_binary(self.binary, self.stage)
        package.package_binary(self.stage, self.output)

    def test_archive_keeps_exact_binary_permissions_and_provenance(self):
        self.stage_and_package()
        receipt = json.loads((self.output / 'artifact.json').read_text())
        archive = self.output / receipt['archive']['name']
        self.assertEqual(package.sha256(archive), receipt['archive']['sha256'])
        self.assertEqual(receipt['checkout'], CHECKOUT)
        self.assertEqual(receipt['binary']['sha256'], package.sha256(self.binary))
        with tarfile.open(archive) as bundle:
            self.assertEqual(bundle.getnames(), ['locald', 'manifest.json'])
            self.assertEqual(bundle.getmember('locald').mode, 0o751)
            self.assertEqual(bundle.extractfile('locald').read(), self.binary.read_bytes())
            self.assertEqual(json.load(bundle.extractfile('manifest.json'))['checkout'], CHECKOUT)
        for line in (self.output / 'SHA256SUMS').read_text().splitlines():
            checksum, name = line.split('  ')
            self.assertEqual(checksum, package.sha256(self.output / name))
        self.assertNotIn('source-path.txt', {p.name for p in self.output.iterdir()})

    def test_binary_replacement_after_smoke_is_rejected(self):
        package.stage_binary(self.binary, self.stage)
        self.binary.write_bytes(b'nightly replacement')
        with self.assertRaisesRegex(RuntimeError, 'changed after smoke'):
            package.package_binary(self.stage, self.output)
        self.assertFalse(self.output.exists())

    def test_staged_binary_or_permissions_tamper_is_rejected(self):
        package.stage_binary(self.binary, self.stage)
        (self.stage / 'locald').chmod(0o640)
        with self.assertRaisesRegex(RuntimeError, 'permissions'):
            package.package_binary(self.stage, self.output)
        (self.stage / 'locald').chmod(0o751)
        (self.stage / 'locald').write_bytes(b'replaced')
        with self.assertRaisesRegex(RuntimeError, 'changed after smoke'):
            package.package_binary(self.stage, self.output)

    def test_non_executable_and_reused_staging_are_rejected(self):
        self.binary.chmod(0o644)
        with self.assertRaisesRegex(RuntimeError, 'owner-executable'):
            package.stage_binary(self.binary, self.stage)
        self.binary.chmod(0o751)
        package.stage_binary(self.binary, self.stage)
        with self.assertRaises(FileExistsError):
            package.stage_binary(self.binary, self.stage)

    def test_checkout_change_is_rejected(self):
        package.stage_binary(self.binary, self.stage)
        with patch.object(package, 'checkout', return_value={'commit': 'c'*40, 'tree': 'b'*40}), \
                self.assertRaisesRegex(RuntimeError, 'checkout changed'):
            package.package_binary(self.stage, self.output)


class Provenance(unittest.TestCase):
    def test_actual_checkout_and_run_context_are_distinct(self):
        env = {'GITHUB_REPOSITORY':'wycats/locald','GITHUB_RUN_ID':'123','GITHUB_RUN_ATTEMPT':'2',
               'GITHUB_EVENT_NAME':'pull_request','GITHUB_REF':'refs/pull/1/merge',
               'GITHUB_SHA':'d'*40,'GITHUB_SERVER_URL':'https://github.com','GITHUB_WORKFLOW':'CI'}
        def read(*command):
            return {('rustc','-vV'):'rustc 1.97.0\nhost: aarch64-apple-darwin',
                    ('/usr/bin/lipo','-archs','binary'):'arm64'}[command]
        with patch.dict(package.os.environ, env, clear=True), patch.object(package,'read',side_effect=read), \
                patch.object(package.platform,'system',return_value='Darwin'), \
                patch.object(package,'checkout',return_value=CHECKOUT):
            result = package.provenance(Path('binary'))
        self.assertEqual(result['checkout'], CHECKOUT)
        self.assertEqual(result['github']['GITHUB_SHA'], 'd'*40)
        self.assertEqual(result['run_url'],'https://github.com/wycats/locald/actions/runs/123')

    def test_unexpected_architecture_is_rejected(self):
        with patch.object(package,'read',side_effect=['host: aarch64-apple-darwin','x86_64']), \
                patch.object(package.platform,'system',return_value='Darwin'), \
                self.assertRaisesRegex(RuntimeError,'arm64 Mach-O'):
            package.provenance(Path('binary'))


class Workflow(unittest.TestCase):
    def setUp(self):
        self.workflow = (ROOT / '.github/workflows/ci.yml').read_text()
        self.job = self.workflow.split('\n  macos-build:\n',1)[1].split('\n  rust-tests-coverage:',1)[0]

    def test_rust_or_web_gate_covers_embedded_inputs_and_packager(self):
        self.assertIn("needs.changes.outputs.rust == 'true' || needs.changes.outputs.web == 'true'",self.job)
        filters = self.workflow.split('          filters: |\n',1)[1].split('            vscode:',1)[0]
        patterns = [line.strip()[3:-1] for line in filters.splitlines() if line.strip().startswith("- '")]
        for path in ('crates/locald-cli/src/main.rs','locald-dashboard/src/App.svelte',
                     'locald-docs/src/content/docs/index.mdx','docs/design/vision.md','package.json',
                     'pnpm-lock.yaml','pnpm-workspace.yaml','.npmrc','.cargo/config.toml',
                     'scripts/ci/package_macos.py','scripts/ci/test_package_macos.py'):
            self.assertTrue(any(fnmatch.fnmatchcase(path,p) for p in patterns),path)

    def test_stable_smoke_snapshot_then_both_test_suites_then_success_only_upload(self):
        order = ['cargo build --locked --release -p locald-cli --bin locald --features channel-stable',
                 'Smoke test - version','Smoke test - help','Preserve smoke-tested stable binary',
                 'Run unit tests (stable features)','Run unit tests (all features)',
                 'Package verified macOS binary','Upload verified macOS binary']
        positions = [self.job.index(value) for value in order]
        self.assertEqual(positions, sorted(positions))
        for name in ('Preserve smoke-tested stable binary','Package verified macOS binary','Upload verified macOS binary'):
            step = self.job.split('- name: '+name,1)[1].split('\n      - ',1)[0]
            self.assertIn("if: ${{ steps.gate.outputs.run == 'true' }}",step)
            self.assertNotIn('always()',step)
        self.assertIn('actions/upload-artifact@v4',self.job)
        self.assertIn('if-no-files-found: error',self.job)
        self.assertIn('retention-days: 14',self.job)
        self.assertIn('name: macOS Build',self.job)
        self.assertIn('runs-on: macos-latest',self.job)


if __name__ == '__main__':
    unittest.main()
