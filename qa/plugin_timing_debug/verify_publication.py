#!/usr/bin/env python3
"""Verify this source-only receipt publication without modifying it."""
from pathlib import Path
import ast, hashlib, json, re
root=Path(__file__).resolve().parent
expected={}
for line in (root/'PUBLICATION_SHA256SUMS').read_text().splitlines():
    digest,name=line.split('  ',1);expected[name]=digest
actual={p.relative_to(root).as_posix():p for p in root.rglob('*') if p.is_file() and p.name!='PUBLICATION_SHA256SUMS'}
assert set(actual)==set(expected), 'Missing or unexpected publication files'
for name,p in actual.items():
    raw=p.read_bytes()
    assert not p.is_symlink(),name
    assert hashlib.sha256(raw).hexdigest()==expected[name],name
    raw.decode('utf-8')
    assert not any(raw.startswith(magic) for magic in [b'\x7fELF',b'MZ',b'RIFF',b'PK\x03\x04']),name
    assert p.suffix.lower() not in {'.wav','.state','.bin','.so','.dll','.dylib','.vst3','.vstpreset','.fxp','.fxb','.pyc'},name
    if p.suffix=='.json':json.loads(raw)
    if p.suffix=='.jsonl':
        for line in raw.decode().splitlines():json.loads(line)
    if p.suffix=='.py':ast.parse(raw,filename=name)
inv=json.loads((root/'INVENTORY.json').read_text())
for e in inv['files']:
    if e['disposition']=='included':
        p=root/e['publication_relative_path'];raw=p.read_bytes()
        assert len(raw)==e['publication_bytes']
        assert hashlib.sha256(raw).hexdigest()==e['publication_sha256']
        if e['normalization']=='unchanged':assert e['raw_sha256']==e['publication_sha256']
    elif e['raw_bytes']==0:
        assert e['raw_sha256']==hashlib.sha256(b'').hexdigest()
        assert 'zero-byte' in e['reason']
print(f'PASS: {len(actual)} hashed files; all inventoried publication copies match; JSON and source syntax valid; text-only source/receipt archive.')
