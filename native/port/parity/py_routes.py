"""Print the Python API app's route table as JSON for coverage_tests (run with the trove-prod venv from the repo root)."""
import json, os, sys
sys.path.insert(0, os.getcwd())
os.environ.setdefault("SECRET_KEY", "x")
from starlette.routing import Mount
import app.main as m
out = []
def walk(routes, prefix=""):
    for r in routes:
        if isinstance(r, Mount):
            walk(r.routes, prefix + r.path); continue
        methods = sorted(getattr(r, "methods", None) or [])
        out.append({"path": prefix + r.path, "methods": [x for x in methods if x != "HEAD"]})
walk(m.app.routes)
json.dump(out, sys.stdout)
