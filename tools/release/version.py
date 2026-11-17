#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
"""The plugin's own version, read from its source (no duplicated constant).

Reads the clap_plugin_descriptor_t version field out of PLUGIN_VERSION_FILE in
release.conf (a cargo plugin: PLUGIN_SOURCE_FILE is also set, see rust_descriptor). Used by manifest.py (to cross-check the tag) and by the release
workflow (a dry run has no tag, so it must derive one).
"""
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from release_conf import REPO, conf

DESCRIPTOR = re.compile(
    r'CLAP_VERSION,\s*'
    r'"(?P<id>[^"]*)",\s*"(?P<name>[^"]*)",\s*"(?P<vendor>[^"]*)",\s*"(?P<url>[^"]*)",\s*'
    r'"[^"]*",\s*"[^"]*",\s*"(?P<version>[^"]*)"')


def rust_descriptor():
    """A cargo plugin: the descriptor's version is its crate's ([package] version in
    PLUGIN_VERSION_FILE), its id and name the PLUGIN_ID and PLUGIN_NAME constants of PLUGIN_SOURCE_FILE."""
    c = conf()
    toml = open(os.path.join(REPO, c['PLUGIN_VERSION_FILE']), encoding='utf-8').read()
    source = open(os.path.join(REPO, c['PLUGIN_SOURCE_FILE']), encoding='utf-8').read()
    package = re.search(r'^\[package\]\s*$(.*?)(?=^\[|\Z)', toml, re.M | re.S)
    found = [package and re.search(r'^version = "([^"]*)"', package.group(1), re.M)]
    found += [re.search(rf'\b{k}: &CStr = c"([^"]*)"', source) for k in ('PLUGIN_ID', 'PLUGIN_NAME')]
    if not all(found):
        sys.exit(f"version.py: no version/PLUGIN_ID/PLUGIN_NAME in {c['PLUGIN_VERSION_FILE']} and {c['PLUGIN_SOURCE_FILE']}")
    return {'version': found[0].group(1), 'id': found[1].group(1), 'name': found[2].group(1)}


def descriptor():
    if conf().get('PLUGIN_SOURCE_FILE'):
        return rust_descriptor()
    text = open(os.path.join(REPO, conf()['PLUGIN_VERSION_FILE']), encoding='utf-8').read()
    m = DESCRIPTOR.search(text)
    if not m:
        sys.exit(f"version.py: no clap_plugin_descriptor_t found in {conf()['PLUGIN_VERSION_FILE']}")
    return m.groupdict()


if __name__ == '__main__':
    print(descriptor()['version'])
