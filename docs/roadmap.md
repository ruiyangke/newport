---
title: "Roadmap"
description: "See planned connection improvements and learn how to request a feature."
slug: roadmap
---

# Roadmap

These are planned improvements, not features you can use today. Priorities can change, and there is no committed release date. See the [release notes](https://newport.app/releases) for features that have shipped.

## More ways to connect over SSH

### Proxy commands

Planned support for SSH `ProxyCommand` would let Newport connect through a helper program, including connections to some local virtual machines and servers reached through an access gateway.

Today, Newport requires a direct SSH connection. A host that relies on `ProxyCommand` cannot be imported and connected as-is. Use a directly reachable SSH address if your environment provides one.

### Clearer SSH import messages

The SSH config importer should explain why each host cannot be imported and keep the full reason visible. In particular, proxy commands and jump hosts need different explanations. The current message may describe both as a jump host.

### Jump hosts

Planned support for `ProxyJump` would let you reach a private server through another SSH host, including connections with more than one hop. Newport does not currently support this connection method.

## Request a feature

[Open a feature request](https://github.com/ruiyangke/newport/issues) with the task you want to accomplish, your operating system, and any workaround you currently use. For SSH-related requests, include a redacted example configuration with private hostnames and account details removed.

[Back to the user guide](README.md)
