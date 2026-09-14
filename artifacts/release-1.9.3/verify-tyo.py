import hashlib
import json
from pathlib import Path
import re
import subprocess
import urllib.error
import urllib.request

release_root = Path('/home/ikaleio/projects/monoize/.git/deploy-worktrees/v1.9.3')
expected_commit = 'c0bd5e193a0f45351ac7b24dc70734658f29fd17'

def output(*args):
    return subprocess.check_output(args, text=True).strip()

def digest(path):
    result = hashlib.sha256()
    with open(path, 'rb') as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b''):
            result.update(chunk)
    return result.hexdigest()

def request(url, expected_status):
    try:
        response = urllib.request.urlopen(url, timeout=20)
    except urllib.error.HTTPError as error:
        response = error
    with response:
        status = response.status
        body = response.read()
    print(json.dumps({'url': url, 'status': status, 'bytes': len(body)}), flush=True)
    assert status == expected_status, (url, status, expected_status)
    return body

commit = output('git', '-C', str(release_root), 'rev-parse', 'HEAD')
assert commit == expected_commit, commit
subprocess.run(['git', '-C', str(release_root), 'diff', '--exit-code', '--', 'Cargo.toml', 'Cargo.lock', 'src', 'frontend'], check=True)
print(json.dumps({'commit': commit}), flush=True)
processes = json.loads(output('pm2', 'jlist'))
process = next(item for item in processes if item['name'] == 'monoize')
environment = process['pm2_env']
pid = process['pid']
assert environment['status'] == 'online'
assert environment['pm_exec_path'] == '/opt/monoize/monoize'
print(json.dumps({'pid': pid, 'status': environment['status'], 'restart_count': environment.get('restart_time'), 'uptime': environment.get('pm_uptime')}), flush=True)
hashes = {str(path): digest(path) for path in [release_root / 'target/release/monoize', Path('/opt/monoize/monoize'), Path(f'/proc/{pid}/exe')]}
print(json.dumps({'sha256': hashes}), flush=True)
assert len(set(hashes.values())) == 1

asset_paths = []
for base in ['http://127.0.0.1:40550', 'https://mono.ikale.io']:
    home = request(base + '/', 200).decode()
    settings = json.loads(request(base + '/api/dashboard/settings/public', 200))
    assert settings['api_base_url'] == 'https://mono.ikale.io/'
    request(base + '/v1/models', 401)
    match = re.search(r'<script\b[^>]*\bsrc="([^"]+\.js[^"]*)"', home)
    assert match is not None, 'Missing served JavaScript asset'
    asset_path = match.group(1)
    assert asset_path.startswith('/assets/') and '..' not in asset_path
    asset_paths.append(asset_path)
    body = request(base + asset_path, 200)
    served_hash = hashlib.sha256(body).hexdigest()
    built_hash = digest(release_root / 'frontend/dist' / asset_path.lstrip('/'))
    assert served_hash == built_hash, (asset_path, served_hash, built_hash)
    print(json.dumps({'asset': asset_path, 'sha256': served_hash, 'matches_build': True}), flush=True)
assert len(set(asset_paths)) == 1
print('TYO_RELEASE_VERIFIED', flush=True)
