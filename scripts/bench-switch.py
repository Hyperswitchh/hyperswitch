#!/usr/bin/env python3
# Sends N `next` requests to hyperswitchd over its socket and prints per-step statistics.
# Usage: scripts/bench-switch.py <socket> <count> <label> <output.json>
# Start the daemon first with the hook setup you want to measure. The daemon reports the time
# of each step itself, so these numbers exclude the client; "client rtt" adds the socket round trip.
import json, socket, sys, time, statistics as st
sock_path, n, label = sys.argv[1], int(sys.argv[2]), sys.argv[3]
def call(cmd):
    s = socket.socket(socket.AF_UNIX); s.connect(sock_path)
    t = time.perf_counter(); s.sendall((json.dumps(cmd) + "\n").encode())
    buf = b""
    while not buf.endswith(b"\n"): buf += s.recv(65536)
    rt = (time.perf_counter() - t) * 1000; s.close()
    return json.loads(buf), rt
rows = []
for i in range(n):
    r, rt = call({"cmd": "next"})
    if not r["ok"]: print("ERROR", r); sys.exit(1)
    d = r["data"]["switched"]; d["rt_ms"] = rt; rows.append(d)
def q(vals, p):
    v = sorted(vals); return v[min(len(v) - 1, int(len(v) * p))]
def line(name, vals, unit):
    print(f"  {name:<12} median {st.median(vals):9.2f}  p95 {q(vals,0.95):9.2f}  max {max(vals):9.2f} {unit}")
print(f"== {label}  (n={len(rows)}, over_budget={sum(1 for r in rows if r['over_budget'])})")
line("total",  [r["elapsed_us"]/1000 for r in rows], "ms")
for k in ("state_us","input_us","blur_us","focus_us"):
    line(k[:-3], [r["steps"][k]/1000 for r in rows], "ms")
line("client rtt", [r["rt_ms"] for r in rows], "ms")
json.dump(rows, open(sys.argv[4], "w"))
