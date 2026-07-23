# Support

## Start with the docs

- [Quick Start](docs/quickstart.md) — install, configure a context, first `apply`.
- [README](README.md) — every capability, with worked examples.
- [EventSource reference](docs/eventsource-reference.md) — the `kind: EventSource` schema.
- [Migration guide](docs/migration.md) — bringing legacy eventconf and `provision.pl` files in.
- `onmsctl <command> --help` — every command is self-documenting.

## Asking a question

GitHub Discussions is not enabled, so **questions go to the
[issue tracker](https://github.com/no42-org/onmsctl/issues)**. Search
first, then open an issue and pick the *Question* template.

A question that includes the `onmsctl version` output, your Horizon or
Meridian version, the exact command, and the actual vs. expected output
gets a useful answer far faster than one that doesn't. Redact
credentials and hostnames.

## Reporting a bug or requesting a feature

Use the [issue tracker](https://github.com/no42-org/onmsctl/issues/new/choose)
and pick the matching template.

## Reporting a security vulnerability

**Not through the issue tracker.** See [SECURITY.md](SECURITY.md).

## What this project can't help with

`onmsctl` is an independent Apache-2.0 client for the OpenNMS REST API.
It is **not affiliated with or supported by The OpenNMS Group.**

For questions about OpenNMS Horizon or Meridian themselves — the server
not starting, a poller or collector misbehaving, database issues — go to
the [OpenNMS community](https://opennms.discourse.group/) instead. If
`onmsctl` is faithfully sending a request and the server rejects it or
behaves unexpectedly, that is usually a server-side question, though
we're happy to help you tell the two apart.

## Expectations

This is a spare-time project. Issues are read, but response times vary
and there is no commercial support offering. Well-scoped pull requests
are the fastest route to a fix — see [CONTRIBUTING.md](CONTRIBUTING.md).
