#!/usr/bin/env python3
"""Record fixtures/<name>/replay.json: the envelope Kalam's own match loop writes for that
fixture's match, played offline through `orion-server dry-run` on a cartridge release.

    scripts/record-replay.py memflaky <cartridge dir> [<kalam checkout>]

The cartridge dir holds `plugin.toml`, `tb-ants.wasm` and `maps/` -- an unpacked ants release,
such as the one under the CLI's cache (`tinybrains games` prints where the registry resolved), or
an ants checkout's `dist/`. Its component's digest is the replay's `engine_digest`, so record on
the release the registry CI plays pins, or `conform` refuses the replay as another engine's. The
kalam checkout defaults to ../kalam; its `kalam-match-run.json` and `shared/` are what play.

The gate, the replay bucket and the token cache are stubbed as kalam's own offline cases stub them
(kalam/tests/make-cases.py); the engine and the two models run for real. The seats carry the
fixture files' real digests, so `conform` finds them in the store once `check` or a match has
read them. Needs orion-server on PATH, at the version kalam's shared/package.json requires.
"""
import hashlib, json, os, subprocess, sys, tempfile

if len(sys.argv) not in (3, 4):
    sys.exit(__doc__)
name, cartridge = sys.argv[1:3]
here = os.path.dirname(os.path.abspath(__file__))
root = os.path.dirname(here)
kalam = os.path.abspath(sys.argv[3] if len(sys.argv) == 4 else os.path.join(root, "..", "kalam"))
fixture = os.path.join(root, "fixtures", name)
match = json.load(open(os.path.join(fixture, "match.json")))
row_in = match["rows"][0]


def sha(path):
    return "sha256:" + hashlib.sha256(open(path, "rb").read()).hexdigest()


engine = sha(os.path.join(cartridge, "tb-ants.wasm"))
board = json.load(open(os.path.join(cartridge, "maps", row_in["map"] + ".json")))
orion = subprocess.run(["orion-server", "--version"], capture_output=True, text=True, check=True)
orion_version = orion.stdout.split()[1]

# The row as the gate would hand it over: the fixture's seats, under their real digests, naming
# the models by the ids the manifests declare (which is how --model-dir finds them).
seats = []
for s in row_in["seats"]:
    manifest = os.path.join(fixture, s["manifest"])
    seats.append({"seat": s["seat"], "model": json.load(open(manifest))["name"],
                  "weights_hash": sha(os.path.join(fixture, s["weights"])), "manifest_hash": sha(manifest),
                  "strike_ceiling": match["vars"].get("strike_ceiling", 5)})
row = {"id": row_in["id"], "seed": row_in["seed"], "map": board, "map_id": board["id"],
       "seat_count": len(seats), "strike_ceiling": match["vars"].get("strike_ceiling", 5), "seats": seats}
gate = {"token": "tok", "expires_in": 600, "match": row, "claim": {"token": "ct-1"},
        "contract": {"turn_ms": match["vars"].get("turn_ms", 1000), "max_turns": match["vars"]["max_turns"],
                     "renew_every_n_turns": 50, "lease_seconds": 300, "refusal_ceiling": 5},
        "started": True, "applied": True, "state": "finished", "mine": True,
        "url": "http://blobs:9000/tinybrains-replays/replays/m-1/ct-1.json",
        "endpoint": "http://blobs:9000", "key": "replays/m-1/ct-1.json"}
stubs = {"http_call": {"kalam-api": gate, "kalam-orion": {"data": {"status": "active"}}, "kalam-blobs-put": ""},
         "cache_read": {"kalam-cache": None}, "cache_write": {"kalam-cache": {}},
         "cache_delete": {"kalam-cache": {"deleted": 1}}}
meta = {"vars": {"engine_digest": engine, "orion_version": orion_version, "runner_key": "k",
                 "runner_label": "h", "arch": "arm64", "node_version": "dev", "match_slots": 2,
                 "ops_budget": match["vars"].get("budget_ops", 1000000), "match_timeout_ms": 2400000,
                 "seat_concurrency": 2, "models_bucket_connector": "kalam-models"},
        "trigger": {"scheduled_for": "2026-01-01T00:00:00Z", "attempt": 1,
                    "occurrence_id": "01a0db56-0000-7000-8000-000000000001", "singleton_slot": 0}}

with tempfile.TemporaryDirectory() as d:
    json.dump(stubs, open(f"{d}/stubs.json", "w"))
    json.dump(meta, open(f"{d}/meta.json", "w"))
    json.dump({}, open(f"{d}/in.json", "w"))
    out = subprocess.run(["orion-server", "dry-run", "--definitions", f"{kalam}/shared",
                          "-w", f"{kalam}/workflows/kalam-match-run.json", "-i", f"{d}/in.json",
                          "-m", f"{d}/meta.json", "--stubs", f"{d}/stubs.json",
                          "--plugin-dir", cartridge, "--model-dir", os.path.join(fixture, "models"),
                          "--trace", "none"], capture_output=True, text=True)
if out.returncode != 0:
    sys.exit(f"dry-run failed:\n{out.stderr[-2000:]}")
run = json.loads(out.stdout)
if run["data"].get("outcome") != "complete":
    sys.exit(f"the match did not complete: outcome {run['data'].get('outcome')!r}, errors {run.get('errors')}")
put = [c for c in run["calls"]["http_call"] if c["task_id"] == "put"]
if len(put) != 1:
    sys.exit(f"expected one replay PUT, saw {len(put)}")

path = os.path.join(fixture, "replay.json")
with open(path, "w") as f:
    json.dump(put[0]["input"]["body"], f, indent=1, sort_keys=True)
    f.write("\n")
refs = run["data"]["refs"]
print(f"{os.path.relpath(path, root)}: engine {engine[:19]}, orion {orion_version}, {run['data']['deltas'].__len__()} turns")
for r in refs:
    print(f"  seat {r['seat']} {r['model']}: {r['infer_turns']} calls, {r['strikes']} struck, forfeited {r['forfeited']}")
