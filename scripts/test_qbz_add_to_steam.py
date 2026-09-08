#!/usr/bin/env python3
"""Unit tests for scripts/qbz-add-to-steam (binary VDF parser/serializer,
shortcut dedupe, appid formula, configset patching). Stdlib unittest only."""

import importlib.util
import struct
import unittest
import zlib
from importlib.machinery import SourceFileLoader
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent / "qbz-add-to-steam"
_loader = SourceFileLoader("qbz_add_to_steam", str(SCRIPT))
spec = importlib.util.spec_from_loader("qbz_add_to_steam", _loader)
qats = importlib.util.module_from_spec(spec)
spec.loader.exec_module(qats)


def make_fixture():
    """A real-world-shaped shortcuts.vdf: two entries (one non-QBZ with
    quirky/unknown fields), nested tags map, non-ASCII strings."""
    other_entry = [
        qats.fld_u32("appid", 0x81234567),
        qats.fld_str("appname", "Cool Game — déjà vu"),
        qats.fld_str("Exe", '"/opt/cool game/run.sh"'),
        qats.fld_str("StartDir", '"/opt/cool game/"'),
        qats.fld_str("icon", ""),
        qats.fld_str("ShortcutPath", ""),
        qats.fld_str("LaunchOptions", "gamemoderun %command%"),
        qats.fld_u32("IsHidden", 0),
        qats.fld_u32("AllowDesktopConfig", 1),
        qats.fld_u32("AllowOverlay", 1),
        qats.fld_u32("OpenVR", 0),
        qats.fld_u32("Devkit", 0),
        qats.fld_str("DevkitGameID", ""),
        qats.fld_u32("DevkitOverrideAppID", 0),
        qats.fld_u32("LastPlayTime", 1753000000),
        qats.fld_str("FlatpakAppID", ""),
        qats.fld_str("sortas", "cool game"),          # newer Steam field
        qats.fld_map("tags", [qats.fld_str("0", "favorite"),
                              qats.fld_str("1", "deck")]),
        qats.fld_str("SomeFutureField", "w\xe9ird\xff"),  # unknown field
    ]
    qbz_entry = qats.build_entry("/usr/bin/qbz", "env X=1 %command%",
                                 qats.shortcut_appid("/usr/bin/qbz"))
    fields = [qats.fld_map("shortcuts", [
        qats.fld_map("0", other_entry),
        qats.fld_map("1", qbz_entry),
    ])]
    return fields


class TestRoundTrip(unittest.TestCase):
    def test_roundtrip_byte_identical(self):
        fields = make_fixture()
        blob = qats.serialize_vdf(fields, True)
        parsed, ended = qats.parse_vdf(blob)
        self.assertTrue(ended)
        self.assertEqual(qats.serialize_vdf(parsed, ended), blob)

    def test_roundtrip_preserves_trailing_end_marker(self):
        fields = make_fixture()
        with_root_end = qats.serialize_vdf(fields, True)
        without_root_end = qats.serialize_vdf(fields, False)
        self.assertEqual(with_root_end, without_root_end + b"\x08")
        p1, e1 = qats.parse_vdf(with_root_end)
        p2, e2 = qats.parse_vdf(without_root_end)
        self.assertTrue(e1)
        self.assertFalse(e2)
        self.assertEqual(p1, p2)

    def test_real_file_shape(self):
        # header is \x00shortcuts\x00, file ends with \x08\x08
        blob = qats.serialize_vdf(make_fixture(), True)
        self.assertTrue(blob.startswith(b"\x00shortcuts\x00"))
        self.assertTrue(blob.endswith(b"\x08\x08"))

    def test_parse_types(self):
        fields, _ = qats.parse_vdf(qats.serialize_vdf(make_fixture(), True))
        top = qats.field_get(fields, "shortcuts")
        self.assertIsNotNone(top)
        self.assertEqual(top[0], qats.T_MAP)
        e0 = top[2][0][2]
        appid = qats.field_get(e0, "appid")
        self.assertEqual(appid[0], qats.T_U32)
        self.assertEqual(struct.unpack("<I", appid[2])[0], 0x81234567)
        tags = qats.field_get(e0, "tags")
        self.assertEqual(tags[0], qats.T_MAP)
        self.assertEqual(qats.field_get(tags[2], "0")[2], b"favorite")

    def test_garbage_rejected(self):
        with self.assertRaises(ValueError):
            qats.parse_vdf(b"\x00shortcuts\x00\x07bogus\x00")


class TestDedupe(unittest.TestCase):
    def test_update_in_place_never_duplicate(self):
        fields = make_fixture()
        blob0 = qats.serialize_vdf(fields, True)
        parsed, ended = qats.parse_vdf(blob0)
        new_fields, action = qats.upsert_entry(
            parsed, "/usr/bin/qbz", "env NEW=1 %command%",
            qats.shortcut_appid("/usr/bin/qbz"))
        self.assertEqual(action, "updated")
        top = qats.field_get(new_fields, "shortcuts")
        # still exactly two entries, QBZ still at key "1"
        self.assertEqual(len(top[2]), 2)
        self.assertEqual(top[2][1][1], b"1")
        lo = qats.field_get(top[2][1][2], "LaunchOptions")
        self.assertEqual(lo[2], b"env NEW=1 %command%")
        # the non-QBZ entry survives byte-identically: re-serialize and diff
        blob1 = qats.serialize_vdf(new_fields, ended)
        entry0_start = blob0.index(b"\x000\x00")
        entry0_end = blob0.index(b"\x001\x00")
        self.assertEqual(blob0[entry0_start:entry0_end],
                         blob1[entry0_start:entry0_end])

    def test_add_when_missing(self):
        fields = make_fixture()
        parsed, ended = qats.parse_vdf(qats.serialize_vdf(fields, True))
        removed = qats.remove_entry(parsed, "/usr/bin/qbz")
        self.assertTrue(removed)
        new_fields, action = qats.upsert_entry(
            parsed, "/usr/bin/qbz", "env X=1 %command%",
            qats.shortcut_appid("/usr/bin/qbz"))
        self.assertEqual(action, "added")
        top = qats.field_get(new_fields, "shortcuts")
        self.assertEqual(len(top[2]), 2)
        # appended reusing the freed index (indices stay contiguous)
        self.assertEqual(top[2][-1][1], b"1")

    def test_dedupe_on_exe_and_name(self):
        # an entry with the same Exe but a different appname must NOT be
        # replaced — a new entry is appended instead
        fields = make_fixture()
        top = qats.field_get(fields, "shortcuts")
        # drop the real QBZ entry, keep only a "twin": same Exe, other name
        top[2][:] = [e for e in top[2] if e[1] != b"1"]
        twin = qats.build_entry("/usr/bin/qbz", "%command%", 0x81234568)
        for i, f in enumerate(twin):
            if f[1] == b"appname":
                twin[i] = qats.fld_str("appname", "QBZ (alt)")
        top[2].append(qats.fld_map("1", twin))
        parsed, _ = qats.parse_vdf(qats.serialize_vdf(fields, True))
        new_fields, action = qats.upsert_entry(
            parsed, "/usr/bin/qbz", "env X=1 %command%",
            qats.shortcut_appid("/usr/bin/qbz"))
        self.assertEqual(action, "added")
        top = qats.field_get(new_fields, "shortcuts")
        self.assertEqual(len(top[2]), 3)
        # the twin entry is untouched
        self.assertEqual(qats.field_get(top[2][1][2], "appname")[2],
                         "QBZ (alt)".encode("utf-8"))

    def test_remove_missing_is_noop(self):
        fields = make_fixture()
        blob0 = qats.serialize_vdf(fields, True)
        parsed, _ = qats.parse_vdf(blob0)
        self.assertFalse(qats.remove_entry(parsed, "/opt/other"))
        self.assertEqual(qats.serialize_vdf(parsed, True), blob0)


class TestAppId(unittest.TestCase):
    def test_formula(self):
        exe = "/usr/bin/qbz"
        expected = (zlib.crc32(('"' + exe + '"QBZ').encode("utf-8"))
                    | 0x80000000) & 0xFFFFFFFF
        self.assertEqual(qats.shortcut_appid(exe), expected)

    def test_known_vector(self):
        # pinned so a regression in quoting/encoding is caught
        self.assertEqual(qats.shortcut_appid("/usr/bin/qbz"), 3124044104)

    def test_fits_u32_and_top_bit(self):
        appid = qats.shortcut_appid("/usr/bin/qbz")
        self.assertTrue(0 <= appid <= 0xFFFFFFFF)
        self.assertTrue(appid & 0x80000000)
        struct.pack("<I", appid)  # must not raise

    def test_launch_url(self):
        appid = 3124044104
        url = qats.launch_url(appid)
        self.assertEqual(url, "steam://rungameid/%d"
                         % ((appid << 32) | 0x02000000))


class TestEntryFields(unittest.TestCase):
    def test_standard_field_order(self):
        entry = qats.build_entry("/usr/bin/qbz", "env X=1 %command%", 123)
        keys = [k.decode() for _, k, _ in entry]
        self.assertEqual(keys, [
            "appid", "appname", "Exe", "StartDir", "icon", "ShortcutPath",
            "LaunchOptions", "IsHidden", "AllowDesktopConfig", "AllowOverlay",
            "OpenVR", "Devkit", "DevkitGameID", "DevkitOverrideAppID",
            "LastPlayTime", "FlatpakAppID", "tags",
        ])

    def test_entry_values(self):
        appid = qats.shortcut_appid("/usr/bin/qbz")
        entry = qats.build_entry("/usr/bin/qbz", "env X=1 %command%", appid)
        self.assertEqual(qats.field_get(entry, "appname")[2], b"QBZ")
        self.assertEqual(qats.field_get(entry, "Exe")[2], b'"/usr/bin/qbz"')
        self.assertEqual(qats.field_get(entry, "StartDir")[2], b'"/usr/bin/"')
        self.assertEqual(qats.field_get(entry, "icon")[2],
                         b"/usr/share/icons/hicolor/512x512/apps/"
                         b"com.blitzfc.qbz.png")
        self.assertEqual(struct.unpack(
            "<I", qats.field_get(entry, "appid")[2])[0], appid)
        self.assertEqual(struct.unpack(
            "<I", qats.field_get(entry, "AllowDesktopConfig")[2])[0], 1)
        self.assertEqual(struct.unpack(
            "<I", qats.field_get(entry, "AllowOverlay")[2])[0], 1)
        self.assertEqual(struct.unpack(
            "<I", qats.field_get(entry, "IsHidden")[2])[0], 0)
        self.assertEqual(struct.unpack(
            "<I", qats.field_get(entry, "OpenVR")[2])[0], 0)


class TestConfigsetPatch(unittest.TestCase):
    EXISTING = ('"controller_config"\n{\n\t"12345"\n\t{\n'
                '\t\t"template"\t\t"other.vdf"\n\t}\n}\n')

    def test_insert_into_existing(self):
        out = qats._patch_configset_text(self.EXISTING, 3124044104, False)
        self.assertIn('"3124044104"', out)
        self.assertIn(qats.LAYOUT_NAME, out)
        self.assertIn('"12345"', out)  # pre-existing entry preserved
        self.assertEqual(out.count("{"), out.count("}"))

    def test_replace_existing_entry(self):
        first = qats._patch_configset_text(self.EXISTING, 3124044104, False)
        second = qats._patch_configset_text(first, 3124044104, False)
        self.assertEqual(second.count('"3124044104"'), 1)  # idempotent
        self.assertIn('"12345"', second)

    def test_remove_entry(self):
        first = qats._patch_configset_text(self.EXISTING, 3124044104, False)
        out = qats._patch_configset_text(first, 3124044104, True)
        self.assertNotIn('"3124044104"', out)
        self.assertIn('"12345"', out)

    def test_bad_format_returns_none(self):
        self.assertIsNone(qats._patch_configset_text(
            '"something_else"\n{\n}\n', 1, False))
        self.assertIsNone(qats._patch_configset_text(
            '"controller_config"\n{\n', 1, False))  # unbalanced braces


if __name__ == "__main__":
    unittest.main()
