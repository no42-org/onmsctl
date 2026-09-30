---
title: TLS
description: Disable TLS certificate verification for lab or self-signed environments.
---

## Verify server certificates

onmsctl verifies server certificates against its bundled Mozilla root certificates (`webpki-roots`).
It does not read the OS trust store and has no option for a custom CA.
A server signed by a private CA fails verification even when the OS trusts that CA.
An untrusted certificate fails with `error: TLS handshake failed` and exit **`7`**.

## Disable certificate verification

| Name | Type | Default | Description |
|---|---|---|---|
| `--insecure-tls` | flag | off | Disable verification for this invocation. It has no environment variable. It **cannot turn verification back on** for a context that disables it. |
| `server.insecure-skip-tls-verify` | boolean, context field | `false` | Disable verification for every request made with this context. |

Set the field in a context:

```yaml
contexts:
  - name: lab
    server:
      url: https://horizon.lab.example/opennms
      insecure-skip-tls-verify: true
```

With verification disabled, onmsctl prints this warning to stderr on **every request**:

```text
warning: TLS certificate verification is disabled (insecure-skip-tls-verify) for GET request. Use only on trusted networks.
```

The HTTP method in the message changes with the request.
The request path is left out on purpose, so it cannot leak into logs.

:::warning Keep it off in production

Disabled verification lets anyone on the network path impersonate the server and read your credentials.
Use it only on trusted lab networks.

:::
