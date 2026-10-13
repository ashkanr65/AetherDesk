"""Package the pinned, self-contained LangGraph engine without running pip."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import stat
import tempfile
import urllib.parse
import urllib.request
import zipfile

PIN = Path(__file__).with_name('assistant-runtime.json')
SERVER = Path(__file__).resolve().parents[1] / 'assistant-runtime' / 'server.py'


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open('rb') as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b''):
            digest.update(block)
    return digest.hexdigest()


def safe_relative(name: str) -> bool:
    return (isinstance(name, str) and bool(name) and '\\' not in name and ':' not in name
            and not name.startswith('/') and all(part not in ('', '.', '..')
                and part[-1:] not in (' ', '.')
                and not re.fullmatch(r'(?i)(con|prn|aux|nul|com[1-9]|lpt[1-9])(\..*)?', part)
                for part in name.rstrip('/').split('/')))


def read_pin(path: Path = PIN) -> dict:
    pin = json.loads(path.read_text(encoding='utf-8-sig'))
    if (pin.get('schemaVersion') != 1 or pin.get('engine') != 'langgraph'
            or pin.get('version') != '1' or pin.get('platform') != 'windows-x64'
            or pin.get('executable') != 'python/python.exe' or pin.get('entrypoint') != 'server.py'
            or not re.fullmatch(r'\d+\.\d+\.\d+', pin.get('langgraphVersion', ''))):
        raise ValueError('Invalid pinned assistant manifest')
    python = pin.get('python', {})
    version = python.get('version', '')
    filename = f'python-{version}-embeddable-amd64.zip'
    if (not re.fullmatch(r'3\.13\.\d+', version) or python.get('filename') != filename
            or python.get('url') != f'https://www.python.org/ftp/python/{version}/{filename}'):
        raise ValueError('Invalid pinned Python archive')
    wheels = pin.get('wheels', [])
    if not wheels or len({x.get('name') for x in wheels}) != len(wheels):
        raise ValueError('Missing or duplicate pinned assistant wheels')
    if not any(x.get('name') == 'langgraph' and x.get('version') == pin['langgraphVersion'] for x in wheels):
        raise ValueError('Pinned LangGraph wheel is missing')
    filenames = set()
    licenses = pin.get('licenses', [])
    if len(licenses) != 1 or licenses[0].get('filename') != 'langsmith-LICENSE.txt' or licenses[0].get('url') != 'https://raw.githubusercontent.com/langchain-ai/langsmith-sdk/v0.14.2/LICENSE':
        raise ValueError('Pinned LangSmith license is missing')
    for artifact in [python, *wheels, *licenses]:
        filename = artifact.get('filename', '')
        url = urllib.parse.urlparse(artifact.get('url', ''))
        if (not safe_relative(filename) or '/' in filename or filename.casefold() in filenames
                or not re.fullmatch(r'[a-f0-9]{64}', artifact.get('sha256', ''))
                or url.scheme != 'https' or url.username or url.password or url.query or url.fragment
                or (artifact not in licenses and url.path.rsplit('/', 1)[-1] != filename)):
            raise ValueError('Invalid pinned assistant artifact')
        if artifact in wheels and (url.hostname != 'files.pythonhosted.org' or not filename.endswith('.whl')):
            raise ValueError('Assistant wheels must come from official PyPI files')
        filenames.add(filename.casefold())
    return pin


def verified_download(artifact: dict, cache: Path) -> Path:
    cache.mkdir(parents=True, exist_ok=True)
    name = artifact['filename']
    destination = cache / name
    if destination.exists():
        if destination.is_symlink() or not destination.is_file() or sha256(destination) != artifact['sha256']:
            raise ValueError(f'Cached assistant artifact failed SHA-256 verification: {name}')
        return destination
    with tempfile.NamedTemporaryFile(dir=cache, prefix=name+'.', suffix='.partial', delete=False) as output:
        partial = Path(output.name)
        try:
            request = urllib.request.Request(artifact['url'], headers={'User-Agent':'AetherDesk-build'})
            with urllib.request.urlopen(request, timeout=90) as response:
                if urllib.parse.urlparse(response.url).scheme != 'https':
                    raise ValueError('Assistant download redirected away from HTTPS')
                shutil.copyfileobj(response, output)
            output.close()
            if sha256(partial) != artifact['sha256']:
                raise ValueError(f'Downloaded assistant artifact failed SHA-256 verification: {name}')
            os.replace(partial, destination)
        finally:
            output.close()
            partial.unlink(missing_ok=True)
    return destination


def extract_archive(archive: Path, destination: Path, *, wheel: bool = False) -> None:
    with zipfile.ZipFile(archive) as zipped:
        entries = zipped.infolist()
        seen = set()
        if len(entries) > 25000 or sum(x.file_size for x in entries) > 512 * 1024 * 1024:
            raise ValueError('Assistant archive exceeds extraction limits')
        for entry in entries:
            name = entry.filename
            if (not safe_relative(name) or name.rstrip('/').casefold() in seen
                    or stat.S_ISLNK(entry.external_attr >> 16)):
                raise ValueError(f'Unsafe assistant archive entry: {name}')
            seen.add(name.rstrip('/').casefold())
            target = destination.joinpath(*PurePosixPath(name).parts)
            if not entry.is_dir() and target.exists():
                raise ValueError(f'Assistant archive would overwrite another package: {name}')
        for entry in entries:
            target = destination.joinpath(*PurePosixPath(entry.filename).parts)
            if entry.is_dir():
                target.mkdir(parents=True, exist_ok=True)
            else:
                target.parent.mkdir(parents=True, exist_ok=True)
                with zipped.open(entry) as source, target.open('xb') as output:
                    shutil.copyfileobj(source, output)


def bundle(destination: Path, cache: Path, pin: dict, server: Path = SERVER) -> Path:
    destination = destination.resolve()
    runtime = destination / 'ai' / 'assistant'
    if runtime.exists() or runtime.is_symlink():
        raise ValueError(f'Refusing to overlay an existing assistant bundle: {runtime}')
    if not server.is_file():
        raise ValueError(f'Assistant server source is missing: {server}')
    artifacts = [(x, verified_download(x, cache)) for x in [pin['python'], *pin['wheels']]]
    licenses = [(x, verified_download(x, cache)) for x in pin.get('licenses', [])]
    ai = destination / 'ai'
    if ai.is_symlink():
        raise ValueError('Assistant package directory must not be a symbolic link')
    ai.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='.assistant-', dir=ai) as temporary:
        staged = Path(temporary) / 'assistant'
        staged.mkdir()
        extract_archive(artifacts[0][1], staged / 'python')
        for _, archive in artifacts[1:]:
            extract_archive(archive, staged / 'packages', wheel=True)
        executable = staged / 'python' / 'python.exe'
        if not executable.is_file() or executable.read_bytes()[:2] != b'MZ':
            raise ValueError('Python archive does not contain a Windows executable')
        if not (staged / 'python' / 'LICENSE.txt').is_file():
            raise ValueError('Python archive is missing its license')
        # Embedded Python ignores registry, PYTHONPATH, user-site and cwd. Never
        # enable import site: only this package's audited paths are importable.
        (staged / 'python' / 'python313._pth').write_text('python313.zip\n.\n../packages\n..\n', encoding='utf-8')
        shutil.copyfile(server, staged / 'server.py')
        (staged / 'runtime-lock.json').write_text(json.dumps(pin, indent=2)+'\n', encoding='utf-8')
        (staged / 'licenses').mkdir()
        for artifact, file in licenses:
            shutil.copyfile(file, staged / 'licenses' / artifact['filename'])
        files = [{'path':x.relative_to(staged).as_posix(), 'sha256':sha256(x), 'size':x.stat().st_size}
                 for x in sorted(staged.rglob('*')) if x.is_file()]
        metadata = {'schemaVersion':1, 'engine':'langgraph', 'version':'1', 'platform':pin['platform'],
                    'executable':'python/python.exe', 'entrypoint':'server.py',
                    'pythonVersion':pin['python']['version'], 'langgraphVersion':pin['langgraphVersion'],
                    'files':files}
        (staged / 'bundle-info.json').write_text(json.dumps(metadata, indent=2)+'\n', encoding='utf-8')
        staged.rename(runtime)
    return runtime


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--destination', type=Path, required=True)
    parser.add_argument('--cache-directory', type=Path, required=True)
    args = parser.parse_args()
    runtime = bundle(args.destination, args.cache_directory, read_pin())
    print(f'Bundled self-contained LangGraph assistant in {runtime}')


if __name__ == '__main__':
    main()
