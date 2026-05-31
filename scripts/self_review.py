#!/usr/bin/env python3
"""PilotDesk Self-Review Checklist Verification Script"""
import os
import re
import sys

ROOT = r"E:\LingXi Workspace\20260530-10-40-29-689\pilotdesk"
SRC_TS = os.path.join(ROOT, "src")
SRC_RS = os.path.join(ROOT, "src-tauri", "src")

def find_files(root, ext):
    result = []
    for dirpath, _, filenames in os.walk(root):
        for f in filenames:
            if f.endswith(ext):
                result.append(os.path.join(dirpath, f))
    return result

def check_no_tbd():
    """Check no TBD/TODO placeholders in source files"""
    patterns = [r'\bTBD\b', r'\bTODO\b']
    issues = []
    for f in find_files(SRC_TS, '.ts') + find_files(SRC_TS, '.tsx'):
        content = open(f, encoding='utf-8').read()
        for pat in patterns:
            for m in re.finditer(pat, content):
                line = content[:m.start()].count('\n') + 1
                issues.append(f"  {os.path.relpath(f, ROOT)}:{line} - {m.group()}")
    return issues

def check_types_consistency():
    """Verify key types exist in both Rust and TypeScript"""
    rust_models = os.path.join(SRC_RS, "db", "models.rs")
    ts_types = os.path.join(SRC_TS, "types", "index.ts")
    
    rust_content = open(rust_models, encoding='utf-8').read()
    ts_content = open(ts_types, encoding='utf-8').read()
    
    checks = {
        'Session': ('struct Session', 'export interface Session'),
        'Message': ('struct Message', 'export interface Message'),
    }
    
    issues = []
    for name, (rust_sig, ts_sig) in checks.items():
        if rust_sig not in rust_content:
            issues.append(f"  Rust missing: {name} ({rust_sig})")
        if ts_sig not in ts_content:
            issues.append(f"  TypeScript missing: {name} ({ts_sig})")
    return issues

def check_no_api_key_leak():
    """Verify API key doesn't leave Rust layer (only masked values in frontend)"""
    issues = []
    for f in find_files(SRC_TS, '.ts') + find_files(SRC_TS, '.tsx'):
        content = open(f, encoding='utf-8').read()
        if re.search(r'api_key(?!_masked|_set)', content) and 'masked' not in content:
            # Check if it's in a type definition (public interface) - that's ok
            for m in re.finditer(r'api_key(?!_masked|_set)', content):
                ctx_start = max(0, m.start() - 50)
                ctx = content[ctx_start:m.end()+50]
                if 'interface' in ctx or 'type ' in ctx:
                    line = content[:m.start()].count('\n') + 1
                    rel = os.path.relpath(f, ROOT)
                    # Only flag if it's not in a Public type
                    if 'Public' not in ctx and 'Masked' not in ctx:
                        issues.append(f"  {rel}:{line} - possible raw api_key reference")
    return issues

def check_no_source_message_id():
    """Verify no sourceMessageId in inspiration (no message-inspiration association)"""
    issues = []
    for f in find_files(SRC_TS, '.ts') + find_files(SRC_TS, '.tsx'):
        content = open(f, encoding='utf-8').read()
        if 'sourceMessageId' in content or 'source_message_id' in content:
            rel = os.path.relpath(f, ROOT)
            issues.append(f"  {rel} - contains sourceMessageId/source_message_id")
    for f in find_files(SRC_RS, '.rs'):
        content = open(f, encoding='utf-8').read()
        if 'source_message_id' in content:
            rel = os.path.relpath(f, ROOT)
            issues.append(f"  {rel} - contains source_message_id")
    return issues

def check_no_agenthub():
    """Verify no agenthub references remain"""
    issues = []
    for f in find_files(ROOT, '.ts') + find_files(ROOT, '.tsx') + find_files(ROOT, '.rs') + find_files(ROOT, '.md'):
        rel = os.path.relpath(f, ROOT)
        if 'node_modules' in rel or 'dist' in rel or '.git' in rel:
            continue
        try:
            content = open(f, encoding='utf-8', errors='ignore').read()
        except:
            continue
        if re.search(r'agenthub', content, re.IGNORECASE):
            issues.append(f"  {rel} - contains 'agenthub'")
    return issues

def check_sidecar_stateless():
    """Verify sidecar has no state persistence"""
    sidecar_dir = os.path.join(ROOT, "sidecar")
    issues = []
    if os.path.exists(sidecar_dir):
        for f in find_files(sidecar_dir, '.ts'):
            content = open(f, encoding='utf-8').read()
            if 'fs.write' in content or 'writeFile' in content or 'sqlite' in content:
                rel = os.path.relpath(f, ROOT)
                issues.append(f"  {rel} - sidecar may have state persistence")
    return issues

print("=" * 60)
print("PilotDesk Self-Review Checklist")
print("=" * 60)

checks = [
    ("No TBD/TODO placeholders", check_no_tbd),
    ("Type definitions (Rust ↔ TS)", check_types_consistency),
    ("API key doesn't leave Rust layer", check_no_api_key_leak),
    ("No sourceMessageId in inspiration", check_no_source_message_id),
    ("No agenthub references remain", check_no_agenthub),
    ("Sidecar is stateless", check_sidecar_stateless),
]

all_pass = True
for name, fn in checks:
    print(f"\n[ ] {name}")
    issues = fn()
    if issues:
        all_pass = False
        print(f"    FAILED ({len(issues)} issue(s)):")
        for i in issues:
            print(i)
    else:
        print(f"    PASSED")

print(f"\n{'='*60}")
if all_pass:
    print("All checks PASSED")
else:
    print("Some checks FAILED - review the issues above")
print("=" * 60)
