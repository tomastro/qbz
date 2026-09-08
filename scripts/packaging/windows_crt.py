#!/usr/bin/env python3
"""Audit MSVC imports (including delay imports) in the exact MSI/ZIP payload.

No loader search paths or System32 fallback: every non-system MSVC dependency
must resolve beside qbz.exe and match the official-redist inventory.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import struct

CRT = re.compile(r"(?:msvc[pr]|vcruntime|vccorlib|concrt|vcomp|vcamp|mfc|mfcm)\d.*\.dll$", re.I)
DEBUG_CRT = re.compile(r"(?:d|d_[0-9]+)\.dll$", re.I)
SYSTEM_DEBUG_CRT = {'ucrtbased.dll', 'msvcrtd.dll'}


def pe_imports(path):
    data = Path(path).read_bytes()

    def unpack(fmt, offset):
        try:
            return struct.unpack_from(fmt, data, offset)
        except struct.error as error:
            raise ValueError(f"{path}: truncated PE") from error

    if data[:2] != b'MZ':
        raise ValueError(f"{path}: missing MZ header")
    pe, = unpack('<I', 60)
    if data[pe:pe + 4] != b'PE\0\0':
        raise ValueError(f"{path}: missing PE signature")
    machine, count = unpack('<HH', pe + 4)
    optional_size, = unpack('<H', pe + 20)
    optional = pe + 24
    magic, = unpack('<H', optional)
    if machine != 0x8664 or magic != 0x20b:
        raise ValueError(f"{path}: expected AMD64 PE32+, got machine {machine:#x}, magic {magic:#x}")
    if optional_size < 112:
        raise ValueError(f"{path}: invalid optional header")
    linker = unpack('BB', optional + 2)
    image_base, = unpack('<Q', optional + 24)
    header_size, = unpack('<I', optional + 60)
    directories, = unpack('<I', optional + 108)
    sections = []
    for index in range(count):
        section = optional + optional_size + 40 * index
        _, rva, raw_size, raw = unpack('<IIII', section + 8)
        sections.append((rva, raw_size, raw))

    def offset(rva):
        if 0 <= rva < min(header_size, len(data)):
            return rva
        for base, size, raw in sections:
            if base <= rva < base + size and raw + rva - base < len(data):
                return raw + rva - base
        raise ValueError(f"{path}: unmapped RVA {rva:#x}")

    def name(rva):
        start = offset(rva)
        end = data.find(b'\0', start, min(len(data), start + 1024))
        if end < 0:
            raise ValueError(f"{path}: unterminated import name")
        value = data[start:end].decode('ascii').lower()
        if not value or '/' in value or '\\' in value:
            raise ValueError(f"{path}: invalid import name {value!r}")
        return value

    imports = set()
    for index, stride in ((1, 20), (13, 32)):
        if directories <= index:
            continue
        if optional_size < 112 + (index + 1) * 8:
            raise ValueError(f"{path}: truncated data directories")
        rva, size = unpack('<II', optional + 112 + index * 8)
        if not rva and not size:
            continue
        if not rva or size < stride or size > len(data):
            raise ValueError(f"{path}: invalid import directory")
        for relative in range(0, size - stride + 1, stride):
            fields = unpack('<' + 'I' * (stride // 4), offset(rva + relative))
            if not any(fields):
                break
            dll_rva = fields[3] if index == 1 else fields[1]
            if index == 13 and not fields[0] & 1:
                dll_rva -= image_base
            imports.add(name(dll_rva))
        else:
            raise ValueError(f"{path}: unterminated import directory")
    return imports, linker


def version(value):
    parts = tuple(int(p) for p in str(value).split('.'))
    return parts + (0,) * (4 - len(parts))


def audit(root):
    root = Path(root)
    manifest = json.loads((root / 'licenses/msvc-runtime.json').read_text(encoding='utf-8-sig'))
    if manifest['architecture'] != 'x64' or not manifest['files']:
        raise ValueError('missing x64 official CRT inventory')
    inventory = {}
    for entry in manifest['files']:
        name = entry['name'].lower()
        if not CRT.fullmatch(name) or DEBUG_CRT.search(name):
            raise ValueError(f'non-release CRT in inventory: {name}')
        if name in inventory:
            raise ValueError(f'duplicate CRT entry: {name}')
        inventory[name] = entry
    local = {p.name.lower(): p for p in root.iterdir() if p.is_file()}
    for name, entry in inventory.items():
        if name not in local:
            raise ValueError(f'app-local CRT missing: {name}')
        if hashlib.sha256(local[name].read_bytes()).hexdigest() != entry['sha256'].lower():
            raise ValueError(f'CRT differs from official source: {name}')
        if version(entry['file_version']) < version(manifest['toolset_version']):
            raise ValueError(f'CRT older than build toolset: {name}')
    edges = []
    binaries = sorted(p for p in root.rglob('*') if p.suffix.lower() in ('.exe', '.dll'))
    if not (root / 'qbz.exe').is_file():
        raise ValueError('qbz.exe missing')
    for binary in binaries:
        imports, linker = pe_imports(binary)
        if binary.name.lower() in SYSTEM_DEBUG_CRT:
            raise ValueError(f'debug system CRT in payload: {binary}')
        if CRT.fullmatch(binary.name) and binary.parent != root:
            raise ValueError(f'CRT shadows app-local copy: {binary}')
        if CRT.fullmatch(binary.name) and binary.name.lower() not in inventory:
            raise ValueError(f'uninventoried/debug CRT in payload: {binary}')
        for name in sorted(imports):
            if name in SYSTEM_DEBUG_CRT:
                raise ValueError(f'{binary.relative_to(root)} imports debug system CRT {name}')
            if not CRT.fullmatch(name):
                continue
            if DEBUG_CRT.search(name) or name not in inventory:
                raise ValueError(f'{binary.relative_to(root)} imports unbundled/debug CRT {name}')
            if version(inventory[name]['file_version'])[:2] < linker:
                raise ValueError(f'{binary.name}: {name} older than linker {linker}')
            edges.append({'binary': str(binary.relative_to(root)), 'imports': name})
    if not edges:
        raise ValueError('no MSVC imports found; wrong or incomplete payload')
    return {'binaries': len(binaries), 'crt_files': len(inventory), 'imports': edges}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('directory', type=Path)
    parser.add_argument('--report', type=Path)
    args = parser.parse_args()
    report = audit(args.directory)
    if args.report:
        args.report.write_text(json.dumps(report, indent=2) + '\n')
    print(f"CRT audit OK: {report['binaries']} AMD64 binaries, {report['crt_files']} official app-local DLLs, {len(report['imports'])} MSVC import edges")


if __name__ == '__main__':
    main()
