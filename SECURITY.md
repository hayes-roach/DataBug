# Security policy

DataBug holds the keys to people's databases, so security reports get priority.

## Reporting a vulnerability

**Please don't report security problems in public issues, discussions or pull requests.**

Report them privately through GitHub: on the repository, open **Security → Report a vulnerability** ([direct link](https://github.com/hayes-roach/DataBug/security/advisories/new)). Only maintainers can see the report.

Include as much of this as you can:

- What the problem is and what an attacker could do with it
- Steps to reproduce, or a proof of concept
- The DataBug version (`databug --version`), your OS and the database engine involved
- Whether the problem is already public anywhere

Never include real passwords, connection strings or data from production databases in a report. Use made-up values.

## What happens next

- You'll get an acknowledgement within **3 working days**.
- The maintainer will confirm the problem, judge how serious it is, and keep you updated at least weekly.
- Fixes are released as a patch version with a GitHub security advisory. You'll be credited unless you'd rather not be.
- Please give us **90 days** (or until a fix ships, if sooner) before disclosing publicly. If the problem is being actively exploited, we'll move faster and may publish a workaround before a fix.

## Supported versions

Security fixes go into the latest release. Before 1.0, older versions aren't patched, so update to the latest release to get fixes.

| Version | Supported |
|---|---|
| Latest 0.x release | ✔ |
| Older releases | ✘ |

## Scope

In scope, for example:

- Passwords or other secrets leaking out of the OS keychain: into `connections.json`, `settings.json`, history, catalog caches, logs or crash output
- Bypassing the **Read-only** or **Production** protections, or the confirmation for dangerous statements
- SQL or command injection through generated SQL (DDL, renames, CSV import, data search, procedure templates) or through Kerberos `kinit`
- TLS settings not being honoured (for example, a connection set to *Require* falling back to plain text)
- The install scripts (`install.sh`, `install.ps1`) or installers doing anything beyond installing DataBug, or being open to tampering

Out of scope:

- Problems that need an attacker who already controls your user account or machine
- Vulnerabilities in a database server itself (report those to its vendor)
- TLS without certificate verification when you chose *Prefer* or *Trust server certificate*; that's documented behaviour
- The apps not being signed with paid Apple or Microsoft certificates (known; see the README)
