#!/usr/bin/env python3
"""
elink8.py — talk to a Holtek e-Link8 Lite (and other e-Link / e-Writer)
programming dongles from Python.

Two layers are provided:

1. ``WCMD`` — drives Holtek's own *DOS Command Mode* programmer,
   ``WCMD.exe``, which ships inside the HOPE3000 installer (the same package
   that programs the e-Link8 Lite).  This is the documented, supported way
   to erase / program / verify / lock an MCU without the GUI, and it is what
   the ``flash`` sub-command uses.  Windows only (needs HOPE3000 installed).

2. ``ELinkUSB`` — a thin raw-USB transport (pyusb, or hidapi for HID
   interfaces) that finds the dongle by its USB IDs, dumps its descriptors
   and lets you send/receive raw packets.  The wire protocol between
   HOPE3000 and the dongle is *not* publicly documented; this layer exists
   for probing, for replaying packets captured with USBPcap/Wireshark, and
   as the place to add commands once you have captured them.

USB identifiers (from ``e-link.ini`` in HOPE3000 3.27)::

    vendor 0x04D9 (Holtek)
    application (normal) mode   PID 0x801A
    bootloader (firmware update) PIDs 0x800E, 0x8030, 0x8032
    8-bit e-Link family         PIDs 0x800C 0x800D 0x800E 0x8013 0x8014
                                     0x8016 0x801A 0x802B
    32-bit e-Link family        PIDs 0x8030 0x8031 (old), 0x8032..0x8035 (new)

Examples::

    python elink8.py probe                          # list Holtek USB devices
    python elink8.py descriptors                    # dump the dongle's descriptors
    python elink8.py flash firmware.MTP --lock      # erase+program+verify+lock via WCMD
    python elink8.py wcmd -T /W1                    # pass a raw WCMD command through
    python elink8.py raw --pid 0x801a --send "01 00 00" --read 64
    python elink8.py replay capture.txt             # replay captured packets

Requirements: Python 3.8+.  ``pip install pyusb`` for the raw layer
(``pip install hidapi`` if the interface turns out to be HID).  On Linux add
a udev rule such as
``SUBSYSTEM=="usb", ATTR{idVendor}=="04d9", MODE="0666"``.
"""

from __future__ import annotations

import argparse
import os
import re
import shutil
import subprocess
import sys
import time
from dataclasses import dataclass
from typing import Dict, Iterable, List, Optional, Sequence, Tuple

# ---------------------------------------------------------------------------
# Device identification
# ---------------------------------------------------------------------------

HOLTEK_VID = 0x04D9

#: PID -> description.  Taken from e-link.ini shipped with HOPE3000 V3.27.
KNOWN_PIDS: Dict[int, str] = {
    0x800C: "e-Link (8-bit family)",
    0x800D: "e-Link (8-bit family)",
    0x800E: "e-Link bootloader (8-bit)",
    0x8013: "e-Link (8-bit family)",
    0x8014: "e-Link (8-bit family)",
    0x8016: "e-Link (8-bit family)",
    0x801A: "e-Link application mode (HOPE3000 AP_ID)",
    0x802B: "e-Link (8-bit family)",
    0x8030: "e-Link32 bootloader (old)",
    0x8031: "e-Link32 (old)",
    0x8032: "e-Link32 bootloader (new)",
    0x8033: "e-Link32 (new)",
    0x8034: "e-Link32 (new)",
    0x8035: "e-Link32 (new)",
}

ELINK8_PIDS = (0x800C, 0x800D, 0x800E, 0x8013, 0x8014, 0x8016, 0x801A, 0x802B)
BOOTLOADER_PIDS = (0x800E, 0x8030, 0x8032)

#: Firmware "types" the e-Link can be loaded with (e-link.ini [EL_NAME]).
ELINK_FIRMWARE_TYPES = {
    0: "EIC-300",
    1: "Holtek 8-Bits MCU OCDS",
    2: "Holtek 8051 MCU OCDS",
    3: "Touch Key I2C Bridge",
    4: "Touch Panel SPI Bridge",
    5: "MotorII",
    6: "Tkw",
    7: "elink32_OCDS",
    8: "elink32_EIC300",
    9: "HT8 OCDS & 32Bit MCU BL",
    10: "HT8051 OCDS",
    11: "Voice Platform",
    12: "e-Link V2.0",
}


# ---------------------------------------------------------------------------
# Layer 1: WCMD.exe (HOPE3000 DOS Command Mode)
# ---------------------------------------------------------------------------

ROM_TYPES = ("Program", "Option", "Data", "Voice")


class WcmdError(RuntimeError):
    """A WCMD command reported an error."""


@dataclass
class WcmdResult:
    command: str
    returncode: int
    output: str

    @property
    def ok(self) -> bool:
        """Best-effort success detection.

        HOPE3000's console prints ``OK``/``Success`` on success and lines
        containing ``Fail``/``Error`` otherwise.  The process exit code has
        not been consistent across HOPE3000 versions (the release notes
        mention fixes to it), so the text is used as the primary signal.
        """
        text = self.output.lower()
        if re.search(r"\b(fail|failed|error|cannot|can't|invalid|not found)\b", text):
            return False
        return True


def _range_arg(prefix: str, ranges: Optional[Sequence[str]]) -> List[str]:
    """Build ``/PProgram=100h-2FFh,Option`` style arguments.

    ``ranges`` is a sequence like ``["Program=100h-2FFh", "Option", "Data"]``.
    """
    if not ranges:
        return []
    for r in ranges:
        rom = r.split("=", 1)[0]
        if rom.capitalize() not in ROM_TYPES:
            raise ValueError(f"unknown ROM type '{rom}' (expected one of {ROM_TYPES})")
    return [f"/{prefix}{','.join(ranges)}"]


class WCMD:
    """Wrapper around ``WCMD.exe`` (HOPE3000 DOS Command Mode).

    Every method maps to one documented WCMD command.  The dongle is
    selected with ``writer`` (1..8, as identified by :meth:`identify`).
    The programming data must first be loaded into the writer with
    :meth:`download` (an ``.MTP`` file produced by HT-IDE3000 or HOPE3000,
    or a ``.MEM`` EEPROM image); subsequent commands operate on that data.
    """

    DEFAULT_LOCATIONS = (
        r"C:\Program Files (x86)\HOLTEK\HOPE3000\WCMD.exe",
        r"C:\Program Files\HOLTEK\HOPE3000\WCMD.exe",
        r"C:\Program Files (x86)\Holtek Semiconductor\HOPE3000\WCMD.exe",
        r"C:\HOPE3000\WCMD.exe",
    )

    def __init__(self, exe: Optional[str] = None, writer: int = 1, timeout: float = 600.0, verbose: bool = False):
        self.exe = exe or self.find_exe()
        if not self.exe:
            raise WcmdError(
                "WCMD.exe not found; install HOPE3000 (for e-Link) or pass --wcmd PATH / set HOPE3000_WCMD"
            )
        if not 1 <= writer <= 8:
            raise ValueError("writer number must be 1..8")
        self.writer = writer
        self.timeout = timeout
        self.verbose = verbose

    @classmethod
    def find_exe(cls) -> Optional[str]:
        env = os.environ.get("HOPE3000_WCMD")
        if env and os.path.isfile(env):
            return env
        on_path = shutil.which("WCMD.exe") or shutil.which("WCMD")
        if on_path:
            return on_path
        for p in cls.DEFAULT_LOCATIONS:
            if os.path.isfile(p):
                return p
        return None

    # -- plumbing ---------------------------------------------------------

    def run(self, command: str, *params: str) -> WcmdResult:
        """Run ``WCMD -<command> <params...>`` and return its output."""
        args = [self.exe, f"-{command}", *params]
        if self.verbose:
            print("$", " ".join(args), file=sys.stderr)
        try:
            proc = subprocess.run(
                args,
                cwd=os.path.dirname(self.exe) or None,
                capture_output=True,
                text=True,
                timeout=self.timeout,
            )
        except subprocess.TimeoutExpired as e:
            raise WcmdError(f"WCMD -{command} timed out after {self.timeout}s") from e
        out = (proc.stdout or "") + (proc.stderr or "")
        res = WcmdResult(" ".join(args[1:]), proc.returncode, out.strip())
        if self.verbose:
            print(res.output, file=sys.stderr)
        return res

    def check(self, command: str, *params: str) -> WcmdResult:
        res = self.run(command, *params)
        if not res.ok:
            raise WcmdError(f"WCMD -{command} failed:\n{res.output}")
        return res

    def _w(self) -> str:
        return f"/W{self.writer}"

    # -- commands (see HOPE3000 user's guide, chapter 5) ----------------

    def identify(self) -> WcmdResult:
        """``-T``: flash the writer's LEDs and confirm it answers."""
        return self.check("T", self._w())

    def download(self, path: str, mcu: Optional[str] = None, package: Optional[str] = None,
                 lock_upload: Optional[bool] = None) -> WcmdResult:
        """``-D``: load an .MTP (or .MEM EEPROM image, needs ``mcu``) into the writer."""
        params = [f"/F{os.path.abspath(path)}"]
        if mcu:
            params.append(f"/M{mcu}")
        if package:
            params.append(f"/K{package}")
        if lock_upload is not None:
            params.append(f"/L{1 if lock_upload else 0}")
        params.append(self._w())
        return self.check("D", *params)

    def upload(self, path: Optional[str] = None) -> WcmdResult:
        """``-U``: save the writer's buffer (after :meth:`read`) to a file."""
        params = [f"/F{os.path.abspath(path)}"] if path else []
        return self.check("U", *params, self._w())

    def erase(self, ranges: Optional[Sequence[str]] = None) -> WcmdResult:
        """``-E``: erase the target (all ROMs, or e.g. ``["Program=100h-2FFh", "Option"]``)."""
        return self.check("E", *_range_arg("E", ranges), self._w())

    def blank_check(self, ranges: Optional[Sequence[str]] = None) -> WcmdResult:
        return self.check("B", *_range_arg("B", ranges), self._w())

    def program(self, ranges: Optional[Sequence[str]] = None) -> WcmdResult:
        """``-P``: program (includes an automatic verify)."""
        return self.check("P", *_range_arg("P", ranges), self._w())

    def verify(self, ranges: Optional[Sequence[str]] = None) -> WcmdResult:
        return self.check("V", *_range_arg("V", ranges), self._w())

    def lock(self, ranges: Optional[Sequence[str]] = None) -> WcmdResult:
        """``-L``: set the read-protection (lock) bits."""
        return self.check("L", *_range_arg("L", ranges), self._w())

    def read(self) -> WcmdResult:
        """``-R``: read the target into the writer's buffer (no partial read)."""
        return self.check("R", self._w())

    def write_words(self, rom: str, address: int, value_hex: str) -> WcmdResult:
        """``-W``: write up to 16 words directly, e.g. ``write_words("Program", 0x100, "01234567")``."""
        if rom.capitalize() not in ("Program", "Data"):
            raise ValueError("rom must be 'Program' or 'Data'")
        return self.check("W", f"/P{rom.capitalize()}={address:X}h:{value_hex}h", self._w())

    def checksum(self, path: str, rom_range_flag: int = 3) -> WcmdResult:
        """``-C``: checksum of a programming file (1=PROM, 2=+Option, 3=+Data/Voice)."""
        return self.check("C", f"/F{os.path.abspath(path)}", f"/R{rom_range_flag}")

    def packages(self, mcu: str) -> List[str]:
        """``-K``: package names known for an MCU (needed by e-WriterPro downloads)."""
        res = self.check("K", f"/M{mcu}")
        return [l.strip() for l in res.output.splitlines() if l.strip() and not l.lower().startswith(("command", "writer"))]

    def auto_setup(self, *settings: str) -> WcmdResult:
        """``-S``: store auto-programming settings (raw parameter strings)."""
        return self.check("S", *settings, self._w())

    def auto(self) -> WcmdResult:
        """``-A``: run the stored auto-programming sequence."""
        return self.check("A", self._w())

    # -- console mode ----------------------------------------------------

    def console(self, commands: Iterable[str]) -> str:
        """Run several commands inside one ``WCMD -CON`` session (faster).

        ``commands`` are given without the leading ``-``; ``Q`` is appended.
        Returns the combined console transcript.
        """
        script = "\n".join(list(commands) + ["Q"]) + "\n"
        if self.verbose:
            print("$ WCMD -CON <<EOF\n" + script + "EOF", file=sys.stderr)
        proc = subprocess.run(
            [self.exe, "-CON"],
            cwd=os.path.dirname(self.exe) or None,
            input=script,
            capture_output=True,
            text=True,
            timeout=self.timeout,
        )
        return (proc.stdout or "") + (proc.stderr or "")

    # -- convenience -----------------------------------------------------

    def flash(self, path: str, *, erase: bool = True, blank: bool = False, verify: bool = True,
              lock: bool = False, mcu: Optional[str] = None, package: Optional[str] = None) -> List[WcmdResult]:
        """Download, erase, (blank-check,) program, (verify,) (lock)."""
        results = [self.identify(), self.download(path, mcu=mcu, package=package)]
        if erase:
            results.append(self.erase())
        if blank:
            results.append(self.blank_check())
        results.append(self.program())
        if verify:
            results.append(self.verify())
        if lock:
            results.append(self.lock())
        return results


# ---------------------------------------------------------------------------
# Layer 2: raw USB access
# ---------------------------------------------------------------------------

def _import_usb():
    try:
        import usb.core  # type: ignore
        import usb.util  # type: ignore
    except ImportError as e:  # pragma: no cover
        raise SystemExit("pyusb is required for this command: pip install pyusb "
                         "(and libusb-1.0; on Windows the WinUSB driver installed by HOPE3000 works)") from e
    return usb


@dataclass
class FoundDevice:
    vid: int
    pid: int
    bus: int
    address: int
    manufacturer: Optional[str]
    product: Optional[str]
    serial: Optional[str]
    interfaces: List[Tuple[int, int, int, int]]  # (number, class, subclass, protocol)

    @property
    def description(self) -> str:
        return KNOWN_PIDS.get(self.pid, "unknown Holtek device")

    @property
    def is_bootloader(self) -> bool:
        return self.pid in BOOTLOADER_PIDS


def find_devices(pid: Optional[int] = None) -> List[FoundDevice]:
    """Enumerate Holtek USB devices (optionally a single PID)."""
    usb = _import_usb()
    found = []
    for dev in usb.core.find(find_all=True, idVendor=HOLTEK_VID):
        if pid is not None and dev.idProduct != pid:
            continue

        def s(idx):
            try:
                return usb.util.get_string(dev, idx) if idx else None
            except Exception:
                return None

        interfaces = []
        try:
            for cfg in dev:
                for intf in cfg:
                    interfaces.append((intf.bInterfaceNumber, intf.bInterfaceClass,
                                       intf.bInterfaceSubClass, intf.bInterfaceProtocol))
        except Exception:
            pass
        found.append(FoundDevice(dev.idVendor, dev.idProduct, dev.bus, dev.address,
                                 s(dev.iManufacturer), s(dev.iProduct), s(dev.iSerialNumber), interfaces))
    return found


class ELinkUSB:
    """Raw pyusb transport to an e-Link dongle.

    Opens the first interface (or ``interface``), claims it, and exposes the
    IN/OUT endpoints it finds.  Works for vendor-specific (WinUSB/libusb)
    interfaces and, on Linux/macOS, for HID interfaces too (the kernel
    driver is detached).  ``write``/``read`` use the first OUT/IN endpoint
    regardless of whether it is bulk or interrupt.
    """

    def __init__(self, pid: Optional[int] = None, interface: Optional[int] = None,
                 timeout_ms: int = 2000, serial: Optional[str] = None):
        usb = _import_usb()
        self._usb = usb
        candidates = list(usb.core.find(find_all=True, idVendor=HOLTEK_VID))
        if pid is not None:
            candidates = [d for d in candidates if d.idProduct == pid]
        else:
            candidates = [d for d in candidates if d.idProduct in ELINK8_PIDS] or candidates
        if serial:
            candidates = [d for d in candidates if usb.util.get_string(d, d.iSerialNumber) == serial]
        if not candidates:
            raise SystemExit("no e-Link found (is it plugged in / do you have permission?)")
        self.dev = candidates[0]
        self.timeout = timeout_ms
        try:
            self.dev.set_configuration()
        except usb.core.USBError:
            pass  # already configured (typical on Windows)
        cfg = self.dev.get_active_configuration()
        intf = None
        for i in cfg:
            if interface is None or i.bInterfaceNumber == interface:
                intf = i
                break
        if intf is None:
            raise SystemExit(f"interface {interface} not found")
        self.intf = intf
        self.ifnum = intf.bInterfaceNumber
        if os.name != "nt":
            try:
                if self.dev.is_kernel_driver_active(self.ifnum):
                    self.dev.detach_kernel_driver(self.ifnum)
            except (NotImplementedError, usb.core.USBError):
                pass
        usb.util.claim_interface(self.dev, self.ifnum)
        self.ep_out = [e for e in intf if usb.util.endpoint_direction(e.bEndpointAddress) == usb.util.ENDPOINT_OUT]
        self.ep_in = [e for e in intf if usb.util.endpoint_direction(e.bEndpointAddress) == usb.util.ENDPOINT_IN]

    def close(self) -> None:
        try:
            self._usb.util.release_interface(self.dev, self.ifnum)
        except Exception:
            pass
        try:
            self._usb.util.dispose_resources(self.dev)
        except Exception:
            pass

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.close()

    @property
    def is_hid(self) -> bool:
        return self.intf.bInterfaceClass == 3

    def endpoints(self) -> str:
        lines = []
        for e in list(self.ep_out) + list(self.ep_in):
            kind = {0: "control", 1: "isochronous", 2: "bulk", 3: "interrupt"}[e.bmAttributes & 3]
            d = "IN" if self._usb.util.endpoint_direction(e.bEndpointAddress) else "OUT"
            lines.append(f"  ep 0x{e.bEndpointAddress:02x} {d:<3} {kind:<9} maxpacket={e.wMaxPacketSize}")
        return "\n".join(lines)

    def write(self, data: bytes, ep: Optional[int] = None) -> int:
        if ep is None:
            if not self.ep_out:
                raise SystemExit("interface has no OUT endpoint; use control transfers")
            ep = self.ep_out[0].bEndpointAddress
        return self.dev.write(ep, data, timeout=self.timeout)

    def read(self, size: int = 64, ep: Optional[int] = None) -> bytes:
        if ep is None:
            if not self.ep_in:
                raise SystemExit("interface has no IN endpoint")
            ep = self.ep_in[0].bEndpointAddress
        return bytes(self.dev.read(ep, size, timeout=self.timeout))

    def control(self, request_type: int, request: int, value: int = 0, index: int = 0,
                data_or_length=None) -> bytes:
        r = self.dev.ctrl_transfer(request_type, request, value, index, data_or_length, timeout=self.timeout)
        return bytes(r) if not isinstance(r, int) else b""

    def hid_set_report(self, report: bytes, report_id: int = 0) -> int:
        """HID SET_REPORT (output report) over the control endpoint."""
        return self.dev.ctrl_transfer(0x21, 0x09, 0x0200 | report_id, self.ifnum, report, timeout=self.timeout)

    def hid_get_report(self, length: int = 64, report_id: int = 0) -> bytes:
        """HID GET_REPORT (input report) over the control endpoint."""
        return bytes(self.dev.ctrl_transfer(0xA1, 0x01, 0x0100 | report_id, self.ifnum, length, timeout=self.timeout))


def parse_hex(text: str) -> bytes:
    """Parse ``"01 0a ff"``, ``"010aff"`` or ``"0x01,0x0a"`` into bytes."""
    cleaned = re.sub(r"0x|[,\s:]", "", text.strip(), flags=re.I)
    if len(cleaned) % 2:
        raise ValueError("odd number of hex digits")
    return bytes.fromhex(cleaned)


def hexdump(data: bytes, width: int = 16) -> str:
    lines = []
    for off in range(0, len(data), width):
        chunk = data[off:off + width]
        hx = " ".join(f"{b:02x}" for b in chunk)
        asc = "".join(chr(b) if 32 <= b < 127 else "." for b in chunk)
        lines.append(f"{off:04x}: {hx:<{width * 3}} {asc}")
    return "\n".join(lines) or "(empty)"


def replay(link: ELinkUSB, script: str, pause: float = 0.0) -> None:
    """Replay a capture script.

    Format, one packet per line (``#`` comments allowed)::

        > 01 02 03 04        send to OUT endpoint
        < 64                 read up to 64 bytes from IN endpoint
        c 21 09 0200 0000 01 02   control transfer: bmRequestType bRequest wValue wIndex data...
        sleep 0.1

    Such a file is easy to produce from a USBPcap/Wireshark capture of
    HOPE3000 talking to the dongle (export the URB payloads).
    """
    for lineno, raw in enumerate(script.splitlines(), 1):
        line = raw.split("#", 1)[0].strip()
        if not line:
            continue
        op, _, rest = line.partition(" ")
        try:
            if op == ">":
                data = parse_hex(rest)
                n = link.write(data)
                print(f"[{lineno}] > sent {n} bytes")
            elif op == "<":
                size = int(rest or "64", 0)
                data = link.read(size)
                print(f"[{lineno}] < {len(data)} bytes\n{hexdump(data)}")
            elif op == "c":
                parts = rest.split()
                bm, br, wv, wi = (int(p, 16) for p in parts[:4])
                payload = parse_hex(" ".join(parts[4:])) if len(parts) > 4 else None
                if bm & 0x80 and payload is None:
                    resp = link.control(bm, br, wv, wi, 64)
                    print(f"[{lineno}] c in {len(resp)} bytes\n{hexdump(resp)}")
                else:
                    link.control(bm, br, wv, wi, payload)
                    print(f"[{lineno}] c out ok")
            elif op == "sleep":
                time.sleep(float(rest))
            else:
                print(f"[{lineno}] unknown op '{op}'", file=sys.stderr)
        except Exception as e:  # keep going, protocol exploration is messy
            print(f"[{lineno}] error: {e}", file=sys.stderr)
        if pause:
            time.sleep(pause)


# ---------------------------------------------------------------------------
# Command line
# ---------------------------------------------------------------------------

def cmd_probe(args) -> int:
    devs = find_devices(args.pid)
    if not devs:
        print("no Holtek (04d9) USB devices found")
        return 1
    for d in devs:
        mode = "BOOTLOADER" if d.is_bootloader else "application"
        print(f"{d.vid:04x}:{d.pid:04x}  bus {d.bus} addr {d.address}  {d.description} [{mode}]")
        print(f"    manufacturer={d.manufacturer!r} product={d.product!r} serial={d.serial!r}")
        for num, cls, sub, proto in d.interfaces:
            name = {3: "HID", 255: "vendor-specific (WinUSB/libusb)", 2: "CDC", 10: "CDC-data"}.get(cls, str(cls))
            print(f"    interface {num}: class {cls:#04x} ({name}) subclass {sub:#04x} protocol {proto:#04x}")
    return 0


def cmd_descriptors(args) -> int:
    usb = _import_usb()
    devs = list(usb.core.find(find_all=True, idVendor=HOLTEK_VID))
    if args.pid is not None:
        devs = [d for d in devs if d.idProduct == args.pid]
    if not devs:
        print("no matching device")
        return 1
    for d in devs:
        print(str(d))
        print()
    return 0


def cmd_raw(args) -> int:
    with ELinkUSB(pid=args.pid, interface=args.interface, timeout_ms=args.timeout) as link:
        print(f"opened {link.dev.idVendor:04x}:{link.dev.idProduct:04x} interface {link.ifnum}"
              f" ({'HID' if link.is_hid else 'vendor'})")
        print(link.endpoints())
        if args.send:
            data = parse_hex(args.send)
            if args.hid_report:
                n = link.hid_set_report(data)
            else:
                n = link.write(data)
            print(f"sent {n} bytes")
        if args.read:
            data = link.hid_get_report(args.read) if args.hid_report else link.read(args.read)
            print(f"read {len(data)} bytes")
            print(hexdump(data))
    return 0


def cmd_replay(args) -> int:
    with open(args.script, encoding="utf-8") as f:
        script = f.read()
    with ELinkUSB(pid=args.pid, interface=args.interface, timeout_ms=args.timeout) as link:
        replay(link, script, pause=args.pause)
    return 0


def _wcmd(args) -> WCMD:
    return WCMD(exe=args.wcmd, writer=args.writer, timeout=args.wcmd_timeout, verbose=args.verbose)


def cmd_flash(args) -> int:
    w = _wcmd(args)
    results = w.flash(args.file, erase=not args.no_erase, blank=args.blank, verify=not args.no_verify,
                      lock=args.lock, mcu=args.mcu, package=args.package)
    for r in results:
        print(f"[{r.command}] {'OK' if r.ok else 'FAILED'}")
        if args.verbose or not r.ok:
            print(r.output)
    return 0


def cmd_wcmd(args) -> int:
    w = _wcmd(args)
    if not args.args:
        print("pass WCMD arguments after '--', e.g. elink8.py wcmd -- -T /W1")
        return 2
    rest = list(args.args)
    if rest and rest[0] == "--":
        rest = rest[1:]
    if not rest:
        print("pass WCMD arguments after '--', e.g. elink8.py wcmd -- -T /W1")
        return 2
    first = rest[0]
    command = first[1:] if first.startswith(("-", "/")) else first
    res = w.run(command, *rest[1:])
    print(res.output)
    return 0 if res.ok else 1


def cmd_console(args) -> int:
    w = _wcmd(args)
    with open(args.script, encoding="utf-8") as f:
        cmds = [l.strip().lstrip("-") for l in f if l.strip() and not l.startswith("#")]
    print(w.console(cmds))
    return 0


def cmd_simple(method: str):
    def run(args) -> int:
        w = _wcmd(args)
        fn = getattr(w, method)
        kwargs = {}
        if method in ("erase", "blank_check", "program", "verify", "lock") and args.ranges:
            kwargs["ranges"] = args.ranges
        if method == "download":
            res = fn(args.file, mcu=args.mcu, package=args.package, lock_upload=args.lock_upload)
        elif method == "upload":
            res = fn(args.file)
        elif method == "checksum":
            res = fn(args.file, args.rom_range)
        elif method == "packages":
            for p in fn(args.mcu):
                print(p)
            return 0
        else:
            res = fn(**kwargs)
        print(res.output)
        return 0 if res.ok else 1
    return run


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0], formatter_class=argparse.RawDescriptionHelpFormatter,
                                epilog="Run 'elink8.py <command> -h' for details of each command.")
    sub = p.add_subparsers(dest="cmd", required=True)

    def add_usb(sp):
        sp.add_argument("--pid", type=lambda s: int(s, 0), default=None, help="USB product id (default: any e-Link8 PID)")
        sp.add_argument("--interface", type=int, default=None, help="interface number to claim")
        sp.add_argument("--timeout", type=int, default=2000, help="USB timeout in ms")

    sp = sub.add_parser("probe", help="list Holtek USB devices and classify them")
    sp.add_argument("--pid", type=lambda s: int(s, 0), default=None)
    sp.set_defaults(func=cmd_probe)

    sp = sub.add_parser("descriptors", help="dump full USB descriptors")
    sp.add_argument("--pid", type=lambda s: int(s, 0), default=None)
    sp.set_defaults(func=cmd_descriptors)

    sp = sub.add_parser("raw", help="send/receive raw packets")
    add_usb(sp)
    sp.add_argument("--send", help="hex bytes to send, e.g. '01 00 ff'")
    sp.add_argument("--read", type=int, default=0, metavar="N", help="read up to N bytes afterwards")
    sp.add_argument("--hid-report", action="store_true", help="use HID SET/GET_REPORT control transfers")
    sp.set_defaults(func=cmd_raw)

    sp = sub.add_parser("replay", help="replay a captured packet script (see replay() docstring)")
    add_usb(sp)
    sp.add_argument("script")
    sp.add_argument("--pause", type=float, default=0.0, help="seconds to wait between lines")
    sp.set_defaults(func=cmd_replay)

    def add_wcmd(sp):
        sp.add_argument("--wcmd", default=None, help="path to WCMD.exe (default: $HOPE3000_WCMD or the HOPE3000 install dir)")
        sp.add_argument("--writer", type=int, default=1, help="writer number 1..8 (see 'identify')")
        sp.add_argument("--wcmd-timeout", type=float, default=600.0)
        sp.add_argument("-v", "--verbose", action="store_true")

    sp = sub.add_parser("flash", help="download + erase + program + verify (+lock) an .MTP via WCMD")
    add_wcmd(sp)
    sp.add_argument("file", help=".MTP programming file (or .MEM with --mcu)")
    sp.add_argument("--mcu", help="MCU type, required for .MEM files (e.g. HT66F0185)")
    sp.add_argument("--package", help="package name (e-WriterPro only; see 'packages')")
    sp.add_argument("--no-erase", action="store_true")
    sp.add_argument("--no-verify", action="store_true")
    sp.add_argument("--blank", action="store_true", help="blank-check before programming")
    sp.add_argument("--lock", action="store_true", help="lock the chip after programming")
    sp.set_defaults(func=cmd_flash)

    for name, method, help_ in [
        ("identify", "identify", "flash the writer LEDs (-T)"),
        ("erase", "erase", "erase the target (-E)"),
        ("blank", "blank_check", "blank-check the target (-B)"),
        ("program", "program", "program the target from the writer buffer (-P)"),
        ("verify", "verify", "verify the target (-V)"),
        ("lock", "lock", "lock the target (-L)"),
        ("read", "read", "read the target into the writer buffer (-R)"),
    ]:
        sp = sub.add_parser(name, help=help_)
        add_wcmd(sp)
        if method in ("erase", "blank_check", "program", "verify", "lock"):
            sp.add_argument("ranges", nargs="*", help="ROM ranges, e.g. Program=100h-2FFh Option Data")
        sp.set_defaults(func=cmd_simple(method))

    sp = sub.add_parser("download", help="load a programming file into the writer (-D)")
    add_wcmd(sp)
    sp.add_argument("file")
    sp.add_argument("--mcu")
    sp.add_argument("--package")
    sp.add_argument("--lock-upload", dest="lock_upload", action="store_true", default=None)
    sp.set_defaults(func=cmd_simple("download"))

    sp = sub.add_parser("upload", help="save the writer buffer to a file (-U)")
    add_wcmd(sp)
    sp.add_argument("file", nargs="?")
    sp.set_defaults(func=cmd_simple("upload"))

    sp = sub.add_parser("checksum", help="checksum of a programming file (-C)")
    add_wcmd(sp)
    sp.add_argument("file")
    sp.add_argument("--rom-range", type=int, default=3, choices=(1, 2, 3))
    sp.set_defaults(func=cmd_simple("checksum"))

    sp = sub.add_parser("packages", help="list package names of an MCU (-K)")
    add_wcmd(sp)
    sp.add_argument("mcu")
    sp.set_defaults(func=cmd_simple("packages"))

    sp = sub.add_parser("wcmd", help="pass any WCMD command through, e.g. 'wcmd -- -T /W2'")
    add_wcmd(sp)
    sp.add_argument("args", nargs=argparse.REMAINDER)
    sp.set_defaults(func=cmd_wcmd)

    sp = sub.add_parser("console", help="run a file of WCMD commands in one -CON session")
    add_wcmd(sp)
    sp.add_argument("script")
    sp.set_defaults(func=cmd_console)
    return p


def main(argv: Optional[Sequence[str]] = None) -> int:
    args = build_parser().parse_args(argv)
    try:
        return args.func(args)
    except (WcmdError, ValueError) as e:
        if type(e).__name__ == "NoBackendError":  # pyusb's NoBackendError subclasses ValueError
            print("error: pyusb found no libusb backend. Install libusb-1.0 (Linux: apt install libusb-1.0-0; "
                  "macOS: brew install libusb; Windows: pip install libusb-package or use the WinUSB/libusbK "
                  "driver that HOPE3000 installs, plus libusb-1.0.dll on PATH).", file=sys.stderr)
            return 1
        print(f"error: {e}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
