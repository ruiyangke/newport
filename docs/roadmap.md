# Roadmap

Planned work for upcoming releases. When something ships, move it to the
release notes and remove it here.

## Next release (after 0.5.0)

### Connect through `ProxyCommand`

Newport's SSH client only dials TCP. Hosts whose `~/.ssh/config` entry uses
`ProxyCommand` can't be imported or reached, and the import says they use a
jump host, which they don't. Common cases:

- OrbStack and Lima VMs (`orb`, `lima-*`): a helper program carries the SSH
  stream.
- AWS Systems Manager: `aws ssm start-session --document-name AWS-StartSSHSession`.
- Cloudflare Access: `cloudflared access ssh --hostname %h`.
- Teleport (`tsh proxy ssh`) and Google Cloud IAP (`gcloud compute start-iap-tunnel`).

Plan:

- Run the command the way OpenSSH does: through the user's shell, with `%h`,
  `%p`, `%r` and `%%` expanded, and SSH over the process's stdin and stdout.
  The SSH library already accepts any byte stream. Stop the process when the
  connection ends, and show its stderr when connecting fails.
- Store the command on the server profile. The SSH config import carries it
  over, so `orb` imports as is; the server editor shows and edits it.
- Every connection path uses it: terminals, file transfers, port forwards,
  Needle setup and updates, and Remote apps.
- `ProxyUseFdpass yes` (the proxy hands back a socket instead of relaying)
  can follow later; it's rare.

Tests: a fixture whose `ProxyCommand` relays to the SSH test server (for
example with `nc %h %p`), covering connect, terminal, files and forwarding, and
a failing command whose stderr reaches the error.

### Clearer reasons in the SSH config import

The import gives one reason, "Uses a jump host, which Newport doesn't support
yet", for both `ProxyJump` and `ProxyCommand`, and the welcome page cuts it off.

- Name the actual setting: a jump host (`ProxyJump`), or a proxy command until
  that is supported.
- Let the reason wrap under the host instead of truncating it.

## Later

### Jump hosts (`ProxyJump`)

Open the SSH connection to the jump host first and tunnel the target's SSH
through it (a `direct-tcpip` channel), including chains (`ProxyJump a,b`) and
`ProxyJump none`. Larger than `ProxyCommand`, since Newport has to authenticate
to each hop itself.
