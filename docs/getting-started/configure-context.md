---
title: Configure a context
description: Set up onmsctl's config file, named contexts, and credential resolution.
---

`onmsctl` follows the kubectl pattern: one config file, one or more named contexts, one active context.

| OS | Default path |
|---|---|
| Linux | `$XDG_CONFIG_HOME/onmsctl/config.yaml` (typically `~/.config/onmsctl/config.yaml`) |
| macOS | `~/Library/Application Support/org.no42-org.onmsctl/config.yaml` |
| Windows | `%APPDATA%\no42-org\onmsctl\config\config.yaml` |

Override the path with `--config <path>` or `$ONMSCTL_CONFIG`.

A minimal config with a single context:

```yaml
current-context: dev
contexts:
  - name: dev
    server:
      url: https://horizon.dev.lab/opennms
    auth:
      basic:
        username: admin
        password: admin      # don't commit inline secrets — see Credentials
```

## Inspect and switch contexts

```sh
onmsctl config view                  # current config (inline secrets redacted)
onmsctl config use-context staging   # atomic switch of the active context
```

`config view` redacts inline `password` / `token` values.
File and keyring references stay visible because they are pointers, not secrets.

## Credentials

`auth.basic` / `auth.bearer` take at most one source: inline (`password`/`token`), a file (`password-file`/`token-file`), or the OS `keyring`.
Declare none to supply the secret only through `ONMS_PASSWORD` (basic) or `ONMS_TOKEN` (bearer).

| Field | Notes |
|---|---|
| `password` / `token` | Inline plain-text. Convenient; leaks if the config leaks. |
| `password-file` / `token-file` | Absolute path to a file. `~` is not expanded. Mode `0600` recommended. Trailing CR/LF is stripped; other whitespace is kept. |
| `keyring` | OS keyring entry, `{service, account}`, both non-empty. macOS Keychain, Windows Credential Manager and the Linux kernel keyring work out of the box. GNOME Keyring/KWallet need a rebuild with `--features keyring/sync-secret-service`. |

```yaml
contexts:
  - name: prod
    server:
      url: https://horizon.example.com/opennms
    auth:
      basic:
        username: automation
        password-file: /home/automation/.secrets/onms-prod   # pointer, safe to commit the config
  - name: lab
    server:
      url: https://horizon.lab.example.com/opennms
    auth:
      basic:
        username: admin
        keyring: { service: onmsctl, account: lab }
```

At request time a set `ONMS_PASSWORD` (basic) or `ONMS_TOKEN` (bearer) overrides the declared source.

## Verify connectivity

```sh
onmsctl iam whoami                   # confirms URL + credentials work
```

## Override context values

`--url`, `--user` and `--context` (or `ONMS_URL`, `ONMS_USER`, `ONMSCTL_CONTEXT`) override the active context for one command.
The full list and their precedence are under [Global flags](../reference/global-flags.md).
