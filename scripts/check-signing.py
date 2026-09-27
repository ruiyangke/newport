"""Reject incompatible signing identities before building a rebranded macOS app."""
import fnmatch
import json
from pathlib import Path
import plistlib
import subprocess
import sys


def validate(entitlements, profile, identifier):
    allowed = profile['Entitlements']
    app_id = entitlements.get('com.apple.application-identifier', entitlements.get('application-identifier', ''))
    if not app_id.endswith('.' + identifier):
        raise ValueError(f'Signing entitlements must identify {identifier}')
    profile_id = allowed.get('com.apple.application-identifier', allowed.get('application-identifier', ''))
    if not fnmatch.fnmatchcase(app_id, profile_id):
        raise ValueError('Provisioning profile does not authorize the app identifier')
    groups = entitlements.get('keychain-access-groups', [])
    permitted = allowed.get('keychain-access-groups', [])
    if not groups or not groups[0].endswith('.' + identifier):
        raise ValueError('The first Keychain group must be the Newport group')
    if not any(group.endswith('.com.porthop.desktop') for group in groups):
        raise ValueError('Retain the previous com.porthop.desktop Keychain group for credential migration')
    if any(not any(fnmatch.fnmatchcase(group, pattern) for pattern in permitted) for group in groups):
        raise ValueError('Provisioning profile does not authorize all migration Keychain groups')


if __name__ == '__main__':
    entitlements = plistlib.loads(Path(sys.argv[1]).read_bytes())
    profile = plistlib.loads(subprocess.check_output(['security', 'cms', '-D', '-i', sys.argv[2]], stderr=subprocess.DEVNULL))
    identifier = json.loads((Path(__file__).resolve().parents[1] / 'src-tauri/tauri.conf.json').read_text())['identifier']
    try:
        validate(entitlements, profile, identifier)
    except ValueError as error:
        raise SystemExit(str(error)) from error
