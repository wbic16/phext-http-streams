"""Checks a Python phext implementation against core/test-vectors.

Usage: python check_python.py [path-to-libphext-py]   (default: ../../../libphext-py)
"""
import json
import pathlib
import sys

here = pathlib.Path(__file__).resolve().parent
lib = pathlib.Path(sys.argv[1]) if len(sys.argv) > 1 else here.parents[2] / "libphext-py"
sys.path.insert(0, str(lib))
from phext.phext import Phext  # noqa: E402

failures = 0
vectors = sorted(here.glob("*.phext"))
for path in vectors:
    text = path.read_bytes().decode("utf-8")
    want = json.loads(path.with_suffix(".expected.json").read_text(encoding="utf-8"))
    got = [{"coord": str(ps.coord), "text": ps.text} for ps in Phext().phokenize(text) if ps.text]
    if got != want:
        failures += 1
        first = next((i for i, (g, w) in enumerate(zip(got, want)) if g != w), min(len(got), len(want)))
        print(f"FAIL {path.stem}: {len(got)} scrolls vs {len(want)} expected; first difference at #{first}")
        print(f"  got:  {got[first] if first < len(got) else None}")
        print(f"  want: {want[first] if first < len(want) else None}")
print(f"python ({lib}): {len(vectors) - failures}/{len(vectors)} vectors match")
sys.exit(1 if failures else 0)
