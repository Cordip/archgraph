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
if "RETURN f.filePath AS path" in query:
    print(json.dumps({"markdown": "| path |\n| --- |\n| src/a.rs |", "row_count": 1}))
    sys.exit(0)
if mode == "query_failure":
    sys.exit("import query failed")
match = re.search(r"SKIP (\d+) LIMIT (\d+)$", query)
if match is None:
    sys.exit("missing pagination")
if "CodeRelation {type: 'IMPORTS'}" not in query or "ORDER BY source, target" not in query:
    sys.exit("unexpected import query")
offset, size = map(int, match.groups())
rows = []
if mode in ("violation", "cycle", "bad_count", "bad_confidence"):
    rows.append(("src/a.rs", "src/b.rs"))
if mode == "cycle":
    rows.append(("src/b.rs", "src/a.rs"))
page = rows[offset:offset + size]
table = "| source | target | confidence | reason |\n| --- | --- | --- | --- |"
for source, target in page:
    confidence = "NaN" if mode == "bad_confidence" else "1.0"
    table += f"\n| {source} | {target} | {confidence} | static\\|import |"
count = len(page) + (1 if mode == "bad_count" else 0)
print(json.dumps({"markdown": table, "row_count": count}))
