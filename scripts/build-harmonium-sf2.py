#!/usr/bin/env python3
"""Build "Cadence Harmonium.sf2" from real harmonium recordings.

Usage:
    python3 scripts/build-harmonium-sf2.py [--out FILE] [--src-cache DIR]
                                           [--check] [--force] [--unpinned]
                                           [--verbose] [--quiet]

Downloads the 38 pinned harmonium notes (mono 16-bit 44.1 kHz WAV, C2..D5)
listed in packaging/soundfont/harmonium-pin.txt into a cache folder
(default .cache/soundfont-src/harmonium, or $CADENCE_HARMONIUM_SRC_CACHE),
verifies every SHA-256 (a mismatch is always fatal and deletes the file),
and writes a SoundFont 2 file with one preset, "Harmonium" (bank 0,
program 20 = GM Reed Organ), holding one sample zone per note:

- the root key comes from the file name, and a per-note pitch correction
  (measured by autocorrelation) tunes each reed to A440;
- the leading silence and the slow bellows ramp are trimmed; each sample
  keeps its natural attack plus one sustain loop of 0.45-1.1 s from the
  steady part of the note (1-2.5 s of audio per note). The loop is chosen
  by sliding a ~2-period waveform template around each candidate start
  over every end position (on a 16x-decimated copy, then refined at full
  rate), scored by waveform difference plus level mismatch and level
  swing inside the loop; the end of the loop is then crossfaded (50 ms)
  into the samples before its start, so the wrap continues the waveform;
- the zones' key ranges split the gaps between recorded notes (the lowest
  zone reaches down to key 0 and the highest up to 127);
- a 5 ms attack and a 0.2 s release on the volume envelope.

The source samples are by Nicholaus P. Brosowsky (tonejs-instruments,
CC BY 3.0), edited from Donya Quick's CC0 recording; see
packaging/soundfont/Cadence-Harmonium-LICENSE.txt.

The output is deterministic (integer math and IEEE-754 doubles only), and its
SHA-256 is pinned (HARMONIUM_SHA256): a build that doesn't reproduce those
bytes fails. --check rebuilds in memory and compares with the existing output
without writing anything. Python 3 standard library only.

Default output: $CADENCE_SOUNDFONT_CACHE (else .cache/soundfonts in the repo)
/ "Cadence Harmonium.sf2". When the output already has the pinned hash the
build is skipped (use --force to rebuild anyway).

Exit codes: 0 ok, 1 download failed / --check mismatch, 2 bad arguments,
3 checksum mismatch (a source file, or the built font against the pin).
"""

import argparse
import hashlib
import math
import operator
import os
import struct
import sys
import time
import urllib.request
import wave
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PIN = ROOT / "packaging" / "soundfont" / "harmonium-pin.txt"

RATE = 44100
NOTE_OFFSETS = {
    "C": 0, "Cs": 1, "D": 2, "Ds": 3, "E": 4, "F": 5,
    "Fs": 6, "G": 7, "Gs": 8, "A": 9, "As": 10, "B": 11,
}

# Analysis frame: 10 ms.
FRAME = 441
# Volume envelope, in SoundFont timecents (1200 * log2(seconds)).
ATTACK_TIMECENTS = -9173   # 5 ms
RELEASE_TIMECENTS = -2786  # 0.2 s
GM_PROGRAM = 20            # Reed Organ
# Loop search ranges, in frames.
LOOP_MIN_FRAMES = 45       # 0.45 s
LOOP_MAX_FRAMES = 110
START_SPAN_FRAMES = 60
CROSSFADE = 2205           # 50 ms
DECIMATE = 16              # coarse loop search resolution (samples)
REFINE = 12                # coarse matches refined at full rate
LOOP_WINDOW_MIN = 1104     # matched waveform window: 2 periods, 25-50 ms
LOOP_WINDOW_MAX = 2208
FADE_IN = 220              # 5 ms
GUARD = 8                  # samples after the loop (copy of its start)
PAD = 46                   # zero samples after each sample (SF2 spec)


class Fatal(Exception):
    def __init__(self, message, code):
        super().__init__(message)
        self.code = code


def log(msg, quiet=False):
    if not quiet:
        print(msg, flush=True)


# ----------------------------------------------------------------------------
# Pin file and sources
# ----------------------------------------------------------------------------

def read_pin():
    values = {}
    samples = []
    for raw in PIN.read_text(encoding="utf-8").splitlines():
        line = raw.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        key, value = key.strip(), value.strip()
        if key == "SAMPLE":
            parts = value.split()
            if len(parts) != 3:
                raise Fatal(f"bad SAMPLE line in {PIN}: {raw}", 2)
            samples.append((parts[0], parts[1].lower(), int(parts[2])))
        else:
            values[key] = value
    for key in ("HARMONIUM_BASE_URL", "HARMONIUM_FILE", "HARMONIUM_SHA256"):
        if not values.get(key):
            raise Fatal(f"{key} missing from {PIN}", 2)
    if not samples:
        raise Fatal(f"no SAMPLE lines in {PIN}", 2)
    return values, samples


def sha256_bytes(data):
    return hashlib.sha256(data).hexdigest()


def fetch_sources(values, samples, cache, quiet):
    """Returns {file name: bytes}, downloading what the cache lacks."""
    cache.mkdir(parents=True, exist_ok=True)
    base = values["HARMONIUM_BASE_URL"].rstrip("/") + "/"
    out = {}
    fetched = 0
    for name, sha, size in samples:
        path = cache / name
        if path.is_file():
            data = path.read_bytes()
            if sha256_bytes(data) == sha:
                out[name] = data
                continue
            path.unlink()
        url = base + name
        data = None
        for attempt in range(3):
            try:
                with urllib.request.urlopen(url, timeout=30) as resp:
                    data = resp.read()
                break
            except Exception as err:  # noqa: BLE001 - report any network error
                if attempt == 2:
                    raise Fatal(f"could not download {url}: {err}", 1) from err
                time.sleep(2)
        got = sha256_bytes(data)
        if got != sha or len(data) != size:
            raise Fatal(
                f"checksum mismatch for {url}\n  expected {sha} ({size} bytes)\n"
                f"  got      {got} ({len(data)} bytes)", 3)
        tmp = cache / f".{name}.{os.getpid()}.tmp"
        tmp.write_bytes(data)
        os.replace(tmp, path)
        out[name] = data
        fetched += 1
    log(f"Harmonium samples: {len(out)} verified ({fetched} downloaded) in {cache}", quiet)
    return out


def midi_key(name):
    stem = name.rsplit(".", 1)[0]
    letter, octave = stem[:-1], stem[-1]
    if letter not in NOTE_OFFSETS or not octave.isdigit():
        raise Fatal(f"unexpected sample name {name}", 2)
    return 12 * (int(octave) + 1) + NOTE_OFFSETS[letter]


def read_wav(name, data):
    import io
    with wave.open(io.BytesIO(data)) as w:
        if (w.getnchannels(), w.getsampwidth(), w.getframerate()) != (1, 2, RATE):
            raise Fatal(f"{name}: expected mono 16-bit {RATE} Hz", 3)
        frames = w.readframes(w.getnframes())
    n = len(frames) // 2
    return list(struct.unpack(f"<{n}h", frames[: 2 * n]))


# ----------------------------------------------------------------------------
# Analysis
# ----------------------------------------------------------------------------

def frame_rms(x):
    out = []
    for i in range(0, len(x) - FRAME + 1, FRAME):
        seg = x[i:i + FRAME]
        out.append(math.sqrt(sum(v * v for v in seg) / FRAME))
    return out


def smooth(values, radius):
    out = []
    n = len(values)
    for i in range(n):
        lo, hi = max(0, i - radius), min(n, i + radius + 1)
        out.append(sum(values[lo:hi]) / (hi - lo))
    return out


def distance(x, a, b, width):
    """Normalized squared difference of x[a:a+width] and x[b:b+width]
    (0 = identical, ~1 = uncorrelated)."""
    u = x[a:a + width]
    v = x[b:b + width]
    den = sum(map(operator.mul, u, u)) + sum(map(operator.mul, v, v))
    num = den - 2 * sum(map(operator.mul, u, v))
    return num / den if den else 1.0


def measure_period(x, at, nominal):
    """Sub-sample period near `nominal` by normalized autocorrelation."""
    width = 4096
    lo = int(nominal * 0.96)
    hi = int(nominal * 1.04) + 1
    seg = x[at:at + width + hi + 2]
    energy0 = sum(v * v for v in seg[:width])

    def corr(lag):
        num = 0
        e1 = 0
        for i in range(width):
            v = seg[i + lag]
            num += seg[i] * v
            e1 += v * v
        return num / math.sqrt(energy0 * e1) if energy0 and e1 else 0.0

    vals = {lag: corr(lag) for lag in range(lo - 1, hi + 2)}
    best = max(range(lo, hi + 1), key=lambda lag: (vals[lag], -lag))
    a, b, c = vals[best - 1], vals[best], vals[best + 1]
    den = a - 2 * b + c
    offset = 0.5 * (a - c) / den if den else 0.0
    return best + max(-0.5, min(0.5, offset)), b


def analyze(name, x):
    key = midi_key(name)
    nominal = RATE / (440.0 * 2 ** ((key - 69) / 12))
    env = frame_rms(x)
    ref_frames = sorted(env[100:500]) or sorted(env)
    ref = ref_frames[len(ref_frames) // 2]
    # Sample start: where the note reaches 10% of its steady level.
    onset_frame = next(i for i, v in enumerate(env) if v >= 0.1 * ref)
    onset = onset_frame * FRAME
    senv = smooth(env, 5)
    # Steady part: smoothed level within 10% of the steady level, at least
    # 0.25 s after the onset, plus 0.1 s.
    steady = next(
        (i for i in range(onset_frame + 25, len(senv)) if senv[i] >= 0.9 * ref),
        onset_frame + 40)
    steady += 10

    # Loop search on a decimated copy (sums of DECIMATE samples, ~5.5 kHz):
    # for every start candidate, slide a template of the waveform around the
    # start over every end position in the allowed loop-length range and
    # score the normalized squared difference, plus the smoothed-level
    # mismatch. All sums are integers, so the choice is exact.
    period = nominal
    width = max(LOOP_WINDOW_MIN, min(LOOP_WINDOW_MAX, 2 * int(period + 0.5)))
    d = DECIMATE
    xd = [sum(x[i:i + d]) for i in range(0, len(x) - d + 1, d)]
    sq = [0]
    for v in xd:
        sq.append(sq[-1] + v * v)
    wd = width // d
    hd = wd // 2
    found = []
    for a in range(steady, steady + START_SPAN_FRAMES, 3):
        sd = a * FRAME // d
        tmpl = xd[sd - hd:sd - hd + wd]
        et = sq[sd - hd + wd] - sq[sd - hd]
        # Level swing inside a loop ending at frame b (a loop over a bellows
        # swell would pulse at the loop rate).
        swing = []
        lo_v = hi_v = senv[a]
        for f in range(a, min(len(senv), a + LOOP_MAX_FRAMES + 2)):
            lo_v = min(lo_v, senv[f])
            hi_v = max(hi_v, senv[f])
            swing.append((hi_v - lo_v) / ref)
        lo = sd + LOOP_MIN_FRAMES * FRAME // d
        hi = min(sd + LOOP_MAX_FRAMES * FRAME // d, len(xd) - wd - 2 * FRAME)
        for ed in range(lo, hi):
            ee = sq[ed - hd + wd] - sq[ed - hd]
            dot = sum(map(operator.mul, tmpl, xd[ed - hd:ed - hd + wd]))
            den = et + ee
            wave_cost = (den - 2 * dot) / den if den else 1.0
            b = ed * d // FRAME
            env_cost = abs(senv[a] - senv[b]) / ref + 0.5 * swing[b - a]
            found.append((wave_cost + 0.5 * env_cost, sd * d, ed * d, env_cost))
    found.sort()
    # Full-rate refinement of the best few coarse matches.
    half = width // 2
    best = None
    for _, start, coarse, env_cost in found[:REFINE]:
        end = min(
            range(coarse - d, coarse + d + 1),
            key=lambda e: (distance(x, start - half, e - half, width), e))
        wave_cost = distance(x, start - half, end - half, width)
        total = wave_cost + 0.5 * env_cost
        if best is None or (total, start, end) < best[:3]:
            best = (total, start, end, wave_cost, env_cost)
    _, start, end, wave_cost, env_cost = best
    measured, clarity = measure_period(x, start, nominal)
    cents = round(1200 * math.log2(nominal / measured))
    if abs(cents) > 50:
        raise Fatal(f"{name}: measured pitch is {cents} cents off its name", 3)
    return {
        "key": key,
        "onset": onset,
        "start": start,
        "end": end,
        "cents": cents,
        "wave_cost": wave_cost,
        "env_cost": env_cost,
        "clarity": clarity,
    }


def render_sample(x, info):
    """The trimmed, faded and loop-crossfaded 16-bit sample data plus the
    loop points relative to its first sample."""
    onset, start, end = info["onset"], info["start"], info["end"]
    y = list(x[onset:end])
    s = start - onset
    e = end - onset
    for i in range(min(FADE_IN, len(y))):
        y[i] = y[i] * (i + 1) // (FADE_IN + 1)
    fade = min(CROSSFADE, (e - s) // 4, s)
    for i in range(fade):
        # Blend the loop's tail into the samples leading up to its start, so
        # the wrap from end-1 to start continues the original waveform.
        j = e - fade + i
        t = (i + 1) / (fade + 1)
        v = y[j] * (1.0 - t) + y[s - fade + i] * t
        y[j] = max(-32768, min(32767, int(math.floor(v + 0.5))))
    y.extend(y[s:s + GUARD])
    return y, s, e


# ----------------------------------------------------------------------------
# SoundFont writer
# ----------------------------------------------------------------------------

def chunk(cid, payload):
    if len(payload) % 2:
        payload += b"\0"
    return cid + struct.pack("<I", len(payload)) + payload


def zstr(text, limit):
    raw = text.encode("ascii")
    if len(raw) >= limit:
        raise Fatal(f"INFO text too long: {text[:40]}...", 2)
    raw += b"\0"
    if len(raw) % 2:
        raw += b"\0"
    return raw


def name20(text):
    raw = text.encode("ascii")[:20]
    return raw + b"\0" * (20 - len(raw))


def gen(oper, amount):
    return struct.pack("<Hh", oper, amount)


def gen_range(oper, lo, hi):
    return struct.pack("<HBB", oper, lo, hi)


def build_sf2(notes, comment):
    """notes: list of (key, name, data, loop_start, loop_end, cents),
    sorted by key."""
    smpl = bytearray()
    shdr = bytearray()
    offset = 0
    for key, label, data, ls, le, cents in notes:
        smpl += struct.pack(f"<{len(data)}h", *data)
        smpl += b"\0\0" * PAD
        shdr += name20(label) + struct.pack(
            "<IIIIIBbHH", offset, offset + len(data), offset + ls, offset + le,
            RATE, key, -cents, 0, 1)
        offset += len(data) + PAD
    shdr += name20("EOS") + struct.pack("<IIIIIBbHH", 0, 0, 0, 0, 0, 0, 0, 0, 0)

    # Instrument zones: split the key gaps between recorded notes.
    keys = [n[0] for n in notes]
    igen = bytearray()
    ibag = bytearray()
    for i, key in enumerate(keys):
        lo = 0 if i == 0 else keys[i - 1] + (key - keys[i - 1]) // 2 + 1
        hi = 127 if i == len(keys) - 1 else key + (keys[i + 1] - key) // 2
        ibag += struct.pack("<HH", len(igen) // 4, 0)
        igen += gen_range(43, lo, hi)          # keyRange
        igen += gen(34, ATTACK_TIMECENTS)       # attackVolEnv
        igen += gen(38, RELEASE_TIMECENTS)      # releaseVolEnv
        igen += gen(54, 1)                      # sampleModes: loop
        igen += gen(58, key)                    # overridingRootKey
        igen += gen(53, i)                      # sampleID (last)
    ibag += struct.pack("<HH", len(igen) // 4, 0)
    igen += struct.pack("<HH", 0, 0)
    inst = name20("Harmonium") + struct.pack("<H", 0)
    inst += name20("EOI") + struct.pack("<H", len(ibag) // 4 - 1)
    imod = b"\0" * 10

    pgen = gen(41, 0) + struct.pack("<HH", 0, 0)   # instrument 0
    pbag = struct.pack("<HH", 0, 0) + struct.pack("<HH", 1, 0)
    pmod = b"\0" * 10
    phdr = name20("Harmonium") + struct.pack("<HHHIII", GM_PROGRAM, 0, 0, 0, 0, 0)
    phdr += name20("EOP") + struct.pack("<HHHIII", 0, 0, 1, 0, 0, 0)

    info = (
        chunk(b"ifil", struct.pack("<HH", 2, 1))
        + chunk(b"isng", zstr("EMU8000", 256))
        + chunk(b"INAM", zstr("Cadence Harmonium", 256))
        + chunk(b"IENG", zstr("Donya Quick (recording); Nicholaus P. Brosowsky (edits); "
                              "Cadence (SoundFont)", 256))
        + chunk(b"ICOP", zstr("Samples CC BY 3.0 (tonejs-instruments, N. P. Brosowsky) "
                              "from a CC0 recording by Donya Quick", 256))
        + chunk(b"ICMT", zstr(comment, 65536))
        + chunk(b"ISFT", zstr("build-harmonium-sf2.py", 256))
    )
    pdta = b"".join([
        chunk(b"phdr", phdr), chunk(b"pbag", pbag), chunk(b"pmod", pmod),
        chunk(b"pgen", pgen), chunk(b"inst", inst), chunk(b"ibag", bytes(ibag)),
        chunk(b"imod", imod), chunk(b"igen", bytes(igen)), chunk(b"shdr", bytes(shdr)),
    ])
    body = (
        b"sfbk"
        + chunk(b"LIST", b"INFO" + info)
        + chunk(b"LIST", b"sdta" + chunk(b"smpl", bytes(smpl)))
        + chunk(b"LIST", b"pdta" + pdta)
    )
    return chunk(b"RIFF", body)


COMMENT = (
    "Harmonium: 38 real harmonium notes (C2-D5) recorded by Donya Quick at the "
    "Euterpea Studio, Yale University (\"Harmonium Samples - All Keys and Drones\", "
    "https://freesound.org/people/donyaquick/sounds/330410/, CC0 1.0). Edited "
    "samples by Nicholaus P. Brosowsky, tonejs-instruments "
    "(https://github.com/nbrosowsky/tonejs-instruments), licensed CC BY 3.0 "
    "(https://creativecommons.org/licenses/by/3.0/). Cadence trimmed the samples, "
    "tuned them to A440, added crossfaded sustain loops and packed them into "
    "this SoundFont with scripts/build-harmonium-sf2.py."
)


def build(sources, samples, quiet, verbose):
    notes = []
    for name, _, _ in samples:
        x = read_wav(name, sources[name])
        info = analyze(name, x)
        data, ls, le = render_sample(x, info)
        stem = name.rsplit(".", 1)[0].replace("s", "#")
        notes.append((info["key"], f"Harmonium {stem}", data, ls, le, info["cents"]))
        if verbose:
            log(f"  {name:8} key {info['key']:3} keep {len(data) / RATE:4.2f} s  "
                f"loop {ls / RATE:4.2f}-{le / RATE:4.2f} s ({(le - ls) / RATE:4.2f} s)  "
                f"tune {-info['cents']:+3d} c  wave {info['wave_cost']:.4f}  "
                f"env {info['env_cost']:.4f}  clarity {info['clarity']:.3f}", quiet)
    notes.sort(key=lambda n: n[0])
    keys = [n[0] for n in notes]
    if len(set(keys)) != len(keys):
        raise Fatal("two samples share a root key", 2)
    return build_sf2(notes, COMMENT)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--out", help="output .sf2 path")
    parser.add_argument("--src-cache", help="folder for the downloaded WAV files")
    parser.add_argument("--check", action="store_true",
                        help="rebuild in memory and compare with --out; write nothing")
    parser.add_argument("--force", action="store_true",
                        help="rebuild even when the output already has the pinned hash")
    parser.add_argument("--unpinned", action="store_true",
                        help="write the font even if it doesn't match the pinned hash "
                             "(for working on this script; update the pin afterwards)")
    parser.add_argument("--quiet", action="store_true")
    parser.add_argument("--verbose", action="store_true", help="print per-note loop details")
    try:
        args = parser.parse_args(argv)
    except SystemExit as exit_:
        return 2 if exit_.code else 0
    try:
        values, samples = read_pin()
        cache_root = os.environ.get("CADENCE_SOUNDFONT_CACHE") or str(ROOT / ".cache" / "soundfonts")
        out = Path(args.out) if args.out else Path(cache_root) / values["HARMONIUM_FILE"]
        if not out.is_absolute():
            out = ROOT / out
        src = Path(args.src_cache or os.environ.get("CADENCE_HARMONIUM_SRC_CACHE")
                   or ROOT / ".cache" / "soundfont-src" / "harmonium")
        if not src.is_absolute():
            src = ROOT / src
        want = values["HARMONIUM_SHA256"].lower()
        if not args.check and not args.force and out.is_file() \
                and sha256_bytes(out.read_bytes()) == want:
            log(f"Harmonium SoundFont cached: {out}", args.quiet)
            return 0
        sources = fetch_sources(values, samples, src, args.quiet)
        began = time.time()
        sf2 = build(sources, samples, args.quiet, args.verbose)
        got = sha256_bytes(sf2)
        log(f"Built {values['HARMONIUM_FILE']}: {len(sf2)} bytes, sha256 {got} "
            f"({time.time() - began:.1f} s)", args.quiet)
        if args.check:
            if not out.is_file():
                print(f"error: {out} does not exist", file=sys.stderr)
                return 1
            if out.read_bytes() != sf2:
                print(f"error: {out} differs from a fresh build", file=sys.stderr)
                return 1
            if got != want:
                print(f"error: build sha256 {got} != pinned {want}", file=sys.stderr)
                return 3
            log(f"OK: {out} matches a fresh build and the pin", args.quiet)
            return 0
        if got != want and not args.unpinned:
            raise Fatal(
                "the built SoundFont does not match HARMONIUM_SHA256 in "
                f"{PIN.relative_to(ROOT)}\n  expected {want}\n  got      {got}\n"
                "(if you changed this script on purpose, update the pin)", 3)
        out.parent.mkdir(parents=True, exist_ok=True)
        tmp = out.with_name(f".{out.name}.{os.getpid()}.tmp")
        tmp.write_bytes(sf2)
        os.replace(tmp, out)
        log(f"Harmonium SoundFont written: {out}", args.quiet)
        return 0
    except Fatal as err:
        print(f"error: {err}", file=sys.stderr)
        return err.code


if __name__ == "__main__":
    sys.exit(main())
