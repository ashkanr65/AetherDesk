import importlib.util
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("cleanup", Path(__file__).with_name("project_cleanup_linux.py"))
cleanup = importlib.util.module_from_spec(spec)
spec.loader.exec_module(cleanup)


def info(project="aetherdesk", task="aetherdesk-fixture1234", role="disposable", **extra):
    value = {"Config": {"Labels": {cleanup.LABEL_PROJECT: project, cleanup.LABEL_TASK: task, cleanup.LABEL_ROLE: role}}, "State": {"Running": False}}
    value.update(extra)
    return value


class FakeStore:
    def __init__(self):
        self.containers = {}; self.images = {}; self.deleted = []
    def inventory(self):
        return ([{"Id": key} for key in self.containers], [{"Id": key} for key in self.images])
    def inspect(self, kind, resource):
        return (self.containers if kind == "container" else self.images)[resource]
    def remove(self, kind, resource):
        self.deleted.append((kind, resource))
        del (self.containers if kind == "container" else self.images)[resource]


class CleanupTests(unittest.TestCase):
    def test_short_socket_paths_preserve_legacy_task_ownership_without_arbitrary_roots(self):
        task="aetherdesk-fixture1234"
        short=cleanup.short_task_directory(task)
        self.assertLessEqual(len(str(short)),20)
        self.assertEqual(cleanup.task_directory("aetherdesk",task,str(short)),short)
        self.assertEqual(cleanup.task_directory("aetherdesk",task),Path("/var/tmp/aether-project-tasks/aetherdesk")/task)
        for path in ["/", "/tmp", str(short.parent), str(short)+"/../other", str(cleanup.short_task_directory("openeda-other1234"))]:
            with self.assertRaises(ValueError):cleanup.task_directory("aetherdesk",task,path)
    def test_dry_run_has_no_mutations_and_exact_owner_is_required(self):
        store=FakeStore()
        store.images={"a"*64:info(), "b"*64:info(project="openeda"), "c"*64:info(task="aetherdesk-another1234"), "d"*64:{"Config":{"Labels":{}}}}
        result=cleanup.clean_resources(store,"aetherdesk","aetherdesk-fixture1234",set())
        self.assertEqual(store.deleted,[])
        self.assertEqual([r["id"] for r in result["removed"]],["a"*64])
        cleanup.clean_resources(store,"aetherdesk","aetherdesk-fixture1234",set(),True)
        self.assertEqual(store.deleted,[("image","a"*64)])

    def test_runtime_active_and_referenced_resources_are_preserved(self):
        store=FakeStore()
        store.images={"a"*64:info(),"b"*64:info(role="runtime"),"c"*64:info()}
        store.containers={"1"*64:info(Image="a"*64,State={"Running":True}),"2"*64:info(project="openeda",Image="c"*64)}
        cleanup.clean_resources(store,"aetherdesk","aetherdesk-fixture1234",set(),True)
        self.assertEqual(store.deleted,[])

    def test_exact_inactive_container_is_removed_before_its_image(self):
        store=FakeStore();store.images={"a"*64:info()};store.containers={"1"*64:info(Image="a"*64)}
        cleanup.clean_resources(store,"aetherdesk","aetherdesk-fixture1234",set(),True)
        self.assertEqual(store.deleted,[("container","1"*64),("image","a"*64)])

    def test_protected_claims_and_busy_external_builds_are_not_deleted(self):
        store=FakeStore();store.containers={"1"*64:info(external=True,Image="a"*64)};store.images={"a"*64:info()}
        cleanup.clean_resources(store,"aetherdesk","aetherdesk-fixture1234",set(),True,build_busy=True)
        self.assertEqual(store.deleted,[])
        cleanup.clean_resources(store,"aetherdesk","aetherdesk-fixture1234",{"1"*64,"a"*64},True,build_busy=False)
        self.assertEqual(store.deleted,[])

    def test_paths_reject_roots_escapes_and_nested_links(self):
        with tempfile.TemporaryDirectory() as name, tempfile.TemporaryDirectory() as other:
            root=Path(name)
            cleanup.validate_tree(root,root)
            for path,base in [(Path("/"),Path("/")),(Path(""),root),(Path(other),root),(root/".."/"outside",root)]:
                with self.assertRaises(ValueError):cleanup.validate_tree(path,base)
            (root/"link").symlink_to(other,target_is_directory=True)
            with self.assertRaises(ValueError):cleanup.validate_tree(root,root)

    def test_inspection_failure_blocks_image_deletion(self):
        store=FakeStore();store.containers={"1"*64:None};store.images={"a"*64:info()}
        original=store.inspect
        def inspect(kind,identity):
            if kind=="container":raise RuntimeError("unknown")
            return original(kind,identity)
        store.inspect=inspect
        cleanup.clean_resources(store,"aetherdesk","aetherdesk-fixture1234",set(),True)
        self.assertEqual(store.deleted,[])

    def test_git_directories_and_worktree_files_are_never_disposable(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            git = root / ".git"
            git.mkdir()
            with self.assertRaises(ValueError):
                cleanup.validate_tree(root, root, for_deletion=True)
            git.rmdir()
            git.write_text("gitdir: /preserved/repository")
            with self.assertRaises(ValueError):
                cleanup.validate_tree(root, root, for_deletion=True)
            self.assertTrue(git.exists())

    def test_external_metadata_preserves_its_backing_image_without_adopting_the_container(self):
        store=FakeStore();store.images={"a"*64:info(),"b"*64:info()}
        store.inventory=lambda: ([{"Id":"1"*64,"State":"storage","Labels":None,"ImageID":"a"*64}], [{"Id":i} for i in store.images])
        original=store.inspect
        def inspect(kind,identity):
            if kind=="container":raise RuntimeError("Buildah storage container")
            return original(kind,identity)
        store.inspect=inspect
        cleanup.clean_resources(store,"aetherdesk","aetherdesk-fixture1234",set(),True)
        self.assertEqual(store.deleted,[("image","b"*64)])


if __name__ == "__main__":unittest.main(verbosity=2)
