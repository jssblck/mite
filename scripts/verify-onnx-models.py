from __future__ import annotations

import json
from pathlib import Path

import onnxruntime as ort


ROOT = Path(__file__).resolve().parents[1]
MANIFEST = json.loads((ROOT / "model-manifest.json").read_text(encoding="utf-8"))


def describe_model(path: Path) -> dict[str, object]:
    session = ort.InferenceSession(str(path), providers=["CPUExecutionProvider"])
    return {
        "path": str(path.relative_to(ROOT)),
        "inputs": [
            {"name": value.name, "shape": value.shape, "type": value.type}
            for value in session.get_inputs()
        ],
        "outputs": [
            {"name": value.name, "shape": value.shape, "type": value.type}
            for value in session.get_outputs()
        ],
    }


def main() -> None:
    reports = []
    for model in MANIFEST["models"]:
        if model["kind"] == "dictionary":
            continue
        path = ROOT / model["local_path"]
        if not path.exists():
            raise SystemExit(f"Missing model: {path}")
        reports.append(describe_model(path))

    print(json.dumps(reports, indent=2))


if __name__ == "__main__":
    main()
