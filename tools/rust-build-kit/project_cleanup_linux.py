"""Exact-owner cleanup in the existing Podman store; never prune or reset."""
import base64
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import uuid

PROJECTS = {"aetherdesk", "openeda"}
LABEL_PROJECT = "io.aetherdesk.project"
LABEL_TASK = "io.aetherdesk.task"
LABEL_ROLE = "io.aetherdesk.cleanup"

def short_task_directory(task):
    return Path("/tmp/ad") / hashlib.sha256(task.encode("ascii")).hexdigest()[:12]

def task_directory(project, task, requested=None):
    legacy = Path("/var/tmp/aether-project-tasks") / project / task
    path = Path(requested) if requested else legacy
    if path not in (legacy, short_task_directory(task)):
        raise ValueError("Unrecognized managed task directory")
    return path


def run(args):
    result = subprocess.run(args, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                            universal_newlines=True)
    if result.returncode:
        raise RuntimeError("{} failed ({}): {}".format(args[0], result.returncode,
                                                     result.stderr[-1000:]))
    return result.stdout


def validate_tree(path, root, for_deletion=False):
    path, root = Path(path), Path(root)
    if not path.is_absolute() or not root.is_absolute() or root == Path("/"):
        raise ValueError("Absolute managed paths required")
    if path != root and root not in path.parents:
        raise ValueError("Outside managed task directory")
    for part in (path,) + tuple(path.parents):
        if part.is_symlink():
            raise ValueError("Symlink ancestor refused: " + str(part))
    if path.resolve() != path or root.resolve() != root:
        raise ValueError("Non-canonical path refused")
    if not path.exists():
        return
    pending = [path]
    while pending:
        item = pending.pop()
        if for_deletion and item.name == ".git":
            raise ValueError("Git history/worktrees must be preserved: " + str(item))
        if item.is_symlink() or os.path.ismount(str(item)):
            raise ValueError("Link or mount refused: " + str(item))
        if item.is_dir():
            pending.extend(item.iterdir())


class Store:
    def __init__(self, config):
        if os.geteuid() != int(config["uid"]):
            raise ValueError("Wrong Podman user")
        if config["driver"] != "vfs" or not Path(config["graphRoot"]).is_dir():
            raise ValueError("Existing configured store is unavailable; no store will be created")
        self.prefix = ["podman", "--storage-driver=" + config["driver"]]
        info = json.loads(run(self.prefix + ["info", "--format=json"]))
        actual = info.get("store", {})
        if (actual.get("graphRoot") != config["graphRoot"] or
                actual.get("graphDriverName") != config["driver"]):
            raise ValueError("Podman store identity changed")

    def inventory(self):
        containers = json.loads(run(self.prefix + ["ps", "--all", "--external", "--no-trunc", "--format=json"]))
        images = json.loads(run(self.prefix + ["images", "--all", "--no-trunc", "--format=json"]))
        return containers, images

    def inspect(self, kind, resource):
        if not re.fullmatch(r"[a-f0-9]{64}", resource):
            raise ValueError("Full exact resource ID required")
        if kind == "container":
            try:
                return json.loads(run(self.prefix + ["container", "inspect", resource]))[0]
            except RuntimeError:
                value = json.loads(run(["buildah", "--storage-driver=vfs", "inspect", "--type", "container", resource]))
                labels = value.get("OCIv1", {}).get("config", {}).get("Labels", {}) or {}
                return {"Id": resource, "Config": {"Labels": labels},
                        "Image": value.get("FromImageID", ""), "external": True,
                        "State": {"Running": False}}
        return json.loads(run(self.prefix + ["image", "inspect", resource]))[0]

    def remove(self, kind, resource):
        if kind == "container":
            run(self.prefix + ["rm", "--force", "--ignore", resource])
        else:
            run(self.prefix + ["image", "rm", "--ignore", "--no-prune", resource])


def resource_id(row):
    return str(row.get("Id", row.get("ID", row.get("id", "")))).replace("sha256:", "", 1)


def labels(info):
    return info.get("Config", {}).get("Labels", {}) or info.get("Labels", {}) or {}


def owned(info, project, task):
    value = labels(info)
    return (value.get(LABEL_PROJECT) == project and value.get(LABEL_TASK) == task
            and value.get(LABEL_ROLE) == "disposable")


def active_builds():
    # Buildah storage containers have no Running flag. Never assume Storage
    # means inactive while ANY Podman/Buildah build is still in progress.
    for entry in Path("/proc").iterdir():
        if not entry.name.isdigit():
            continue
        try:
            args = (entry / "cmdline").read_bytes().split(b"\0")
            if args and Path(os.fsdecode(args[0])).name in ("podman", "buildah") and any(a in (b"build", b"bud") for a in args):
                return True
        except (OSError, ValueError):
            continue
    return False


def container_info(store, row):
    try:
        return store.inspect("container", resource_id(row))
    except Exception:
        # Podman does not inspect Buildah storage containers, but ps --external
        # supplies their immutable backing ImageID. Use that ONLY to preserve
        # references to an unlabelled, unowned container; never to delete it.
        image = str(row.get("ImageID", "")).replace("sha256:", "", 1)
        if row.get("State") == "storage" and not row.get("Labels") and re.fullmatch(r"[a-f0-9]{64}", image):
            return {"Config": {"Labels": {}}, "Image": image,
                    "external": True, "State": {"Running": False}}
        raise


def active_workspace_jobs(roots):
    # Read paths only; do not record command contents or environment secrets.
    for entry in Path("/proc").iterdir():
        if not entry.name.isdigit() or int(entry.name) == os.getpid():
            continue
        try:
            cwd = str((entry / "cwd").resolve())
            args = (entry / "cmdline").read_bytes().split(b"\0")
            paths = [cwd] + [os.fsdecode(a) for a in args if a.startswith(b"/")]
            if any(p == root or p.startswith(root + "/") for p in paths for root in roots):
                return True
        except (OSError, ValueError):
            continue
    return False


def clean_resources(store, project, task, protected, apply=False, build_busy=None):
    containers, images = store.inventory()
    output = {"resources": [], "removed": [], "deferred": [], "remainingImageCacheBytes": 0}
    used = set()
    for item in containers:
        identity = resource_id(item)
        try:
            info = container_info(store, item)
        except Exception as error:
            # Failure to inspect an unknown container also makes image-use
            # information incomplete. Refuse image deletion in that case.
            output["deferred"].append("container inspection unavailable: " + identity)
            used.add("unknown")
            continue
        used.add(str(info.get("Image", "")).replace("sha256:", "", 1))
        if not owned(info, project, task):
            continue
        output["resources"].append({"kind": "container", "id": identity})
        busy = info.get("State", {}).get("Running", False) or (info.get("external") and (active_builds() if build_busy is None else build_busy))
        if identity in protected or busy:
            output["deferred"].append("active/protected container: " + identity)
            continue
        # Reinspect immediately before an exact-ID mutation.
        fresh = store.inspect("container", identity)
        if not owned(fresh, project, task) or fresh.get("State", {}).get("Running", False):
            output["deferred"].append("container changed: " + identity)
            continue
        if apply:
            store.remove("container", identity)
        output["removed"].append({"kind": "container", "id": identity, "dryRun": not apply})
    # Refresh references after removing inactive task containers. Any container,
    # including another project's inactive/runtime container, protects its image.
    if apply:
        remaining, _ = store.inventory()
        used = set()
        for item in remaining:
            try:
                used.add(str(container_info(store, item).get("Image", "")).replace("sha256:", "", 1))
            except Exception:
                used.add("unknown")
    seen = set()
    for item in images:
        identity = resource_id(item)
        if identity in seen:
            continue
        seen.add(identity)
        try:
            info = store.inspect("image", identity)
        except Exception:
            output["deferred"].append("image inspection unavailable: " + identity)
            continue
        if not owned(info, project, task):
            continue
        output["resources"].append({"kind": "image", "id": identity})
        if identity in protected or identity in used or "unknown" in used:
            output["deferred"].append("referenced/protected image: " + identity)
            if identity not in protected:
                output["remainingImageCacheBytes"] += int(info.get("Size", 0))
            continue
        fresh = store.inspect("image", identity)
        if not owned(fresh, project, task):
            output["deferred"].append("image ownership changed: " + identity)
            continue
        if apply:
            store.remove("image", identity)
        output["removed"].append({"kind": "image", "id": identity, "dryRun": not apply})
    return output


def main(request):
    project, task = request["project"], request.get("task", "")
    if project not in PROJECTS or (task and not re.fullmatch(project + r"-[A-Za-z0-9-]{8,100}", task)):
        raise ValueError("Invalid project/task")
    store = Store(request["store"])
    if request["action"] == "inspect":
        containers, images = store.inventory()
        return {"df": run(["df", "-h", "/"]), "podmanDf": run(store.prefix + ["system", "df"]),
                "containers": containers, "images": images}
    root = task_directory(project, task, request.get("wslWork"))
    marker = root / ".project-owner.json"
    if request["action"] == "begin":
        validate_tree(root, root)
        root.mkdir(parents=True, exist_ok=False)
        marker.write_text(json.dumps({"project": project, "task": task}))
        os.chmod(str(root), 0o700)
        return {"work": str(root)}
    if request["action"] == "run":
        validate_tree(root, root)
        if json.loads(marker.read_text()) != {"project": project, "task": task}:
            raise ValueError("Test container task owner mismatch")
        image = request.get("image", "")
        if not image or image.startswith("-") or any(c.isspace() for c in image):
            raise ValueError("A single existing image name/ID is required")
        suffix = uuid.uuid4().hex
        cid = root / (suffix + ".cid")
        log = root / (suffix + ".log")
        command = store.prefix + ["run", "--rm", "--pull=never", "--network=none",
                  "--cidfile", str(cid), "--label", LABEL_PROJECT + "=" + project,
                  "--label", LABEL_TASK + "=" + task, "--label", LABEL_ROLE + "=disposable",
                  "-v", str(root) + ":/task", image] + request.get("command", [])
        with log.open("wb") as output:
            status = subprocess.call(command, stdout=output, stderr=subprocess.STDOUT)
        with log.open("rb") as output:
            output.seek(max(0, log.stat().st_size - 4096))
            tail = output.read().decode("utf-8", "replace")
        return {"exitCode": status, "containerId": cid.read_text().strip() if cid.exists() else "",
                "log": str(log), "tail": tail}
    if request["action"] != "clean":
        raise ValueError("Unknown action")
    validate_tree(root, root, for_deletion=True)
    roots = [str(root)]
    if request.get("windowsWork"):
        roots.append(run(["wslpath", "-u", request["windowsWork"]]).strip())
    if active_workspace_jobs(roots):
        return {"resources": [], "removed": [], "remainingImageCacheBytes": 0,
                "deferred": ["A live WSL job still references the task workspace"]}
    result = clean_resources(store, project, task, set(request.get("protected", [])), request.get("apply", False))
    if root.exists():
        validate_tree(root, root, for_deletion=True)
        if json.loads(marker.read_text()) != {"project": project, "task": task}:
            raise ValueError("Task directory owner mismatch")
        # A running task container might still be using this directory.
        if result["deferred"] or request.get("keepWork", False):
            result["deferred"].append("WSL task directory retained while resources are uncertain")
        else:
            if request.get("apply", False):
                shutil.rmtree(str(root))
            result["removed"].append({"kind": "wsl-directory", "path": str(root), "dryRun": not request.get("apply", False)})
    return result


if __name__ == "__main__":
    try:
        print(json.dumps(main(json.loads(base64.b64decode(sys.argv[1]).decode("utf-8")))))
    except Exception as error:
        print(str(error), file=sys.stderr)
        sys.exit(1)
