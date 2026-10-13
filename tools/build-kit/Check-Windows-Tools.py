"""Check Flutter's symlink requirement using disposable files only."""
from pathlib import Path
import os
import shutil
import tempfile

seven_zip = shutil.which('7z.exe') or str(Path(os.environ['ProgramFiles']) / '7-Zip/7z.exe')
if not Path(seven_zip).is_file():
    raise SystemExit('The build machine needs 7-Zip to package the built-in X11 runtime. AetherDesk end users do not need it.')

with tempfile.TemporaryDirectory(prefix="aetherdesk-symlink-check-") as temp:
    root = Path(temp)
    source = root / "source.txt"
    link = root / "link.txt"
    source.write_text("AetherDesk build probe", encoding="utf-8")
    try:
        link.symlink_to(source)
    except OSError as error:
        raise SystemExit("Flutter needs Windows Developer Mode enabled: Settings > For developers > Developer Mode. " + str(error))
    assert link.read_text(encoding="utf-8") == "AetherDesk build probe"
print("Windows symbolic links and Python checked.")
