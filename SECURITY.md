# Security Policy

## Supported versions

viewr is pre-1.0. Security fixes are developed for the current default branch and
the latest published release. Older releases are not supported after a newer
release is available.

| Version | Supported |
| --- | --- |
| Current default branch | Yes |
| Latest published release, when available | Yes |
| Older releases | No |

Reports against older versions are still useful when the behavior also affects a
supported version.

## Report a vulnerability safely

The repository URL declared in `Cargo.toml` is not currently published, and no
verified private reporting channel is operational. Private reporting must be
enabled and verified before a public release. Until then, do not send vulnerability
details to a guessed address or publish them in an issue, discussion, pull request,
or commit.

If you received viewr from a maintainer or distributor, use that same trusted
channel only to request a private security contact. If the repository becomes
public before private reporting is available, a public issue may contain only that
contact request. Do not include reproduction steps, impact details, private
filenames, file paths, images, metadata, logs, or exploit code in the request.

After a maintainer confirms a private channel, include the following when
available:

- the affected viewr version or commit;
- the operating system, package type, and relevant configuration;
- prerequisites and the security boundary that is crossed;
- minimal reproduction steps using synthetic, non-sensitive files;
- the observed and expected results, including any file-state change;
- the potential impact and any known mitigation.

A maintainer who accepts the report through a confirmed private channel will
validate it, coordinate a fix when needed, and agree on disclosure before
publishing technical details. This project does not promise a response or
remediation deadline.

## Scope

Security reports are welcome for:

- image parsing, resource limits, and the isolated decode worker;
- file selection, system Trash, restore, permanent deletion, and race safety;
- metadata handling, export privacy, logs, diagnostics, and local preferences;
- sandbox, platform packaging, file associations, and native integration;
- dependencies, build provenance, release artifacts, and CI policy.

The documented immediate identity check before a pathname-based Trash, restore, or
permanent-delete operation is not claimed to be atomic. A report is still valuable
if it demonstrates a stronger impact, a wider attacker model, or a practical way
to preserve native Trash and exact Undo semantics while closing that boundary. See
the curation safety contract in [README.md](README.md) and the security boundaries
in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

Use only systems and files you own or have explicit permission to test. Prefer
disposable synthetic fixtures, keep recoverable backups, and stop before testing
would expose another person's data or disrupt a service.
