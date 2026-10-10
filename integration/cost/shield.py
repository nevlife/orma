#!/usr/bin/env python3
"""shield.py save|restore FILE CPUS, as root.

save moves every task off CPUS and writes each moved task's affinity to FILE before moving it.
restore puts back what is still alive, leaves a task that set its own affinity in the meantime,
widens tasks born during the shield (their affinity is exactly the remaining CPUs) to all CPUs,
and removes FILE. Per-cpu kernel threads refuse and are left alone.

run.py --shield calls both and restores on every exit it can see (failure, SIGINT, SIGTERM,
SIGHUP). After a SIGKILL of run.py, restore by hand with the FILE it printed.
"""

import os
import sys


def cpus(spec):
    out = set()
    for part in spec.split(","):
        a, _, b = part.partition("-")
        out |= set(range(int(a), int(b or a) + 1))
    return out


def tids():
    for p in os.listdir("/proc"):
        if p.isdigit():
            try:
                yield from (int(t) for t in os.listdir(f"/proc/{p}/task"))
            except OSError:
                continue


def save(path, bench, house):
    moved = refused = 0
    with open(path, "w") as out:
        for tid in tids():
            try:
                cur = os.sched_getaffinity(tid)
                if not cur & bench:
                    out.write(f"seen {tid}\n")
                    continue
                out.write(f"{tid} {','.join(map(str, sorted(cur)))}\n")
                out.flush()
                os.sched_setaffinity(tid, (cur - bench) or house)
                moved += 1
            except OSError:
                refused += 1
    print(f"shield: moved {moved} tasks off {sorted(bench)}, {refused} refused or gone")


def restore(path, bench, house):
    allc = bench | house
    saved, seen = {}, set()
    for line in open(path):
        a, b = line.split()
        if a == "seen":
            seen.add(int(b))
        else:
            saved[int(a)] = {int(x) for x in b.split(",")}
    ok = gone = kept = widened = 0
    for tid, c in saved.items():
        try:
            if os.sched_getaffinity(tid) != ((c - bench) or house):
                kept += 1
                continue
            os.sched_setaffinity(tid, c)
            ok += 1
        except OSError:
            gone += 1
    for tid in tids():
        if tid in saved or tid in seen:
            continue
        try:
            if os.sched_getaffinity(tid) == house:
                os.sched_setaffinity(tid, allc)
                widened += 1
        except OSError:
            pass
    os.remove(path)
    print(f"shield: restored {ok}, gone {gone}, kept {kept}, widened {widened} born during it")


def main():
    op, path, spec = sys.argv[1:4]
    bench = cpus(spec)
    house = set(range(os.cpu_count())) - bench
    {"save": save, "restore": restore}[op](path, bench, house)


if __name__ == "__main__":
    main()
