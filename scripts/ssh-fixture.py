#!/usr/bin/env python3
"""Local-only SSH/agent fixture. No real credentials.

Uses paramiko + cryptography to generate keys and serve protocol requests.
The server implements fixed test commands. Clipboard tests additionally run only
the repository's installer and allowlisted helper operations with an isolated HOME.
"""
import argparse
import base64
import pathlib
import os
import re
import shutil
import select
import socket
import struct
import subprocess
import sys
import pty
import fcntl
import termios
import signal
import threading
import time
import paramiko
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
from cryptography.hazmat.primitives import serialization

parser = argparse.ArgumentParser()
parser.add_argument('--directory', required=True)
directory = pathlib.Path(parser.parse_args().directory)
remote_home = directory / 'remote-home'
remote_home.mkdir()
source = pathlib.Path(__file__).resolve().parents[1] / 'src-tauri/src'
installer = (source / 'clipboard-install.sh').read_text().replace("'", "'\"'\"'")
install_command = ("sh -c '" + installer + "'").encode()
helper_command = re.compile(rb'bash "\$HOME/\.local/bin/porthop-clip" --(?:begin|receive|heartbeat|clear) [0-9a-f-]{36}')
# macOS lacks util-linux's flock executable. This test-only adapter exercises the
# same inherited-fd kernel lock; actual Linux servers use their installed flock.
fixture_path = os.environ['PATH']
if not shutil.which('flock'):
    test_bin = directory / 'test-bin'
    test_bin.mkdir()
    lock_tool = test_bin / 'flock'
    lock_tool.write_text('#!/usr/bin/env python3\nimport fcntl, sys\nfcntl.flock(int(sys.argv[2]), fcntl.LOCK_SH if sys.argv[1] == "-s" else fcntl.LOCK_EX)\n')
    lock_tool.chmod(0o700)
    fixture_path = str(test_bin) + os.pathsep + fixture_path
private = Ed25519PrivateKey.generate()
public = private.public_key().public_bytes(serialization.Encoding.OpenSSH, serialization.PublicFormat.OpenSSH)
client = directory / 'client'
client.write_bytes(private.private_bytes(serialization.Encoding.PEM, serialization.PrivateFormat.OpenSSH, serialization.NoEncryption()))
client.chmod(0o600)
encrypted = directory / 'client.encrypted'
encrypted.write_bytes(private.private_bytes(serialization.Encoding.PEM, serialization.PrivateFormat.OpenSSH, serialization.BestAvailableEncryption(b'fixture-passphrase')))
encrypted.chmod(0o600)
directory.joinpath('client.pub').write_bytes(public)
other = Ed25519PrivateKey.generate().public_key().public_bytes(serialization.Encoding.OpenSSH, serialization.PublicFormat.OpenSSH)
directory.joinpath('other.pub').write_bytes(other)
public_key = public.decode().split()[1]
key_blob = base64.b64decode(public_key)
host = paramiko.RSAKey.generate(2048)
transports = []

def string(data):
    return struct.pack('>I', len(data)) + data

def read_exact(sock, count):
    data = b''
    while len(data) < count:
        chunk = sock.recv(count - len(data))
        if not chunk:
            raise EOFError()
        data += chunk
    return data

def agent_client(sock):
    try:
        while True:
            size = struct.unpack('>I', read_exact(sock, 4))[0]
            if size > 1024 * 1024:
                break
            request = read_exact(sock, size)
            if request[0] == 11:
                reply = b'\x0c' + struct.pack('>I', 1) + string(key_blob) + string(b'fixture')
            elif request[0] == 13:
                n = struct.unpack('>I', request[1:5])[0]
                offset = 5 + n
                n = struct.unpack('>I', request[offset:offset + 4])[0]
                payload = request[offset + 4:offset + 4 + n]
                reply = b'\x0e' + string(string(b'ssh-ed25519') + string(private.sign(payload)))
            else:
                reply = b'\x05'
            sock.sendall(string(reply))
    except (EOFError, OSError):
        pass
    finally:
        sock.close()

def agent_loop(listener):
    while True:
        sock, _ = listener.accept()
        threading.Thread(target=agent_client, args=(sock,), daemon=True).start()

agent = socket.socket(socket.AF_UNIX)
agent.bind(str(directory / 'agent.sock'))
agent.listen(16)
threading.Thread(target=agent_loop, args=(agent,), daemon=True).start()

class Server(paramiko.ServerInterface):
    def __init__(self, transport):
        self.transport = transport
        self.direct = set()
        self.forwarders = []
        self.ptys = {}
    def check_channel_pty_request(self, channel, term, width, height, pixelwidth, pixelheight, modes):
        if width == 13:  # Explicit refusal case for the terminal integration test.
            return False
        master, slave = pty.openpty()
        fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack('HHHH', height, width, 0, 0))
        self.ptys[channel.chanid] = (master, slave)
        return True
    def check_channel_window_change_request(self, channel, width, height, pixelwidth, pixelheight):
        if channel.chanid in self.ptys:
            master, _ = self.ptys[channel.chanid]
            fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack('HHHH', height, width, 0, 0))
            return True
        return False
    def check_channel_shell_request(self, channel):
        if channel.chanid not in self.ptys:
            return False
        # OpenSSH can adjust the channel window before acknowledging the shell.
        adjustment = paramiko.Message()
        adjustment.add_byte(paramiko.common.cMSG_CHANNEL_WINDOW_ADJUST)
        adjustment.add_int(channel.remote_chanid)
        adjustment.add_int(1024)
        self.transport._send_user_message(adjustment)
        channel.sendall(b'early shell output\r\n')
        def shell():
            time.sleep(0.03)
            master, slave = self.ptys[channel.chanid]
            # Establish the controlling terminal in a fresh single-threaded process.
            setup = 'import os,fcntl,termios,signal; signal.signal(signal.SIGINT,signal.SIG_DFL); signal.signal(signal.SIGQUIT,signal.SIG_DFL); fcntl.ioctl(0,termios.TIOCSCTTY,0); os.execv("/bin/bash", ["bash","--noprofile","--norc","-i"])'
            process = subprocess.Popen([sys.executable, '-c', setup], stdin=slave, stdout=slave, stderr=slave,
                start_new_session=True, cwd=remote_home,
                env=dict(os.environ, HOME=str(remote_home), PS1='fixture> ', TERM='xterm-256color', HISTFILE='/dev/null'))
            os.close(slave)
            try:
                while not channel.closed:
                    for source in select.select([master, channel], [], [], 0.1)[0]:
                        if source == master:
                            try:
                                data = os.read(master, 32768)
                            except OSError:
                                data = b''
                            if not data:
                                channel.send_exit_status(process.wait(timeout=2))
                                return
                            channel.sendall(data)
                        else:
                            data = channel.recv(32768)
                            if not data:
                                return
                            os.write(master, data)
            finally:
                self.ptys.pop(channel.chanid, None)
                os.close(master)
                if process.poll() is None:
                    os.killpg(process.pid, signal.SIGKILL)
                process.wait()
                channel.close()
        threading.Thread(target=shell, daemon=True).start()
        return True
    def check_auth_publickey(self, username, key):
        return paramiko.AUTH_SUCCESSFUL if username == 'fixture' and key.get_base64() == public_key else paramiko.AUTH_FAILED
    def check_auth_password(self, username, password):
        return paramiko.AUTH_SUCCESSFUL if username == 'fixture' and password == 'fixture-password' else paramiko.AUTH_FAILED
    def get_allowed_auths(self, username):
        return 'publickey,password'
    def check_channel_request(self, kind, chanid):
        return paramiko.OPEN_SUCCEEDED if kind == 'session' else paramiko.OPEN_FAILED_ADMINISTRATIVELY_PROHIBITED
    def check_channel_direct_tcpip_request(self, chanid, origin, destination):
        self.direct.add(chanid)
        return paramiko.OPEN_SUCCEEDED
    def check_port_forward_request(self, address, port):
        listener = socket.socket()
        listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        try:
            listener.bind((address, port))
            listener.listen(8)
        except OSError:
            listener.close()
            return False
        self.forwarders.append(listener)
        def forward():
            try:
                while self.transport.is_active():
                    if not select.select([listener], [], [], 0.1)[0]:
                        continue
                    stream, origin = listener.accept()
                    channel = self.transport.open_forwarded_tcpip_channel(origin, (address, port))
                    threading.Thread(target=bridge, args=(stream, channel), daemon=True).start()
            except Exception:
                pass
            finally:
                listener.close()
        threading.Thread(target=forward, daemon=True).start()
        return port
    def check_channel_exec_request(self, channel, command):
        def run():
            time.sleep(0.03)  # Let paramiko send the exec-request success first.
            try:
                if command == b'drop':
                    channel.send_exit_status(0)
                    time.sleep(0.1)
                    for transport in list(transports):
                        transport.close()
                    return
                if command == b'printf ok':
                    channel.sendall(b'ok')
                elif command == b'stdin':
                    count = 0
                    while data := channel.recv(32768):
                        count += len(data)
                    channel.sendall(str(count).encode())
                elif command == b'fail':
                    channel.send_stderr(b'fixture failure')
                    channel.send_exit_status(7)
                    return
                elif command == b'large':
                    channel.sendall(b'x' * (5 * 1024 * 1024))
                elif command == install_command or helper_command.fullmatch(command) or command in (
                    b'"$HOME/.local/bin/xclip" -selection clipboard -o',
                    b'"$HOME/.local/bin/xclip" -o -t TARGETS',
                ):
                    payload = bytearray()
                    while data := channel.recv(32768):
                        payload.extend(data)
                        if len(payload) > 48 * 1024 * 1024:
                            raise ValueError('Fixture input too large')
                    result = subprocess.run(command.decode(), shell=True, executable='/bin/sh',
                        input=payload, capture_output=True, timeout=10,
                        env=dict(os.environ, HOME=str(remote_home), PATH=fixture_path, PORTHOP_CLIPBOARD_NATIVE="0"))
                    if result.stdout:
                        channel.sendall(result.stdout)
                    if result.stderr:
                        channel.sendall_stderr(result.stderr)
                    channel.send_exit_status(result.returncode)
                    return
                else:
                    channel.sendall(b'LISTEN 0 128 127.0.0.1:5432 0.0.0.0:* users:(("postgres",pid=812,fd=3))\n')
                channel.send_exit_status(0)
            except Exception:
                pass
            finally:
                channel.close()
        threading.Thread(target=run, daemon=True).start()
        return True

def bridge(left, right):
    try:
        while True:
            for source in select.select([left, right], [], [], 1)[0]:
                data = source.recv(65536)
                if not data:
                    return
                (right if source is left else left).sendall(data)
    except Exception:
        pass
    finally:
        left.close()
        right.close()

def echo(channel):
    try:
        while data := channel.recv(65536):
            channel.sendall(data)
    except Exception:
        pass
    finally:
        channel.close()

class FixtureSFTP(paramiko.SFTPServerInterface):
    def local(self, path):
        candidate = (remote_home / path.lstrip('/')).resolve()
        if not candidate.is_relative_to(remote_home.resolve()):
            raise OSError(13, 'Outside fixture root')
        return candidate

    def canonicalize(self, path):
        try:
            return '/' + str(self.local(path).relative_to(remote_home.resolve())).replace('\\', '/') if path not in ('.', '/') else '/'
        except OSError:
            return '/'

    def list_folder(self, path):
        try:
            result = []
            for item in self.local(path).iterdir():
                attrs = paramiko.SFTPAttributes.from_stat(item.lstat())
                attrs.filename = item.name
                result.append(attrs)
            return result
        except OSError as error:
            return paramiko.SFTPServer.convert_errno(error.errno)

    def stat(self, path):
        try:
            return paramiko.SFTPAttributes.from_stat(self.local(path).stat())
        except OSError as error:
            return paramiko.SFTPServer.convert_errno(error.errno)

    lstat = stat

    def open(self, path, flags, attr):
        try:
            fd = os.open(self.local(path), flags, attr.st_mode or 0o600)
            file = os.fdopen(fd, 'r+b' if flags & os.O_RDWR else 'wb' if flags & os.O_WRONLY else 'rb')
            handle = paramiko.SFTPHandle(flags)
            if flags & (os.O_WRONLY | os.O_RDWR): handle.writefile = file
            if not flags & os.O_WRONLY: handle.readfile = file
            return handle
        except OSError as error:
            return paramiko.SFTPServer.convert_errno(error.errno)

    def remove(self, path):
        try:
            self.local(path).unlink()
            return paramiko.SFTP_OK
        except OSError as error:
            return paramiko.SFTPServer.convert_errno(error.errno)

    def rename(self, oldpath, newpath):
        try:
            if self.local(newpath).exists(): return paramiko.SFTP_FAILURE
            self.local(oldpath).rename(self.local(newpath))
            return paramiko.SFTP_OK
        except OSError as error:
            return paramiko.SFTPServer.convert_errno(error.errno)

remote_home.joinpath('Documents').mkdir()
remote_home.joinpath('Documents/hello 世界.txt').write_text('Hello over SFTP!\n<script>inert text</script>\n')
remote_home.joinpath('binary.dat').write_bytes(bytes(range(256)))
remote_home.joinpath('large.txt').write_bytes(b'x' * (17 * 1024 * 1024))

def connection(sock):
    transport = paramiko.Transport(sock)
    transport.add_server_key(host)
    transport.set_subsystem_handler("sftp", paramiko.SFTPServer, FixtureSFTP)
    transports.append(transport)
    server = Server(transport)
    try:
        transport.start_server(server=server)
        while transport.is_active():
            channel = transport.accept(1)
            if channel is not None and channel.chanid in server.direct:
                threading.Thread(target=echo, args=(channel,), daemon=True).start()
    except Exception:
        pass
    finally:
        for listener in server.forwarders:
            listener.close()
        transport.close()

listener = socket.socket()
listener.bind(('127.0.0.1', 0))
listener.listen(32)
port = listener.getsockname()[1]
directory.joinpath('known_hosts').write_text(f'[127.0.0.1]:{port} {host.get_name()} {host.get_base64()}\n')
directory.joinpath('port').write_text(str(port))
print(port, flush=True)
while True:
    sock, _ = listener.accept()
    threading.Thread(target=connection, args=(sock,), daemon=True).start()
