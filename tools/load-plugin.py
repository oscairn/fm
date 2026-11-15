# SPDX-License-Identifier: AGPL-3.0-or-later
"""Loads the built plugin library as a host would: its clap_entry, init, the plugin factory, one
plugin made, initialised and destroyed. Usage: python3 tools/load-plugin.py LIBRARY"""
import ctypes as c
import sys

lib = c.CDLL(sys.argv[1])


class Version(c.Structure):
    _fields_ = [("major", c.c_uint32), ("minor", c.c_uint32), ("revision", c.c_uint32)]


class Entry(c.Structure):
    _fields_ = [("version", Version), ("init", c.CFUNCTYPE(c.c_bool, c.c_char_p)),
                ("deinit", c.CFUNCTYPE(None)), ("get_factory", c.CFUNCTYPE(c.c_void_p, c.c_char_p))]


class Descriptor(c.Structure):
    _fields_ = [("version", Version)] + [(n, c.c_char_p) for n in
                ("id", "name", "vendor", "url", "manual_url", "support_url", "version_", "description")]


class Factory(c.Structure):
    _fields_ = [("count", c.CFUNCTYPE(c.c_uint32, c.c_void_p)),
                ("descriptor", c.CFUNCTYPE(c.POINTER(Descriptor), c.c_void_p, c.c_uint32)),
                ("create", c.CFUNCTYPE(c.c_void_p, c.c_void_p, c.c_void_p, c.c_char_p))]


class Plugin(c.Structure):
    _fields_ = [("desc", c.c_void_p), ("data", c.c_void_p), ("init", c.CFUNCTYPE(c.c_bool, c.c_void_p)),
                ("destroy", c.CFUNCTYPE(None, c.c_void_p))]


class Host(c.Structure):
    _fields_ = [("version", Version), ("data", c.c_void_p)] + [(n, c.c_char_p) for n in
                ("name", "vendor", "url", "host_version")] + [(n, c.c_void_p) for n in
                ("get_extension", "request_restart", "request_process", "request_callback")]


entry = Entry.in_dll(lib, "clap_entry")
assert entry.version.major == 1 and entry.init(sys.argv[1].encode())
f = entry.get_factory(b"clap.plugin-factory")
factory = Factory.from_address(f)
assert factory.count(f) == 1
d = factory.descriptor(f, 0).contents
host = Host(Version(1, 2, 0), None, b"load-plugin", b"", b"", b"1")
p = factory.create(f, c.byref(host), d.id)
plugin = Plugin.from_address(p)
assert plugin.init(p)
plugin.destroy(p)
entry.deinit()
print("loaded", d.id.decode(), d.name.decode(), d.version_.decode())
