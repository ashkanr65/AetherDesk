"""Check Linux compiler registrations; a registry alone is never a test result."""
import argparse
import hashlib
import json
from pathlib import Path
import re


def verify(catalog, phase, text):
    registered = set(re.findall(r"^([A-Za-z0-9_:]+): test\s*$", text, re.MULTILINE))
    if not registered:
        raise ValueError("Compiler test registry is empty")
    required = {check["test"] for feature in catalog["features"]
                for check in feature["checks"] if check["phase"] == phase}
    if not required:
        raise ValueError("Regression phase has no required tests")
    missing = [name for name in required if not any(
        test == name or test.endswith("::" + name) for test in registered)]
    if missing:
        raise ValueError("Missing compiler registrations: " + ", ".join(sorted(missing)))
    return {"schemaVersion": 1, "phase": phase, "registered": len(registered), "required": len(required),
            "compilerTestsPassed": False, "liveAcceptancePassed": False,
            "replacementReady": False}


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--catalog", required=True)
    parser.add_argument("--phase", required=True)
    parser.add_argument("--list", required=True)
    parser.add_argument("--output")
    parser.add_argument("--passed", action="store_true",
                        help="Use only after all cargo/Python checks finish successfully")
    args = parser.parse_args()
    result = verify(json.loads(Path(args.catalog).read_text(encoding="utf-8-sig")),
                    args.phase, Path(args.list).read_text(encoding="utf-8"))
    result["compilerTestsPassed"] = args.passed
    result["catalogSha256"] = hashlib.sha256(Path(args.catalog).read_bytes()).hexdigest()
    if args.output:
        Path(args.output).write_text(json.dumps(result, indent=2), encoding="utf-8")
    print(json.dumps(result))
