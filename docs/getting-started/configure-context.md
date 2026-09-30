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

Override the path with **`--config <path>`** or **`$ONMSCTL_CONFIG`**.

## Create the config file

There is no `config init` command.
Write the file by hand:

1. Create the directory for the default path.

   ```sh
   mkdir -p ~/.config/onmsctl                                   # Linux
   mkdir -p ~/Library/Application\ Support/org.no42-org.onmsctl  # macOS
   ```

2. Write `config.yaml` in that directory with at least one context.

   ```yaml
   current-context: dev
   contexts:
     - name: dev
       server:
         url: https://horizon.dev.lab/opennms
       auth:
         basic:
           username: admin
           password: admin      # don't commit inline secrets, see Credentials
   ```

3. Restrict the file to your user.

   ```sh
   chmod 600 ~/.config/onmsctl/config.yaml                                   # Linux
   chmod 600 ~/Library/Application\ Support/org.no42-org.onmsctl/config.yaml  # macOS
   ```

An inline `password` or `token` works, but every command then prints a warning to stderr:

```text
warning: plaintext password/token in config — consider keyring or password-file
```

Use a file or keyring source to avoid it.
See [Credentials](#credentials).

## Look up the context settings

The table lists every key a context accepts.
Unknown keys are rejected when the config loads.

| Name | Type | Default | Description |
|---|---|---|---|
| **`current-context`** | string | none | Top-level key. Name of the active context. Must match a declared context. Without it, pass `--context` or `ONMSCTL_CONTEXT`. |
| **`name`** | string | required | Context name, used by `config use-context` and `--context`. |
| **`read-only`** | boolean | `false` | Refuse every write command locally, before any HTTP request. See [Read-only contexts](../concepts/read-only-contexts.md). |
| **`server.url`** | string | required | Horizon base URL, `http` or `https`, e.g. `https://horizon.example.com/opennms`. |
| **`server.insecure-skip-tls-verify`** | boolean | `false` | Skip TLS certificate verification. |
| **`auth.basic.username`** | string | required with `basic` | Horizon user name. |
| **`auth.basic.password`** | string | none | Inline password. Prints the plaintext warning. |
| **`auth.basic.password-file`** | path | none | Absolute path to a file holding the password. |
| **`auth.basic.keyring`** | `{service, account}` | none | OS keyring entry holding the password. |
| **`auth.bearer.token`** | string | none | Inline token. Prints the plaintext warning. |
| **`auth.bearer.token-file`** | path | none | Absolute path to a file holding the token. |
| **`auth.bearer.keyring`** | `{service, account}` | none | OS keyring entry holding the token. |
| **`iam.protected-roles`** | list of strings | `[ROLE_ADMIN]` | Roles `User` apply refuses to leave without a holder. See [User context settings](../kinds/user.mdx#context-settings). |
| **`iam.known-roles`** | list of strings | the 16 Horizon built-in roles | Roles that pass without an unknown-role warning. See [User context settings](../kinds/user.mdx#context-settings). |
| **`iam.allow-admin-lockout`** | boolean | `false` | Skip the admin-lockout refusal. See [User context settings](../kinds/user.mdx#context-settings). |

`auth` declares exactly one of `basic` or `bearer`.

## Inspect the config

**`config view`** prints the current config with inline secrets redacted:

```sh
onmsctl config view
```

Expected output:

```text
current-context: dev
contexts:
- name: dev
  server:
    url: https://horizon.dev.lab/opennms
    insecure-skip-tls-verify: false
  auth:
    basic:
      username: admin
      password: <redacted>
- name: staging
  server:
    url: https://horizon.staging.example.com/opennms
    insecure-skip-tls-verify: false
  auth:
    basic:
      username: automation
      password-file: /home/automation/.secrets/onms-staging
```

`config view` redacts inline `password` and `token` values.
File and keyring references stay visible because they are pointers, not secrets.

## Switch the active context

**`config use-context`** rewrites `current-context` in the config file atomically:

```sh
onmsctl config use-context staging
```

Expected output:

```text
switched to context 'staging'
```

## Choose a credential source {#credentials}

`auth.basic` and `auth.bearer` take at most one source: inline (`password`/`token`), a file (`password-file`/`token-file`), or the OS `keyring`.
Declare none to supply the secret only through **`ONMS_PASSWORD`** (basic) or **`ONMS_TOKEN`** (bearer).

| Field | Notes |
|---|---|
| `password` / `token` | Inline plain-text. Convenient, but leaks if the config leaks. |
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

**`iam whoami`** confirms the URL and credentials work.
It prints the authenticated user name:

```sh
onmsctl iam whoami
```

Expected output:

```text
admin
```

## Override context values

`--url`, `--user` and `--context` (or `ONMS_URL`, `ONMS_USER`, `ONMSCTL_CONTEXT`) override the active context for one command.
The full list and their precedence are under [Global flags](../reference/global-flags.md).
