"""Generate ground-truth bytes + expected decoded values for the 0x73 cyclic
report, using the real layout from
refs/decompiled/studio/zenstudiotb_report_format.py.

Run:  python3 gen_cyclic_gt.py
Output: prints the report hex and a JSON of expected field values.

The 0x73 report is pushed by the device on a timer. Its `seq` field holds the
CRC32 of the contents (bytes after the 16-byte header).
"""
import os
import ctypes
import json
import sys
import zlib


def cfield(name, t, bw):
    """Build a ctypes field tuple from the report-format spec."""
    if t.startswith("byte*"):
        return (name, ctypes.c_int8 * int(t[5:]))
    if t.startswith("ubyte*"):
        return (name, ctypes.c_uint8 * int(t[6:]))
    if bw is None:
        return (name, getattr(ctypes, "c_" + t))
    return (name, getattr(ctypes, "c_" + t), bw)


# Full 0x73 contents layout (mirrors zenstudiotb_report_format.py).
FIELDS = [
    ("current_preset", "ubyte", 3),
    ("power_on", "ubyte", 1),
    ("reserved1", "ubyte", 1),
    ("reserved1_0", "ubyte", 2),
    ("device_updated", "ubyte", 1),
    ("reserved2", "ubyte", 1),
    ("locked_wc", "ubyte", 1),
    ("locked_atomic", "ubyte", 1),
    ("usb_present", "ubyte", 1),
    ("reserved3", "ubyte", 1),
    ("adat_present", "ubyte", 1),
    ("spdif_present", "ubyte", 1),
    ("tbolt_present", "ubyte", 1),
    ("base_index", "byte", 4),
    ("reserved4", "ubyte", 2),
    ("reserved5", "ubyte", 2),
    ("sync_source", "ubyte", None),
    ("osc_freq_left", "ubyte", 2),
    ("osc_freq_right", "ubyte", 2),
    ("osc_level", "ubyte", 2),
    ("osc_mute_left", "ubyte", 1),
    ("osc_mute_right", "ubyte", 1),
    ("sync_freq_hi", "ubyte", None),
    ("sync_freq_mid", "ubyte", None),
    ("sync_freq_low", "ubyte", None),
    ("adc_trim", "ubyte", 3),
    ("reserved6", "ubyte", 1),
    ("reserved6_0", "ubyte", 4),
    ("spdif_src", "ubyte", 1),
    ("reserved7", "ubyte", 1),
    ("monitor_trim", "ubyte", 3),
    ("line_out_trim", "ubyte", 3),
    ("brightness", "ubyte", None),
    ("adat_avail_channels", "ubyte", None),
    ("monitor_vol", "ubyte", 7),
    ("monitor_mute", "ubyte", 1),
    ("hp1_vol", "ubyte", 7),
    ("hp1_mute", "ubyte", 1),
    ("hp2_vol", "ubyte", 7),
    ("hp2_mute", "ubyte", 1),
    ("line_out_vol", "ubyte", 7),
    ("line_out_mute", "ubyte", 1),
    ("reamp_vol", "ubyte", 7),
    ("reamp_mute", "ubyte", 1),
    ("preamp_gains", "byte*12", None),
    # preamps: struct array, count=12, each = pretype(4)|phantom(1)|hpf(1)|phase_inv(1)|zero_cross(1)
    ("preamps", {"fields": [
        ("pretype", "ubyte", 4),
        ("phantom", "ubyte", 1),
        ("hpf", "ubyte", 1),
        ("phase_inv", "ubyte", 1),
        ("zero_cross", "ubyte", 1),
    ], "count": 12}),
    ("reserved9", "ubyte", 2),
    ("hp1_enabled", "ubyte", 1),
    ("hp2_enabled", "ubyte", 1),
    ("mon_enabled", "ubyte", 1),
    ("talkback_on", "ubyte", 1),
    ("reserved10", "ubyte", 2),
    ("tb_mic_volume", "ubyte", None),
    ("reserved10_0", "ubyte*21", None),
    ("line_gains", "byte*8", None),
    ("adat_gains", "byte*16", None),
    ("spdif_gains", "byte*2", None),
    ("reserved12", "ubyte*26", None),
    ("pm_bank_src", "ubyte*4", None),
    ("peaks_meters", "ubyte*32", None),
    ("peaks_mixer", "ubyte*32", None),
    ("reserved13", "ubyte*16", None),
    ("peaks_adat", "ubyte*16", None),
    ("peaks_preamp", "ubyte*12", None),
    ("peaks_line", "ubyte*8", None),
    ("peaks_spdif", "ubyte*2", None),
    ("peaks_reverb", {"fields": [
        ("in_peaks", "ubyte*2", None),
        ("out_peaks", "ubyte*2", None),
    ], "count": 1}),
]


def build_contents():
    # Expand each spec into a ctypes field tuple.
    expanded = []
    for spec in FIELDS:
        if len(spec) == 2:
            # dict form: (name, {"fields": [...], "count": N})
            name, d = spec
            elem_fields = [cfield(fn, ft, fb) for (fn, ft, fb) in d["fields"]]

            class Elem(ctypes.Structure):
                _pack_ = 1
                _fields_ = elem_fields

            expanded.append((name, Elem * d["count"]))
        else:
            name, t, bw = spec
            expanded.append(cfield(name, t, bw))
    return expanded


class Header(ctypes.Structure):
    _pack_ = 1
    _fields_ = [
        ("cmd", ctypes.c_uint32),
        ("seq", ctypes.c_uint32),
        ("ext2", ctypes.c_uint32),
        ("ext3", ctypes.c_uint32),
    ]


class Contents(ctypes.Structure):
    _pack_ = 1
    _fields_ = build_contents()


class Report(ctypes.Structure):
    _pack_ = 1
    _fields_ = [("header", Header), ("contents", Contents)]


def set_values(r):
    c = r.contents
    c.current_preset = 5
    c.power_on = 1
    c.base_index = -1
    c.sync_source = 3
    c.osc_freq_left = 2
    c.osc_freq_right = 1
    c.osc_mute_left = 1
    c.monitor_vol = 100
    c.hp1_vol = 50
    Pg = ctypes.c_int8 * 12
    c.preamp_gains = Pg(*[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12])
    # preamps struct array: set element 0
    P = ctypes.c_uint8 * 12
    c.preamps[0].pretype = 7
    c.preamps[0].phantom = 1
    c.preamps[0].hpf = 0
    c.preamps[0].phase_inv = 1
    c.preamps[0].zero_cross = 1
    c.hp1_enabled = 1
    c.tb_mic_volume = 200 - 256  # -56 as signed byte
    Lg = ctypes.c_int8 * 8
    c.line_gains = Lg(*[-1, -2, -3, -4, -5, -6, -7, -8])


def read_back():
    """Read each field back into an expected-values dict."""
    r = Report()
    set_values(r)
    expected = {}
    c = r.contents
    for spec in FIELDS:
        name = spec[0]
        if len(spec) == 2:
            # dict form: struct array -> read element 0
            v = getattr(c, name)[0]
            inner = {}
            for (fn, ft, fb) in spec[1]["fields"]:
                ev = getattr(v, fn)
                if isinstance(ev, ctypes.Array):
                    ev = list(ev)
                inner[fn] = ev
            expected[name] = inner
            continue
        v = getattr(c, name)
        if isinstance(v, ctypes.Array):
            v = list(v)
        expected[name] = v
    return r, expected


def layout():
    """Return the 0x73 returns layout as a JSON-serializable list of field
    entries, exactly as it appears in zenstudiotb_report_format.py."""
    return [
        ["current_preset", "ubyte", 3],
        ["power_on", "ubyte", 1],
        ["reserved1", "ubyte", 1],
        ["reserved1_0", "ubyte", 2],
        ["device_updated", "ubyte", 1],
        ["reserved2", "ubyte", 1],
        ["locked_wc", "ubyte", 1],
        ["locked_atomic", "ubyte", 1],
        ["usb_present", "ubyte", 1],
        ["reserved3", "ubyte", 1],
        ["adat_present", "ubyte", 1],
        ["spdif_present", "ubyte", 1],
        ["tbolt_present", "ubyte", 1],
        ["base_index", "byte", 4],
        ["reserved4", "ubyte", 2],
        ["reserved5", "ubyte", 2],
        ["sync_source", "ubyte"],
        ["osc_freq_left", "ubyte", 2],
        ["osc_freq_right", "ubyte", 2],
        ["osc_level", "ubyte", 2],
        ["osc_mute_left", "ubyte", 1],
        ["osc_mute_right", "ubyte", 1],
        ["sync_freq_hi", "ubyte"],
        ["sync_freq_mid", "ubyte"],
        ["sync_freq_low", "ubyte"],
        ["adc_trim", "ubyte", 3],
        ["reserved6", "ubyte", 1],
        ["reserved6_0", "ubyte", 4],
        ["spdif_src", "ubyte", 1],
        ["reserved7", "ubyte", 1],
        ["monitor_trim", "ubyte", 3],
        ["line_out_trim", "ubyte", 3],
        ["brightness", "ubyte"],
        ["adat_avail_channels", "ubyte"],
        ["monitor_vol", "ubyte", 7],
        ["monitor_mute", "ubyte", 1],
        ["hp1_vol", "ubyte", 7],
        ["hp1_mute", "ubyte", 1],
        ["hp2_vol", "ubyte", 7],
        ["hp2_mute", "ubyte", 1],
        ["line_out_vol", "ubyte", 7],
        ["line_out_mute", "ubyte", 1],
        ["reamp_vol", "ubyte", 7],
        ["reamp_mute", "ubyte", 1],
        ["preamp_gains", "byte * 12"],
        ["preamps", {
            "fields": [
                ["pretype", "ubyte", 4],
                ["phantom", "ubyte", 1],
                ["hpf", "ubyte", 1],
                ["phase_inv", "ubyte", 1],
                ["zero_cross", "ubyte", 1],
            ],
            "count": 12,
        }],
        ["reserved9", "ubyte", 2],
        ["hp1_enabled", "ubyte", 1],
        ["hp2_enabled", "ubyte", 1],
        ["mon_enabled", "ubyte", 1],
        ["talkback_on", "ubyte", 1],
        ["reserved10", "ubyte", 2],
        ["tb_mic_volume", "ubyte"],
        ["reserved10_0", "ubyte * 21"],
        ["line_gains", "byte * 8"],
        ["adat_gains", "byte * 16"],
        ["spdif_gains", "byte * 2"],
        ["reserved12", "ubyte * 26"],
        ["pm_bank_src", "ubyte * 4"],
        ["peaks_meters", "ubyte * 32"],
        ["peaks_mixer", "ubyte * 32"],
        ["reserved13", "ubyte * 16"],
        ["peaks_adat", "ubyte * 16"],
        ["peaks_preamp", "ubyte * 12"],
        ["peaks_line", "ubyte * 8"],
        ["peaks_spdif", "ubyte * 2"],
        ["peaks_reverb", {
            "fields": [
                ["in_peaks", "ubyte * 2"],
                ["out_peaks", "ubyte * 2"],
            ],
            "count": 1,
        }],
    ]


def main():
    r, expected = read_back()
    r.header.cmd = 0x73  # cyclic report id
    contents = bytes(memoryview(r.contents).tobytes())
    crc = zlib.crc32(contents) & 0xFFFFFFFF
    r.header.seq = crc
    report = bytes(memoryview(r).tobytes())
    print("REPORT_HEX:", report.hex())
    print("CONTENTS_HEX:", contents.hex())
    print("REPORT_LEN:", len(report))
    print("CONTENTS_LEN:", len(contents))
    print("CRC32: 0x%08x" % crc)
    with open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "cyclic_gt.json"), "w") as f:
        json.dump(
            {"report": report.hex(), "layout": layout(), "expected": expected},
            f,
            indent=1,
        )
    print("WROTE cyclic_gt.json")


if __name__ == "__main__":
    main()
