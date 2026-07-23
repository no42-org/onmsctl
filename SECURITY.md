# Security policy

## Reporting a vulnerability

**Do not open a public issue for a security vulnerability.**

Report privately through GitHub's private vulnerability reporting:

👉 **[Report a vulnerability](https://github.com/no42-org/onmsctl/security/advisories/new)**

(Repository → *Security* → *Advisories* → *Report a vulnerability*.)

If you cannot use GitHub, email **ronny@no42.org** instead.

Please include:

- the `onmsctl version` output and the platform you ran it on,
- what an attacker gains, and what access they need to start,
- a minimal reproduction — a redacted config file or manifest is fine,
- any suggested fix, if you have one.

### What to expect

| Stage | Target |
|---|---|
| Acknowledgement of your report | 5 working days |
| Initial assessment (accepted / not-a-vuln / need more info) | 10 working days |
| Fix released for an accepted report | depends on severity; you'll get a timeline in the assessment |

This is a small project maintained in spare time — those are honest
targets, not a commercial SLA. If a report goes quiet past these
windows, a nudge on the advisory thread is welcome.

Fixes ship as a normal release. Reporters are credited in the advisory
and the release notes unless you ask otherwise.

## Supported versions

`onmsctl` is pre-1.0 (`v0.x.y`). Only the **most recent release** gets
security fixes; there are no maintained patch branches for older minor
versions. Upgrade to the latest release before reporting.

## Scope

In scope — anything in this repository, notably:

- disclosure of credentials or tokens from the config file, the OS
  keyring integration, logs, or error output;
- TLS verification weaknesses, or auth material sent to the wrong host;
- a crafted YAML/XML input (`apply -f`, the eventconf and `provision.pl`
  migrators, `convert`) causing memory unsafety, or reading or writing
  files outside the intended paths;
- the release supply chain — the workflows under `.github/workflows/`,
  the published binaries, or the container image.

Out of scope:

- **Vulnerabilities in OpenNMS Horizon or Meridian itself.** `onmsctl`
  is an independent client and is not affiliated with The OpenNMS Group.
  Report those to [the OpenNMS project](https://github.com/OpenNMS/opennms/security/policy).
- A server accepting a configuration that `onmsctl` faithfully sent on
  the user's instruction. `onmsctl` is an API client; it applies what
  the operator asked for.
- Findings that require an attacker to already control the machine
  running `onmsctl`, or to already hold the credentials it uses.
- Advisories against a dependency with no demonstrated reachable path
  in this codebase. `make verify` runs `cargo deny check advisories` on
  every pull request and weekly, so plain RUSTSEC IDs are usually
  already visible to us — but if you can show it is *exploitable
  through `onmsctl`*, that is in scope and worth reporting.

## Verifying what you run

Every release artifact and container image is signed with Sigstore
cosign (keyless, GitHub OIDC — no long-lived keys), and releases carry a
CycloneDX SBOM. Verification recipes are in
[`README.md`](README.md#install) and [`RELEASING.md`](RELEASING.md#verifying-a-published-release).
