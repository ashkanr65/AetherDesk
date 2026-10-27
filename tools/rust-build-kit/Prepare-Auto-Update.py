"""Run after a completed build. Default: publish and verify the normal update.

--check: local verification only. --draft: upload without publishing.
This launcher also works from the sibling beginner build-kit folder.
"""
from pathlib import Path
import os
import runpy
import sys


def main():
    here = Path(__file__).resolve()
    configured = os.environ.get("AETHERDESK_RUST_SOURCE") or os.environ.get("AETHERDESK_SOURCE_ROOT")
    candidates = [Path(configured)] if configured else [here.parents[2], here.parent.parent / "AetherDesk-Rust"]
    for root in candidates:
        module = root / "tools/rust-build-kit/auto_update.py"
        if module.is_file():
            sys.path.insert(0, str(module.parent))
            try:
                runpy.run_path(str(module), run_name="__main__")
            except ModuleNotFoundError as error:
                if error.name and error.name.startswith("cryptography"):
                    print("Publisher dependency missing. Install it once with:", flush=True)
                    print('"' + sys.executable + '" -m pip install cryptography', flush=True)
                    return 1
                raise
            return 0
    print("Cannot find the AetherDesk source folder beside this build kit. Set AETHERDESK_RUST_SOURCE if you moved it.", flush=True)
    return 1


if __name__ == "__main__":
    sys.exit(main())
