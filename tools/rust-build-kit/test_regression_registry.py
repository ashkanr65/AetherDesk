import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location("registry", Path(__file__).with_name("check_regression_registry.py"))
registry = importlib.util.module_from_spec(spec)
spec.loader.exec_module(registry)


class RegistryTests(unittest.TestCase):
    def setUp(self):
        self.catalog = {"features": [{"checks": [
            {"phase": "linux", "test": "first"},
            {"phase": "linux", "test": "second"},
            {"phase": "package", "test": "bundled"}]}]}

    def test_empty_or_partial_registry_never_counts_as_coverage(self):
        for text in ["", "first: test\n", "not_first: test\nsecond: test\n"]:
            with self.assertRaises(ValueError):
                registry.verify(self.catalog, "linux", text)

    def test_registrations_are_not_passing_or_live_acceptance(self):
        result = registry.verify(self.catalog, "linux", "one::first: test\ntwo::second: test\n")
        self.assertEqual(result["required"], 2)
        self.assertFalse(result["compilerTestsPassed"])
        self.assertFalse(result["liveAcceptancePassed"])
        self.assertFalse(result["replacementReady"])
        with self.assertRaises(ValueError):
            registry.verify(self.catalog, "unknown", "first: test\nsecond: test\n")


if __name__ == "__main__":
    unittest.main()
