#!/usr/bin/env python3
"""PE fixtures reproduce missing/transitive/architecture CRT packaging failures."""
import hashlib
import json
from pathlib import Path
import struct
import tempfile
import unittest
import sys

sys.dont_write_bytecode = True
from windows_crt import audit, pe_imports


def pe(path, imports=(), delayed=(), machine=0x8664):
    # One PE32+ .rdata section. Imports are real descriptor/name layouts;
    # fixtures are parser inputs, deliberately not runnable executables.
    data = bytearray(4096)
    data[:2] = b'MZ'
    struct.pack_into('<I', data, 60, 128)
    data[128:132] = b'PE\0\0'
    struct.pack_into('<HH', data, 132, machine, 1)
    struct.pack_into('<H', data, 148, 240)
    optional = 152
    struct.pack_into('<HBB', data, optional, 0x20b, 14, 44)
    struct.pack_into('<Q', data, optional + 24, 0x140000000)
    struct.pack_into('<I', data, optional + 60, 512)
    struct.pack_into('<I', data, optional + 108, 16)
    section = optional + 240
    struct.pack_into('<IIII', data, section + 8, 3584, 4096, 3584, 512)
    names = 2048
    for index, at, stride, values in ((1, 512, 20, imports), (13, 1024, 32, delayed)):
        if not values:
            continue
        struct.pack_into('<II', data, optional + 112 + index * 8, at - 512 + 4096, (len(values) + 1) * stride)
        for i, value in enumerate(values):
            rva = names - 512 + 4096
            if index == 1:
                struct.pack_into('<IIIII', data, at + i * stride, 0, 0, 0, rva, 0)
            else:
                struct.pack_into('<IIIIIIII', data, at + i * stride, 1, rva, 0, 0, 0, 0, 0, 0)
            encoded = value.encode() + b'\0'
            data[names:names + len(encoded)] = encoded
            names += len(encoded)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)


class Deployment(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='qbz-crt-test-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        pe(self.root / 'qbz.exe', ['MSVCP140.dll', 'KERNEL32.dll'])
        pe(self.root / 'msvcp140.dll', ['VCRUNTIME140.dll', 'ucrtbase.dll'])
        pe(self.root / 'vcruntime140.dll')
        pe(self.root / 'vcruntime140_1.dll', ['vcruntime140.dll'])
        pe(self.root / 'platforms/qwindows.dll', delayed=['VCRUNTIME140_1.dll'])
        self.manifest = {
            'architecture': 'x64', 'toolset_version': '14.44.35207',
            'files': [{'name': name, 'file_version': '14.44.35211.0',
                       'sha256': hashlib.sha256((self.root / name).read_bytes()).hexdigest()}
                      for name in ('msvcp140.dll', 'vcruntime140.dll', 'vcruntime140_1.dll')],
        }
        self.write_manifest()

    def write_manifest(self):
        path = self.root / 'licenses/msvc-runtime.json'
        path.parent.mkdir(exist_ok=True)
        path.write_text(json.dumps(self.manifest))

    def test_complete_payload_including_delay_loaded_plugin_passes(self):
        result = audit(self.root)
        self.assertEqual(result['binaries'], 5)
        self.assertIn({'binary': 'platforms/qwindows.dll', 'imports': 'vcruntime140_1.dll'}, result['imports'])

    def test_redistributable_installer_is_not_a_dll(self):
        (self.root / 'vcruntime140.dll').unlink()
        (self.root / 'vc_redist.x64.exe').write_bytes(b'installer does not satisfy imports')
        with self.assertRaisesRegex(ValueError, 'app-local CRT missing'):
            audit(self.root)

    def test_uninventoried_transitive_crt_fails(self):
        pe(self.root / 'platforms/qwindows.dll', ['concrt140.dll'])
        with self.assertRaisesRegex(ValueError, 'unbundled/debug CRT concrt140.dll'):
            audit(self.root)

    def test_missing_delay_import_fails(self):
        self.manifest['files'].pop()
        self.write_manifest()
        (self.root / 'vcruntime140_1.dll').unlink()
        with self.assertRaisesRegex(ValueError, 'vcruntime140_1.dll'):
            audit(self.root)

    def test_debug_import_fails(self):
        pe(self.root / 'platforms/qwindows.dll', ['MSVCP140D.dll'])
        with self.assertRaisesRegex(ValueError, 'unbundled/debug CRT'):
            audit(self.root)

    def test_wrong_architecture_in_a_plugin_fails(self):
        pe(self.root / 'platforms/qwindows.dll', machine=0x14c)
        with self.assertRaisesRegex(ValueError, 'expected AMD64'):
            audit(self.root)

    def test_debug_system_crt_import_is_rejected(self):
        pe(self.root / 'platforms/qwindows.dll', ['ucrtbased.dll'])
        with self.assertRaisesRegex(ValueError, 'debug system CRT'):
            audit(self.root)

    def test_unused_debug_crt_cannot_hide_in_payload(self):
        pe(self.root / 'msvcp140d.dll')
        with self.assertRaisesRegex(ValueError, 'uninventoried/debug CRT'):
            audit(self.root)

    def test_modified_crt_fails_source_hash(self):
        with (self.root / 'vcruntime140.dll').open('ab') as stream:
            stream.write(b'changed')
        with self.assertRaisesRegex(ValueError, 'differs from official source'):
            audit(self.root)

    def test_outdated_crt_fails_toolset_check(self):
        self.manifest['files'][0]['file_version'] = '14.43.1.0'
        self.write_manifest()
        with self.assertRaisesRegex(ValueError, 'older than build toolset'):
            audit(self.root)

    def test_newer_qt_linker_requires_a_newer_crt(self):
        path = self.root / 'platforms/qwindows.dll'
        data = bytearray(path.read_bytes())
        data[155] = 45
        path.write_bytes(data)
        with self.assertRaisesRegex(ValueError, 'older than linker'):
            audit(self.root)

    def test_nested_crt_cannot_shadow_the_audited_copy(self):
        pe(self.root / 'platforms/msvcp140.dll')
        with self.assertRaisesRegex(ValueError, 'shadows app-local'):
            audit(self.root)

    def test_truncated_pe_fails_closed(self):
        path = self.root / 'qbz.exe'
        path.write_bytes(b'MZ')
        with self.assertRaisesRegex(ValueError, 'truncated PE'):
            pe_imports(path)

    def test_system_ucrt_is_not_required_app_local(self):
        pe(self.root / 'qbz.exe', ['msvcp140.dll', 'api-ms-win-crt-runtime-l1-1-0.dll', 'msvcrt.dll'])
        self.assertEqual(audit(self.root)['crt_files'], 3)


if __name__ == '__main__':
    unittest.main()
