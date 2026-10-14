#!/usr/bin/env python3
"""Exercise real Fleet, PostgreSQL, nginx and two nodes using disposable local PKI."""
import argparse
import base64
import hashlib
import hmac
import http.client
import json
import pathlib
import secrets
import shutil
import socket
import ssl
import struct
import subprocess
import tempfile
import time
import urllib.parse


def command(*args, capture=False):
    return subprocess.run(args, check=True, text=True, stdout=subprocess.PIPE if capture else None).stdout


def wait_for(check, description, attempts=90):
    for _ in range(attempts):
        result = check()
        if result:
            return result
        time.sleep(2)
    raise AssertionError(description)


class Smoke:
    def __init__(self, args):
        self.args = args
        self.path = pathlib.Path(tempfile.mkdtemp(prefix='telemt-managed-smoke-'))
        self.path.chmod(0o755)
        self.project = 'telemt-smoke-' + secrets.token_hex(4)
        self.files = []
        self.cookie = self.csrf = ''
        self.nodes = []

    def compose(self, filename, *args, capture=False):
        return command('docker', 'compose', '-f', str(self.path / filename), *args, capture=capture)

    def setup(self):
        def openssl(*args):
            subprocess.run(['openssl', *args], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, cwd=self.path)
        openssl('req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-keyout', 'ca.key', '-out', 'ca.pem', '-days', '3', '-subj', '/CN=Telemt Test CA', '-addext', 'basicConstraints=critical,CA:TRUE', '-addext', 'keyUsage=critical,keyCertSign,cRLSign')
        for name, host in [('fleet', 'fleet.test'), ('node', 'node.test'), ('renewed', 'node.test')]:
            openssl('req', '-newkey', 'rsa:2048', '-nodes', '-keyout', name + '.key', '-out', name + '.csr', '-subj', '/CN=' + host)
            (self.path / (name + '.ext')).write_text(f'subjectAltName=DNS:{host}\nextendedKeyUsage=serverAuth\nbasicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\n')
            openssl('x509', '-req', '-in', name + '.csr', '-CA', 'ca.pem', '-CAkey', 'ca.key', '-CAcreateserial', '-out', name + '.pem', '-days', '2', '-extfile', name + '.ext')
        (self.path / 'master.key').write_bytes(secrets.token_bytes(32))
        (self.path / 'password').write_text(secrets.token_hex(24))
        # Fixtures are temporary test credentials; the unprivileged Fleet container must read them.
        for file in self.path.iterdir():
            file.chmod(0o644)
        (self.path / 'nginx.conf').write_text('events {}\nhttp { server { listen 18443 ssl; server_name fleet.test; ssl_certificate /fixtures/fleet.pem; ssl_certificate_key /fixtures/fleet.key; client_max_body_size 2m; location / { proxy_pass http://admin:8080; proxy_set_header Host $host; } } }')
        compose = {
            'name': self.project,
            'services': {
                'postgres': {'image': 'postgres:16-bookworm', 'environment': {'POSTGRES_USER': 'telemt', 'POSTGRES_DB': 'telemt', 'POSTGRES_PASSWORD': 'local-test-only'}, 'networks': ['test'], 'healthcheck': {'test': ['CMD-SHELL', 'pg_isready -U telemt'], 'interval': '2s', 'timeout': '2s', 'retries': 30}},
                'admin': {'image': self.args.admin_image, 'environment': {'ADMIN_ORIGIN': 'https://fleet.test:18443', 'ADMIN_LISTEN': '0.0.0.0:8080', 'DATABASE_URL': 'postgresql://telemt:local-test-only@postgres/telemt', 'ADMIN_MASTER_KEY_FILE': '/fixtures/master.key', 'MANAGED_CA_FILE': '/fixtures/ca.pem', 'MANAGED_NODE_IMAGE': 'ghcr.io/test/telemt-managed-node@sha256:' + 'a' * 64}, 'volumes': [str(self.path) + ':/fixtures:ro'], 'networks': ['test'], 'depends_on': {'postgres': {'condition': 'service_healthy'}}},
                'nginx': {'image': 'nginx:1.28-bookworm', 'volumes': [str(self.path) + ':/fixtures:ro', str(self.path / 'nginx.conf') + ':/etc/nginx/nginx.conf:ro'], 'ports': [f'127.0.0.1:{self.args.port}:18443'], 'networks': {'test': {'aliases': ['fleet.test']}}, 'depends_on': ['admin']},
            },
            'networks': {'test': {'name': self.project, 'ipam': {'config': [{'subnet': self.args.subnet + '.0/24'}]}}},
        }
        (self.path / 'fleet.json').write_text(json.dumps(compose))
        self.files.append('fleet.json')
        self.compose('fleet.json', 'up', '-d')
        self.context = ssl.create_default_context(cafile=str(self.path / 'ca.pem'))
        wait_for(self.health, 'Fleet failed to start')
        owner = json.loads(self.compose('fleet.json', 'exec', '-T', 'admin', '/app/telemt-admin', 'create-owner', 'owner', '/fixtures/password', capture=True))
        secret = owner['totp_secret']
        secret = base64.b32decode(secret + '=' * (-len(secret) % 8))
        digest = hmac.new(secret, struct.pack('>Q', int(time.time()) // 30), hashlib.sha1).digest()
        offset = digest[-1] & 15
        code = str((int.from_bytes(digest[offset:offset + 4], 'big') & 0x7fffffff) % 1000000).zfill(6)
        session, cookie = self.request('/api/login', 'POST', {'username': 'owner', 'password': (self.path / 'password').read_text(), 'totp': code})
        self.cookie = cookie.split(';')[0]
        self.csrf = session['csrf']

    def request(self, path, method='GET', body=None, revision=None, status=200):
        connection = http.client.HTTPSConnection('fleet.test', context=self.context, timeout=20)
        connection.sock = self.context.wrap_socket(socket.create_connection(('127.0.0.1', self.args.port), timeout=20), server_hostname='fleet.test')
        headers = {'Origin': 'https://fleet.test:18443', 'Content-Type': 'application/json', 'Cookie': self.cookie, 'X-CSRF-Token': self.csrf}
        if revision is not None:
            headers['If-Match'] = str(revision)
        connection.request(method, path, None if body is None else json.dumps(body), headers)
        response = connection.getresponse()
        raw = response.read()
        cookie = response.getheader('set-cookie')
        connection.close()
        assert response.status == status, (path, response.status, status)
        data = json.loads(raw)
        return data.get('data'), cookie

    def api(self, path, method='GET', body=None, revision=None, status=200):
        return self.request('/api/' + path, method, body, revision, status)[0]

    def health(self):
        try:
            self.request('/api/session', status=200 if self.cookie else 401)
            return True
        except (OSError, ValueError, AssertionError):
            return False

    def profile(self):
        return next(row for row in self.api('profiles') if row['id'] == self.pid)

    def applied(self, revisions=None):
        def check():
            profile = self.profile()
            members = profile['nodes']
            if len(members) != len(self.nodes):
                return None
            return profile if all(n['status'] == 'applied' and (revisions is None or n['applied_revision'] > revisions[n['id']]) for n in members) else None
        return wait_for(check, 'Nodes did not acknowledge the desired deployment')

    def read(self, node, path):
        data = self.api(f'nodes/{node}/read?path=' + urllib.parse.quote(path, safe=''))
        assert data['status'] == 200, (path, data['status'])
        return data['response']['data']

    def add_node(self, suffix, number):
        ip = self.args.subnet + '.' + str(number)
        node = self.api('managed-nodes', 'POST', {'name': 'Local node ' + suffix, 'ip': ip, 'profile_id': self.pid})
        source = node['compose']
        # Only the test network, local image and test PKI differ from the wizard's exact file.
        source = source.replace('name: telemt-managed', 'name: ' + self.project + '-' + suffix)
        source = source.replace('    network_mode: host', '    networks: {test: {ipv4_address: ' + ip + '}}')
        image = source.split('    image: ')[1].splitlines()[0]
        source = source.replace('    image: ' + image, '    image: ' + self.args.node_image)
        source = source.replace('      FLEET_URL:', '      MANAGED_CA_FILE: /test-ca.pem\n      FLEET_URL:')
        source = source.replace('    volumes: [state:/var/lib/telemt-managed]', f'    volumes: [state:/var/lib/telemt-managed, {self.path}/ca.pem:/test-ca.pem:ro]')
        source += '\nnetworks:\n  test:\n    external: true\n    name: ' + self.project + '\n'
        filename = suffix + '.yml'
        (self.path / filename).write_text(source)
        (self.path / filename).chmod(0o600)
        self.files.append(filename)
        self.nodes.append(node['id'])
        self.compose(filename, 'up', '-d')
        self.applied()

    def run(self):
        self.setup()
        cert = self.api('certificates', 'POST', {'name': 'Test certificate', 'domain': 'node.test'})
        certificate_path = 'certificates/' + cert['id'] + '/import'
        self.api(certificate_path, 'POST', {'fullchain': (self.path / 'node.pem').read_text(), 'private_key': (self.path / 'node.key').read_text()})
        profile = self.api('profiles', 'POST', {'name': 'Shared profile', 'domain': 'node.test', 'certificate_id': cert['id'], 'document': {'native_port': 8443, 'users': [{'username': 'alice', 'secret': '', 'enabled': True}], 'settings': {'general': {'use_middle_proxy': False}}}})
        self.pid = profile['id']
        self.api(f'profiles/{self.pid}/publish', 'POST', {}, 1)
        self.add_node('a', 10)
        self.add_node('b', 11)
        profile = self.applied()
        self.api(f'profiles/{self.pid}', 'PATCH', {'document': profile['document']}, 1, status=409)
        print('PASS two-node enrollment, real readiness, conflict protection', flush=True)
        revisions = {n['id']: n['applied_revision'] for n in profile['nodes']}
        started = [self.read(n, '/v1/system/info')['process_started_at_epoch_secs'] for n in self.nodes]
        self.api(certificate_path, 'POST', {'fullchain': (self.path / 'renewed.pem').read_text(), 'private_key': (self.path / 'node.key').read_text()}, status=400)
        self.api(certificate_path, 'POST', {'fullchain': (self.path / 'renewed.pem').read_text(), 'private_key': (self.path / 'renewed.key').read_text()})
        profile = self.applied(revisions)
        assert started == [self.read(n, '/v1/system/info')['process_started_at_epoch_secs'] for n in self.nodes]
        assert len({n['report']['certificate_fingerprint'] for n in profile['nodes']}) == 1
        print('PASS certificate/key validation and verified TLS replacement without telemt restart', flush=True)
        links = self.api(f'profiles/{self.pid}/links', 'POST', {})
        revisions = {n['id']: n['applied_revision'] for n in profile['nodes']}
        document = profile['document']
        document['users'].append({'username': 'bob', 'secret': '', 'enabled': True})
        edited = self.api(f'profiles/{self.pid}', 'PATCH', {'document': document}, profile['draft_revision'])
        self.api(f'profiles/{self.pid}/publish', 'POST', {}, edited['draft_revision'])
        profile = self.applied(revisions)
        assert all(any(u['username'] == 'bob' for u in self.read(n, '/v1/users')) for n in self.nodes)
        revisions = {n['id']: n['applied_revision'] for n in profile['nodes']}
        self.api(f'profiles/{self.pid}/rollback', 'POST', {'revision': 1}, profile['draft_revision'])
        profile = self.applied(revisions)
        assert all(all(u['username'] != 'bob' for u in self.read(n, '/v1/users')) for n in self.nodes)
        assert links == self.api(f'profiles/{self.pid}/links', 'POST', {})
        print('PASS shared user rollout and rollback preserving original client links', flush=True)
        self.compose('a.yml', 'exec', '-T', 'node', 'sh', '-c', 'printf "\\n# Local drift fixture\\n" >> /var/lib/telemt-managed/telemt.toml')
        wait_for(lambda: any(n['id'] == self.nodes[0] and n['status'] == 'drift' for n in self.profile()['nodes']), 'Local drift was not reported')
        self.api(f'nodes/{self.nodes[0]}/redeploy', 'POST', {})
        self.applied()
        content = self.compose('a.yml', 'exec', '-T', 'node', 'cat', '/var/lib/telemt-managed/telemt.toml', capture=True)
        assert 'Local drift fixture' not in content
        print('PASS drift detection and explicit profile repair', flush=True)
        self.compose('fleet.json', 'stop', 'admin')
        for filename in ['a.yml', 'b.yml']:
            self.compose(filename, 'restart', 'node')
            wait_for(lambda: self.node_health(filename), 'Last-good startup without Fleet failed')
        print('PASS both nodes restart and serve while Fleet is offline', flush=True)
        self.compose('fleet.json', 'start', 'admin')
        wait_for(self.health, 'Fleet did not recover')
        self.applied()
        print('ALL MANAGED COMPOSE CHECKS PASSED', flush=True)

    def node_health(self, filename):
        try:
            self.compose(filename, 'exec', '-T', 'node', '/app/telemt-node-agent', 'healthcheck', capture=True)
            return True
        except subprocess.CalledProcessError:
            return False

    def cleanup(self):
        if self.args.keep:
            print('Retained isolated test fixtures:', self.path)
            return
        for filename in reversed(self.files):
            subprocess.run(['docker', 'compose', '-f', str(self.path / filename), 'down', '-v', '--remove-orphans'], check=False)
        shutil.rmtree(self.path)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--admin-image', default='telemt-admin:managed-test')
    parser.add_argument('--node-image', default='telemt-managed-node:local')
    parser.add_argument('--port', type=int, default=18444)
    parser.add_argument('--subnet', default='172.30.91')
    parser.add_argument('--keep', action='store_true', help='Keep disposable containers and test credentials for diagnosis')
    test = Smoke(parser.parse_args())
    try:
        test.run()
    finally:
        test.cleanup()


if __name__ == '__main__':
    main()
