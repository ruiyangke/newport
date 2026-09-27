"""Configure an ephemeral macOS signing keychain on a release runner."""
import base64, os, pathlib, plistlib, secrets, subprocess
root = pathlib.Path(os.environ['RUNNER_TEMP'])
for name in ['APPLE_CERTIFICATE', 'APPLE_PROVISIONING_PROFILE', 'APPLE_ENTITLEMENTS']:
    if not os.environ.get(name): raise SystemExit('Missing secret: ' + name)
cert = root / 'signing.p12'
cert.write_bytes(base64.b64decode(os.environ['APPLE_CERTIFICATE']))
profile = root / 'release.provisionprofile'
profile.write_bytes(base64.b64decode(os.environ['APPLE_PROVISIONING_PROFILE']))
ent = root / 'release.entitlements'
ent.write_text(os.environ['APPLE_ENTITLEMENTS'])
assert not plistlib.loads(ent.read_bytes()).get('com.apple.security.get-task-allow')
kc = str(root / 'release.keychain-db')
pw = secrets.token_urlsafe(32)
def run(*args):
    result = subprocess.run(args, stdout=subprocess.DEVNULL)
    if result.returncode: raise SystemExit("Signing setup failed: " + args[0])
run('security', 'create-keychain', '-p', pw, kc)
run('security', 'set-keychain-settings', '-lut', '7200', kc)
run('security', 'unlock-keychain', '-p', pw, kc)
run('security', 'import', str(cert), '-k', kc, '-P', os.environ['APPLE_CERTIFICATE_PASSWORD'], '-T', '/usr/bin/codesign')
run('security', 'set-key-partition-list', '-S', 'apple-tool:,apple:,codesign:', '-s', '-k', pw, kc)
run('security', 'list-keychains', '-d', 'user', '-s', kc)
with open(os.environ['GITHUB_ENV'], 'a') as out:
    out.write(f'PORTHOP_PROVISIONING_PROFILE={profile}\nPORTHOP_SIGNING_ENTITLEMENTS={ent}\n')
