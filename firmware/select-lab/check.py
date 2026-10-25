"""Check `tuning-studio watch` output from select-lab.

Reads the output of a non-terminal watch on stdin, such as

    tuning-studio watch --interval 0 --count 2000 \
        "select_lab::waves::POOL[task 0].future" waves.sine waves.cosine | python3 check.py

Each sample is a count and a value. A sample is consistent when its value is the
sine or cosine of its count's phase, as the firmware computes it, and in order
when its count did not go backwards since the previous read of the same sample.
"""

import math
import re
import sys

STEPS = 1024
# libm's sinf and Python's sin differ by far less than this.
TOLERANCE = 1e-5

FUNCTIONS = {"sine": math.sin, "cosine": math.cos}


def expected(name, count):
    return FUNCTIONS[name]((count % STEPS) * (2 * math.pi / STEPS))


def samples(block):
    """(source, name, count or None, value or None) for each sample in one read."""
    out = []
    # The task's future: `a` is sine and `b` is cosine, in `select(sine(), cosine())`.
    for which, name in (("a", "sine"), ("b", "cosine")):
        head = re.search(rf"^    {which}: \S+ = (\S+)$", block, re.M)
        if not head:
            continue
        rest = block[head.end():]
        count = re.search(r"^        count: u32 = (\d+)$", rest, re.M)
        value = re.search(r"^        value: f32 = (\S+)$", rest, re.M)
        if head.group(1) != "Suspend0" or not count or not value:
            out.append(("task", name, None, head.group(1)))
        else:
            out.append(("task", name, int(count.group(1)), float(value.group(1))))
    for name in FUNCTIONS:
        head = re.search(rf"^waves\.{name}: Sample( = <(.*)>)?$", block, re.M)
        if not head:
            continue
        if head.group(1):
            out.append(("watch", name, None, head.group(2)))
            continue
        rest = block[head.end():]
        count = re.search(r"^  count: u32 = (\d+)$", rest, re.M)
        value = re.search(r"^  value: f32 = (\S+)$", rest, re.M)
        out.append(("watch", name, int(count.group(1)), float(value.group(1))))
    return out


def main():
    text = sys.stdin.read()
    parts = re.split(r"^--- ([0-9.]+) s$", text, flags=re.M)
    times = [float(t) for t in parts[1::2]]
    blocks = parts[2::2]
    stats = {}
    last = {}
    examples = []
    for block in blocks:
        for source, name, count, value in samples(block):
            s = stats.setdefault(source, {"reads": 0, "ok": 0, "wrong": 0, "back": 0, "unread": 0})
            s["reads"] += 1
            if count is None:
                s["unread"] += 1
                if len(examples) < 5:
                    examples.append(f"{source} {name}: {value}")
                continue
            if abs(value - expected(name, count)) > TOLERANCE:
                s["wrong"] += 1
                if len(examples) < 5:
                    examples.append(
                        f"{source} {name}: count {count}, value {value}, "
                        f"expected {expected(name, count):.7f}"
                    )
            elif last.get((source, name), -1) > count:
                s["back"] += 1
            else:
                s["ok"] += 1
            last[(source, name)] = count
    if not times:
        sys.exit("no reads in the input")
    elapsed = times[-1] - times[0]
    rate = (len(times) - 1) / elapsed if elapsed > 0 else float("nan")
    print(f"{len(times)} reads in {elapsed:.2f} s, {rate:.0f} reads/s of everything watched")
    for source, s in stats.items():
        print(
            f"{source}: {s['reads']} samples, {s['ok']} consistent, {s['wrong']} wrong, "
            f"{s['back']} went backwards, {s['unread']} unreadable"
        )
    for example in examples:
        print(f"  {example}")


main()
