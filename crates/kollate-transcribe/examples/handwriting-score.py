#!/usr/bin/env python3
"""Scores handwriting-eval results against the writers' answers.

    python3 handwriting-score.py <set dir> [--results <dir in it>] [--json]

For each run in <set dir>/results:
- Handwriting: character accuracy (1 - edit distance / answer length, over
  all characters) and the share of pages read exactly, after joining lines
  (a note's line breaks are where the margin ran out) and straightening
  quotes. The model's notes may come in any order; the best order is used.
- Pen marks: stars (* or NB), lone question marks and #tags found and missed.
- Marked passages: underlined and circled passages found, by count.
- Circled words: single circled words sent to Vocabulary.
- Speed: median milliseconds per model call and per page.
Items marked "Leave out" or "I can't read it either" are not scored.
"""

import itertools
import json
import re
import statistics
import sys
from difflib import SequenceMatcher
from pathlib import Path


def edits(a, b):
    """Levenshtein distance."""
    prev = list(range(len(b) + 1))
    for i, ca in enumerate(a, 1):
        cur = [i]
        for j, cb in enumerate(b, 1):
            cur.append(min(prev[j] + 1, cur[j - 1] + 1, prev[j - 1] + (ca != cb)))
        prev = cur
    return prev[-1]


def tidy(text):
    text = (text or "").translate(str.maketrans("‘’“”–—", "''\"\"--"))
    return " ".join(text.split())


def lines(text):
    return [l.strip() for l in (text or "").splitlines() if l.strip()]


def best_order(truth, predicted):
    """The model's notes joined in the order closest to the answer."""
    notes = lines(predicted)
    if len(notes) > 7:
        return tidy(predicted)
    best = None
    for order in itertools.permutations(notes):
        joined = tidy(" ".join(order))
        d = edits(truth, joined)
        if best is None or d < best[0]:
            best = (d, joined)
    return best[1] if best else ""


def pen_marks(text):
    words = tidy(text).split()
    return {
        "star": sum(w in ("*", "NB") for w in words),
        "question": sum(l == "?" for l in lines(text)),
        "tags": sorted(w.lower().rstrip(".,!?") for w in words if re.match(r"#\w", w)),
    }


def same_word(a, b):
    a, b = a.lower(), b.lower()
    return a == b or SequenceMatcher(None, a, b).ratio() >= 0.8


def score(run, answers, manifest, writer=None):
    items = {i["key"]: i for i in manifest["items"]}
    s = dict(chars=0, errors=0, notes=0, exact=0, phantom=0, empty=0,
             star=[0, 0, 0], question=[0, 0, 0], tags=[0, 0, 0],
             passages=[0, 0, 0], circled=[0, 0, 0], ms_item=[], ms_call=[], items=0)
    for key, result in run["items"].items():
        a = answers.get(key, {})
        if a.get("skip") or a.get("unreadable") or "error" in result:
            continue
        if writer and (a.get("writer") or "unassigned") != writer:
            continue
        s["items"] += 1
        s["ms_item"].append(result["ms"])
        if result["calls"]:
            s["ms_call"].append(result["ms"] / result["calls"])
        truth = tidy(a.get("text"))
        if truth:
            read = best_order(truth, result.get("note"))
            s["chars"] += len(truth)
            s["errors"] += min(edits(truth, read), len(truth))
            s["notes"] += 1
            s["exact"] += truth == read
            if not read:
                s["empty"] += 1
        elif tidy(result.get("note")):
            s["phantom"] += 1
        # Pen marks: [found, in the answers, read where there were none]
        want, got = pen_marks(a.get("text")), pen_marks(result.get("note"))
        for k in ("star", "question"):
            s[k][0] += min(want[k], got[k])
            s[k][1] += want[k]
            s[k][2] += max(got[k] - want[k], 0)
        tags = list(got["tags"])
        for t in want["tags"]:
            s["tags"][1] += 1
            match = next((g for g in tags if same_word(t, g)), None)
            if match:
                s["tags"][0] += 1
                tags.remove(match)
        s["tags"][2] += len(tags)
        if run["setup"] == "whole-page" or items[key]["kind"] == "notebook":
            continue
        want_p = (a.get("underlines") or 0) + (a.get("circles") or 0)
        got_p = len(lines(result.get("text")))
        s["passages"][0] += min(want_p, got_p)
        s["passages"][1] += want_p
        s["passages"][2] += max(got_p - want_p, 0)
        want_c = [w.strip() for w in (a.get("circled") or "").split(",") if w.strip()]
        got_c = list(result.get("circled") or [])
        for w in want_c:
            s["circled"][1] += 1
            match = next((g for g in got_c if same_word(w, g)), None)
            if match:
                s["circled"][0] += 1
                got_c.remove(match)
        s["circled"][2] += len(got_c)
    return s


def summary(s):
    pct = lambda n, d: f"{100 * n / d:.0f}%" if d else "–"
    frac = lambda v: f"{v[0]}/{v[1]}" + (f" (+{v[2]} extra)" if v[2] else "")
    med = lambda v: f"{statistics.median(v):.0f}" if v else "–"
    return {
        "pages": s["items"],
        "pages with writing": s["notes"],
        "char accuracy": pct(s["chars"] - s["errors"], s["chars"]),
        "read exactly": f"{s['exact']}/{s['notes']} ({pct(s['exact'], s['notes'])})",
        "writing missed": s["empty"],
        "writing invented": s["phantom"],
        "stars": frac(s["star"]),
        "question marks": frac(s["question"]),
        "tags": frac(s["tags"]),
        "passages": frac(s["passages"]),
        "circled words": frac(s["circled"]),
        "ms per call": med(s["ms_call"]),
        "ms per page": med(s["ms_item"]),
    }


def main():
    set_dir = Path(sys.argv[1])
    manifest = json.loads((set_dir / "manifest.json").read_text())
    answers = json.loads((set_dir / "answers.json").read_text())["answers"]
    writers = sorted({a.get("writer") or "unassigned" for a in answers.values()
                      if not a.get("skip")})
    out = {}
    results = sys.argv[sys.argv.index("--results") + 1] if "--results" in sys.argv else "results"
    for path in sorted((set_dir / results).glob("*.json")):
        run = json.loads(path.read_text())
        name = f"{run['model']} · {run['setup']} · {run['device']}"
        out[name] = {"all": summary(score(run, answers, manifest))}
        for w in writers:
            out[name][w] = summary(score(run, answers, manifest, w))
    if "--json" in sys.argv:
        print(json.dumps(out, indent=2, ensure_ascii=False))
        return
    for name, by_writer in out.items():
        print(f"\n## {name}")
        keys = list(by_writer["all"])
        cols = list(by_writer)
        print("| | " + " | ".join(cols) + " |")
        print("|---" * (len(cols) + 1) + "|")
        for k in keys:
            print(f"| {k} | " + " | ".join(str(by_writer[c][k]) for c in cols) + " |")


if __name__ == "__main__":
    main()
