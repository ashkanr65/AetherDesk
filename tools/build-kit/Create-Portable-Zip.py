"""Archive every packaged file, including hidden files, and verify the ZIP."""
import pathlib
import sys
import subprocess
import zipfile

source = pathlib.Path(sys.argv[1]).resolve(strict=True)
destination = pathlib.Path(sys.argv[2])
if not (source / 'aetherdesk.exe').is_file():
    raise SystemExit('The portable folder does not contain aetherdesk.exe')
subprocess.run([sys.executable, '-X', 'utf8', str(pathlib.Path(__file__).with_name('Verify-Assistant.py')),
                '--package', str(source), '--files-only'], check=True)
with zipfile.ZipFile(destination, 'x', compression=zipfile.ZIP_DEFLATED,
                     compresslevel=6, allowZip64=True) as archive:
    for path in sorted(source.rglob('*')):
        if path.is_file():
            archive.write(path, pathlib.PurePosixPath('AetherDesk') / path.relative_to(source).as_posix())
with zipfile.ZipFile(destination) as archive:
    bad = archive.testzip()
    if bad:
        raise SystemExit('ZIP verification failed for ' + bad)
print('Portable ZIP created and verified:', destination)
