import importlib.util
import pathlib

ROOT = pathlib.Path(__file__).resolve().parent.parent
SAMPLES = {
    "a.png": b"\x89PNG\r\n\x1a\n" + b"\0" * 32,
    "b.jpg": b"\xff\xd8\xff\xe0\x00\x10JFIF\x00" + b"\0" * 32,
    "c.gif": b"GIF89a" + b"\0" * 32,
    "d.txt": b"plain text, not an image",
}


def load():
    spec = importlib.util.spec_from_file_location("thumbs", ROOT / "tools" / "thumbs.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_imports_on_this_python():
    assert "imghdr" not in (ROOT / "tools" / "thumbs.py").read_text()
    load()


def test_kinds(tmp_path):
    thumbs = load()
    found = {}
    for name, data in SAMPLES.items():
        path = tmp_path / name
        path.write_bytes(data)
        found[name] = thumbs.kind(path)
    assert found == {"a.png": "png", "b.jpg": "jpeg", "c.gif": "gif", "d.txt": None}
