"""Long task: a three.js night city, graded by the grader's own page in headless Chrome."""

import html
import os
import re
import shutil
import tempfile

from .browser import dump_dom, serve

HERE = os.path.dirname(os.path.abspath(__file__))

PROMPT = """Build a night city scene with three.js in this project. three.js is already here at
vendor/three/three.module.min.js (import it as "three" through an import map; there is no network,
so use only files in this project).

1. `src/scene.js` exports `createScene(THREE)` returning `{ scene, camera, update }`:
   - a ground plane and at least 40 buildings of varied heights (at least 8 different heights)
     laid out on a street grid; give every building mesh `userData.kind = "building"`
     (an InstancedMesh with that tag counts each instance);
   - lit windows: building materials (or separate window meshes) with an emissive color or an
     emissive map, so some windows glow;
   - at least two lights, one of them a directional, point or spot light (the moon or street
     lights), and fog;
   - `update(seconds)` moves the camera slowly around the city (it must move between t=0 and t=5)
     and keeps it looking at the city.
   `createScene` must not create a renderer or touch the DOM: it only builds the scene.
2. `index.html` shows the scene full-window: a WebGLRenderer, resizing with the window, and an
   animation loop that calls `update`. It must load and run without errors in the browser.

The scene should read as night: mostly dark, with glowing windows. Check your work as far as you
can, then tell me what you built."""


def setup(project):
    shutil.copytree(os.path.join(HERE, "vendor"), os.path.join(project, "vendor"))


def measure(project):
    """The probe's measurements of the agent's scene and the console errors of its page."""
    import json
    root = tempfile.mkdtemp(prefix="hard-threejs-")
    try:
        for name in ("src", "vendor"):
            if os.path.isdir(os.path.join(project, name)):
                shutil.copytree(os.path.join(project, name), os.path.join(root, name))
        if os.path.exists(os.path.join(project, "index.html")):
            shutil.copy(os.path.join(project, "index.html"), os.path.join(root, "index.html"))
        # The grader's own copy of three.js and probe: the agent's files cannot change them.
        shutil.rmtree(os.path.join(root, "vendor"), ignore_errors=True)
        shutil.copytree(os.path.join(HERE, "vendor"), os.path.join(root, "vendor"))
        shutil.copy(os.path.join(HERE, "probe.html"), os.path.join(root, "__probe.html"))
        with serve(root) as base:
            dom, _ = dump_dom(base + "/__probe.html")
            page, page_errors = dump_dom(base + "/index.html") if os.path.exists(
                os.path.join(root, "index.html")) else ("", ["index.html missing"])
        found = re.search(r'<pre id="probe">(.*?)</pre>', dom, re.S)
        try:
            probe = json.loads(html.unescape(found.group(1))) if found else {"error": "probe did not run"}
        except ValueError:
            probe = {"error": "probe did not finish"}
        return {"probe": probe, "page_canvas": "<canvas" in page, "page_errors": page_errors[:5]}
    finally:
        shutil.rmtree(root, ignore_errors=True)


LIMITS = [("buildings", 40), ("heights", 8), ("emissive", 1), ("lights", 2), ("keyLights", 1)]


def problems(measured):
    """Every way the scene misses the spec (empty when it passes)."""
    probe = measured["probe"]
    if "error" in probe:
        return [f"scene module: {probe['error'][:200]}"]
    found = [f"{key} {probe.get(key)} < {least}" for key, least in LIMITS if (probe.get(key) or 0) < least]
    if not probe.get("fog"):
        found.append("no fog")
    if (probe.get("moved") or 0) < 0.5:
        found.append("camera does not move")
    if not 0.002 <= probe.get("bright", 0) <= 0.6:
        found.append(f"bright pixels {probe.get('bright')} (no glowing windows, or washed out)")
    if probe.get("dark", 0) < 0.3:
        found.append(f"dark pixels {probe.get('dark')} (does not read as night)")
    if probe.get("colors", 0) < 30:
        found.append("render is nearly flat")
    if not measured["page_canvas"]:
        found.append("index.html shows no canvas")
    if measured["page_errors"]:
        found.append(f"index.html errors: {measured['page_errors'][0][:160]}")
    return found


def check(project, report):
    measured = measure(project)
    found = problems(measured)
    return not found, "; ".join(found)
