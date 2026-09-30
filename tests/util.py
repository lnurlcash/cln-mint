import json
import os
from pathlib import Path

import pytest
import requests

RUST_PROFILE = os.environ.get("RUST_PROFILE", "debug")
plugin_dir = Path(__file__).parent.parent.resolve()
COMPILED_PATH = plugin_dir / "target" / RUST_PROFILE / "cln-mint"
DOWNLOAD_PATH = plugin_dir / "tests" / "cln-mint"
VECTORS_PATH = Path(__file__).parent / "vectors"


@pytest.fixture
def get_plugin(directory):
    if COMPILED_PATH.is_file():
        return COMPILED_PATH
    elif DOWNLOAD_PATH.is_file():
        return DOWNLOAD_PATH
    else:
        raise ValueError("No files were found.")


def vectors(lud):
    with open(VECTORS_PATH / f"{lud}-vectors.json") as f:
        return json.load(f)


def get(url, params=None, method="GET"):
    """An LNURL request: always HTTP 200, success or {"status": "ERROR"}."""
    res = requests.request(method, url, params=params, timeout=30)
    assert res.status_code == 200, res.text
    return res.json()


def ok(body):
    assert body.get("status") != "ERROR", body
    return body


def error(body, reason=None):
    assert body.get("status") == "ERROR", body
    if reason is not None:
        assert body["reason"] == reason, body
    return body["reason"]
