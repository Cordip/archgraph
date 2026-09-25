#!/usr/bin/env python3
"""Subprocess fixture only; never used as a production provider fallback."""
import json
import os
import re
import sys

args = sys.argv[1:]
mode = os.environ.get("ARCHGRAPH_FAKE_MODE", "clean")
log = os.environ.get("ARCHGRAPH_FAKE_LOG")
if log:
    with open(log, "a", encoding="utf-8") as stream:
        stream.write(json.dumps({"args": args, "cwd": os.getcwd()}) + "\n")
if args == ["--version"]:
    print("fixture-1.0")
    sys.exit(0)
if args and args[0] == "analyze":
    if len(args) != 3 or args[2] != "--index-only" or args[1] != os.getcwd():
        sys.exit("unexpected analyze argv")
    print("fixture-index-progress")
    sys.exit(0)
if not args or args[0] != "cypher" or len(args) not in (2, 4):
    sys.exit("unexpected command argv")
if len(args) == 4 and args[2] != "--repo":
    sys.exit("expected --repo")
if mode == "unindexed":
    sys.exit("repository is not indexed")
if mode == "malformed":
    print("{}")
    sys.exit(0)
query = args[1]
files_match = re.search(r"MATCH \(f:File\) RETURN f.filePath AS path ORDER BY path SKIP (\d+) LIMIT (\d+)$", query)
if files_match:
    # The indexed-file listing. src/a2.rs is deliberately not indexed.
    offset, size = map(int, files_match.groups())
    if mode == "ignores_skip":
        offset = 0
    page = ["src/a.rs", "src/b.rs"][offset:offset + size]
    table = "| path |\n| --- |" + "".join(f"\n| {path} |" for path in page)
    print(json.dumps({"markdown": table, "row_count": len(page)}))
    sys.exit(0)
if "RETURN f.filePath AS path" in query:
    print(json.dumps({"markdown": "| path |\n| --- |\n| src/a.rs |", "row_count": 1}))
    sys.exit(0)
if mode == "query_failure":
    sys.exit("import query failed")
match = re.search(r"SKIP (\d+) LIMIT (\d+)$", query)
if match is None:
    sys.exit("missing pagination")
if "r.type IN ['IMPORTS']" not in query or "ORDER BY source, target, kind, reason" not in query:
    sys.exit("unexpected dependency query")
offset, size = map(int, match.groups())
if mode == "index_rewrite":
    # Another `gitnexus analyze` finishing while ArchGraph pages through results.
    with open(os.path.join(".gitnexus", "meta.json"), "a", encoding="utf-8") as meta:
        meta.write(" ")
rows = []
if mode in ("violation", "cycle", "bad_count", "bad_confidence", "node_like_large"):
    rows.append(("src/a.rs", "src/b.rs"))
if mode == "cycle":
    rows.append(("src/b.rs", "src/a.rs"))
if mode == "out_of_scope":
    rows.append(("src/a.rs", "vendor/lib.rs"))
if mode == "violation_grown":
    rows.extend([("src/a.rs", "src/b.rs"), ("src/a2.rs", "src/b.rs")])
page = rows[offset:offset + size]
table = "| source | target | kind | confidence | reason |\n| --- | --- | --- | --- | --- |"
for source, target in page:
    confidence = "NaN" if mode == "bad_confidence" else "1.0"
    reason = "x" * 200_000 if mode == "node_like_large" else "static\\|import"
    table += f"\n| {source} | {target} | IMPORTS | {confidence} | {reason} |"
count = len(page) + (1 if mode == "bad_count" else 0)
payload = json.dumps({"markdown": table, "row_count": count}) + "\n"
if mode == "node_like_large":
    # Like Node: non-blocking stdout, then exit without draining. A pipe keeps
    # only what fits in its buffer; a regular file receives the whole payload.
    os.set_blocking(1, False)
    try:
        os.write(1, payload.encode())
    except BlockingIOError:
        pass
    os._exit(0)
sys.stdout.write(payload)
