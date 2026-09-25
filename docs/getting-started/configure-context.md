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

`auth.basic` / `auth.bearer` take exactly one source: inline (`password`/`token`), a file (`password-file`/`token-file`), or the OS `keyring`.

| Field | Notes |
|---|---|
| `password` / `token` | Inline plain-text. Convenient; leaks if the config leaks. |
| `password-file` / `token-file` | Path to a file; mode `0600` recommended, trailing newline stripped. |
| `keyring` | OS keyring (macOS Keychain / Windows Credential Manager work out of the box; Linux GNOME Keyring/KWallet needs a rebuild with `--features keyring/sync-secret-service`). |

```yaml
contexts:
  - name: prod
    server:
      url: https://horizon.example.com/opennms
    auth:
      basic:
        username: automation
        password-file: ~/.secrets/onms-prod   # pointer, safe to commit the config
```

Resolution at request time: `env ($ONMS_PASSWORD / $ONMS_TOKEN) > keyring > file > inline`.

## Verify connectivity

```sh
onmsctl iam whoami                   # confirms URL + credentials work
```

## Override precedence

`flags (--url, --user, --context) > env (ONMS_URL, ONMS_USER, ONMSCTL_CONTEXT) > active context > built-in default`.
