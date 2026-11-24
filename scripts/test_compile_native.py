#!/usr/bin/env python3
"""Tests for compile-native's prologue strip and framing. Run: python3 scripts/test_compile_native.py"""
import importlib.util
import struct
from pathlib import Path

spec = importlib.util.spec_from_file_location("cn", Path(__file__).with_name("compile-native.py"))
cn = importlib.util.module_from_spec(spec)
spec.loader.exec_module(cn)


def vnop():
    # A VOP3 v_nop: encoding 0xD5800000 (VOP3, opcode 0x180) + 8-byte literal.
    return struct.pack("<I", 0xD5800000) + b"\x00" * 8


def test_strip_counts_all_leading_vnops():
    # 0, 1, 2, 3 (the 36-byte case), 4 leading v_nops, each followed by real code.
    real = struct.pack("<I", 0xBFA00003)  # s_inst_prefetch
    for n in (0, 1, 2, 3, 4):
        code = vnop() * n + real
        stripped, off = cn.strip_vnop_prologue(code)
        assert off == 12 * n, f"n={n}: stripped {off}, want {12 * n}"
        assert stripped == real, f"n={n}: residual {stripped.hex()}"
    # The 36-byte case specifically (3 v_nops) must not leave a v_nop behind.
    code = vnop() * 3 + real
    stripped, off = cn.strip_vnop_prologue(code)
    assert off == 36 and stripped[:4] == real, "36-byte prologue must strip fully"


def test_strip_stops_at_first_non_vnop():
    real = struct.pack("<I", 0xBFA00003)
    # A v_nop, then real code, then another v_nop (must NOT be stripped).
    code = vnop() + real + vnop()
    stripped, off = cn.strip_vnop_prologue(code)
    assert off == 12 and stripped == real + vnop()


def test_framing_embeds_code_after_header():
    # The framed blob must carry the FSSM header AND the code at code_off=0x148,
    # so the translator's encoding cross-check reads real code (not zeros).
    code = struct.pack("<I", 0xBFA00003) + struct.pack("<I", 0xBF810000)
    header = cn.compute_header((1, 1, 1), 0, True)
    code_off = 0x148
    framed = bytearray(header) + bytearray(code_off + len(code) - len(header))
    struct.pack_into("<I", framed, 0xC0, code_off - 0x28)
    framed[code_off:code_off + len(code)] = code
    assert framed[:4] == b"FSSM"
    assert framed[code_off:code_off + 4] == struct.pack("<I", 0xBFA00003)
    # code_range start = u32@0xC0 + 0x28 must equal code_off.
    assert struct.unpack_from("<I", framed, 0xC0)[0] + 0x28 == code_off


if __name__ == "__main__":
    test_strip_counts_all_leading_vnops()
    test_strip_stops_at_first_non_vnop()
    test_framing_embeds_code_after_header()
    print("all compile-native tests passed")