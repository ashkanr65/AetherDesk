import importlib.util
import os
from pathlib import Path
import tempfile
import unittest
from unittest import mock

spec = importlib.util.spec_from_file_location("x11_icons", Path(__file__).with_name("x11-icons.py"))
icons = importlib.util.module_from_spec(spec)
spec.loader.exec_module(icons)


class IconTests(unittest.TestCase):
    def test_agent_owned_helper_has_no_implicit_parent_watch(self):
        with mock.patch.dict(os.environ, {}, clear=True), mock.patch("threading.Thread") as thread:
            icons.watch_parent_process()
            thread.assert_not_called()

    def test_anchor_identity_is_checked_and_pid_reuse_exits(self):
        stat = "123 (a shell with spaces) " + " ".join(["0"] * 19 + ["100"])
        with mock.patch.dict(os.environ, {"AETHERDESK_X11_PARENT": "123"}), \
             mock.patch("builtins.open", mock.mock_open(read_data=stat)), \
             mock.patch("threading.Thread") as thread:
            icons.watch_parent_process()
            self.assertNotIn("AETHERDESK_X11_PARENT", os.environ)
            watch = thread.call_args.kwargs["target"]
        with mock.patch("builtins.open", mock.mock_open(read_data=stat[:-3] + "200")), \
             mock.patch("time.sleep"), mock.patch.object(os, "_exit", side_effect=SystemExit) as leave:
            with self.assertRaises(SystemExit):
                watch()
            leave.assert_called_once_with(0)

    def test_missing_anchor_exits_without_starting_a_thread(self):
        with mock.patch.dict(os.environ, {"AETHERDESK_X11_PARENT": "123"}), \
             mock.patch("builtins.open", side_effect=FileNotFoundError), \
             mock.patch.object(os, "_exit", side_effect=SystemExit), mock.patch("threading.Thread") as thread:
            with self.assertRaises(SystemExit):
                icons.watch_parent_process()
            thread.assert_not_called()

    def test_frequent_title_changes_do_not_trigger_network_icon_lookups(self):
        x = icons.XIcons.__new__(icons.XIcons)
        x.x = mock.Mock()
        x.display = 1
        x.wm_class, x.wm_hints, x.net_icon, x.net_pid = 1, 2, 3, 4
        for atom, expected in [(5, set()), (6, set()), (1, {77}), (3, {77})]:
            x.x.XPending.side_effect = [1, 1, 0]
            def event(_display, pointer):
                pointer._obj.type = 28
                pointer._obj.property.atom = atom
                pointer._obj.property.window = 77
            x.x.XNextEvent.side_effect = event
            self.assertEqual(x.changed_windows(), expected)
        x.x.XGetWindowProperty.assert_not_called()

    def test_idle_monitor_waits_on_socket_without_querying_windows(self):
        x = icons.XIcons.__new__(icons.XIcons)
        x.x = mock.Mock()
        x.display = 1
        x.x.XPending.return_value = 0
        x.x.XConnectionNumber.return_value = 7
        with mock.patch.object(icons.select, "select", return_value=([], [], [])) as wait:
            self.assertEqual(x.changed_windows(), set())
            wait.assert_called_once_with([7], [], [])
        x.x.XGetWindowProperty.assert_not_called()
        x.x.XQueryTree.assert_not_called()

    def test_desktop_entry_matches_wm_class_and_resolves_theme(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            apps = root / "applications"
            apps.mkdir()
            (apps / "cadence-virtuoso.desktop").write_text(
                "[Desktop Entry]\nName=Virtuoso\nStartupWMClass=Virtuoso\nIcon=cadence-virtuoso\n"
            )
            artwork = root / "icons/hicolor/48x48/apps/cadence-virtuoso.png"
            artwork.parent.mkdir(parents=True)
            artwork.touch()
            entries = icons.desktop_icons([root])
            self.assertEqual(icons.find_icon(["Virtuoso"], entries, [root], {}), artwork)
            self.assertIsNone(icons.find_icon(["../../missing"], entries, [root], {}))

    def test_user_mapping_handles_site_specific_cadence_classes(self):
        with tempfile.TemporaryDirectory() as directory:
            artwork = Path(directory) / "innovus.xpm"
            artwork.touch()
            self.assertEqual(icons.find_icon(["CadenceCustom"], {}, [], {"cadencecustom": str(artwork)}), artwork)

    def test_cadence_artwork_near_installed_executable(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            executable = root / "bin/virtuoso"
            executable.parent.mkdir()
            executable.touch()
            artwork = root / "tools/dfII/etc/icons/virtuoso.xpm"
            artwork.parent.mkdir(parents=True)
            artwork.touch()
            with mock.patch.object(icons.shutil, "which", return_value=str(executable)):
                self.assertEqual(icons.find_icon(["Virtuoso"], {}, [], {}), artwork)

    def test_clock_is_a_complete_argb_icon(self):
        pixels = icons.clock_icon()
        self.assertEqual(pixels[:2], [48, 48])
        self.assertEqual(len(pixels), 2 + 48 * 48)
        self.assertIn(0xff25364a, pixels)

    def test_innovus_aliases_and_pid_executable_artwork(self):
        self.assertEqual(icons.cadence_program(["encounter64"]), "innovus")
        self.assertEqual(icons.cadence_program(["Innovus 23.1 - Design"]), "innovus")
        self.assertIsNone(icons.cadence_program(["unrelated"] ))
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            executable = root / "tools.lnx86/innovus/bin/innovus"
            executable.parent.mkdir(parents=True)
            executable.touch()
            artwork = root / "tools.lnx86/innovus/etc/icons/Innovus.xpm"
            artwork.parent.mkdir(parents=True)
            artwork.touch()
            self.assertEqual(icons.find_icon(["encounter64"], {}, [], {}, executable), artwork)

    @unittest.skipUnless(os.name == "posix", "GdkPixbuf is a remote Linux dependency")
    def test_gdk_pixbuf_preserves_color_channels(self):
        with tempfile.TemporaryDirectory() as directory:
            artwork = Path(directory) / "red.ppm"
            artwork.write_bytes(b"P6\n1 1\n255\n\xff\x00\x00")
            pixels = icons.load_pixels(artwork)
            self.assertIsNotNone(pixels)
            self.assertTrue(all(pixel == 0xffff0000 for pixel in pixels[2:]))


if __name__ == "__main__":
    unittest.main()
