"""Run against the deployed Linux agent, with real shells and isolated homes."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

AGENT = Path.home() / '.local/bin/porthop-agent'
for shell, rc_name in [('bash', '.bashrc'), ('zsh', '.zshrc'), ('fish', '.config/fish/conf.d/porthop.fish')]:
    assert shutil.which(shell), f'{shell} must be installed in the fixture'
    with tempfile.TemporaryDirectory() as directory:
        home = Path(directory)
        binary = home / '.local/bin/porthop-agent'
        binary.parent.mkdir(parents=True)
        shutil.copy2(AGENT, binary)
        rc = home / rc_name
        rc.parent.mkdir(parents=True, exist_ok=True)
        original = '# user configuration\n'
        rc.write_text(original)
        env = {**os.environ, 'HOME': directory, 'SHELL': shutil.which(shell), 'RC': str(rc)}
        for key in ['ZDOTDIR', 'XDG_CONFIG_HOME', 'DISPLAY', 'WAYLAND_DISPLAY', 'XAUTHORITY']:
            env.pop(key, None)
        def install():
            subprocess.run([str(binary), 'install'], env=env, check=True, capture_output=True)
        install()
        installed = rc.read_bytes()
        assert installed.startswith(original.encode())
        assert installed.count(b'# >>> Porthop >>>') == 1
        install()
        assert rc.read_bytes() == installed, f'{shell}: duplicate installation'
        state = home / '.cache/porthop/clipboard'
        state.mkdir(parents=True, exist_ok=True)
        state.chmod(0o700)
        (state / 'features').write_text('clipboard browser')
        (state / 'display').write_text(':99')
        source = 'source "$RC"; source "$RC";'
        script = source + (' printf "%s\\n" $DISPLAY; count (string match -- $HOME/.local/bin $PATH)' if shell == 'fish' else ' printf "%s\\n" "$DISPLAY" "$PATH"')
        args = {'bash': ['--noprofile', '--norc', '-eu'], 'zsh': ['-f'], 'fish': ['--no-config']}[shell]
        def run(script):
            return subprocess.run([shell, *args, '-c', script], env=env, check=True, capture_output=True, text=True).stdout
        output = run(script).splitlines()
        assert output[0] == ':99', (shell, output)
        assert (output[1] == '1' if shell == 'fish' else output[1].split(':').count(str(binary.parent)) == 1)
        binary.unlink()
        assert run(source + ' echo survived').strip() == 'survived'
        binary.write_text('#!/bin/sh\nexit 1\n')
        binary.chmod(0o700)
        assert run(source + ' echo survived').strip() == 'survived'
        # Replacing a read-only file by rename would succeed in this directory.
        # Installation must respect its mode and preserve the inode and bytes.
        shutil.copy2(AGENT, binary)
        rc.write_text(original)
        rc.chmod(0o444)
        before = rc.stat()
        backups = set(rc.parent.glob('*.porthop-backup-*'))
        install()
        assert rc.read_text() == original
        assert rc.stat().st_ino == before.st_ino
        assert rc.stat().st_mode == before.st_mode
        assert set(rc.parent.glob('*.porthop-backup-*')) == backups
        rc.chmod(0o600)
        rc.unlink()
        target = home / 'protected'
        target.write_text(original)
        rc.symlink_to(target)
        install()
        assert rc.is_symlink() and target.read_text() == original
    print(f'{shell}: repeated install, missing/failing agent, read-only and symlink checks passed')
