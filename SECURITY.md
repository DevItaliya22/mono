# Security policy

## Reporting a vulnerability

**Email security@koru.do.** Please do not open a public issue, discussion or
pull request for a vulnerability, and do not include exploit details in one.

A useful report says:

- what is affected: the app and its version (the `VERSION` file beside the
  executables names it), the platform, or the commit if you built from source;
- what an attacker can do, and what they need first (local access, a paired
  device, a malicious document, a network position, …);
- how to reproduce it, as small as you can make it.

You can also use GitHub's private vulnerability reporting on this repository
("Security" tab → "Report a vulnerability"), which reaches the same people.

## What happens next

- **Within 3 working days** we confirm we have the report and who is handling it.
- **Within 10 working days** we tell you whether we can reproduce it and how we
  assess its severity, or what we still need from you.
- We keep you informed until it is fixed, and agree a disclosure date with you.
  Our default is to publish once a fixed release is available, and no later
  than 90 days after the report unless we agree otherwise.
- We credit you in the release notes and the advisory, under the name you
  choose, unless you prefer not to be named.

Koru is maintained by a small team, so these are commitments about response,
not about how long every fix takes. If you have heard nothing after 3 working
days, please write again: mail does get lost.

## Supported versions

Fixes land on `main` and ship in the next release of every channel.

| What | Supported |
| --- | --- |
| The latest **stable** release on each platform | Yes |
| The latest **beta** and **nightly** builds | Yes; they are where fixes appear first |
| Older releases | No. The built-in updater moves every installation to the latest release of its channel |
| A build you made yourself from source | Yes, for the code: report against the commit you built |

The platforms we maintain are listed on the
[governance page](GOVERNANCE.md#platforms).

## Scope

**In scope:**

- everything in this repository: the desktop and mobile apps, every crate they
  link, the wire contracts, the release tool and the build scripts;
- the official builds we publish, including their update channel and how an
  update is verified;
- **the hosted Koru Cloud service** (`*.koru.do`: sign-in, accounts, Cloud
  sync, the public API). Its source code is private, but you report it here,
  in the same way and with the same commitments;
- third-party code we ship (dependencies, and Koru's patches in `third-party/`),
  where the problem is in how Koru uses it. A bug in the library itself is
  best reported upstream too; tell us if you have done so.

**Out of scope:**

- reports from automated scanners with no demonstrated impact;
- denial of service by sheer volume against the hosted service, and social
  engineering of Koru staff;
- issues that need a device that is already compromised, or a person who
  already has the user's unlocked session, unless Koru makes things worse than
  the platform does;
- missing hardening with no attack (for example a header that is absent but
  protects nothing in context).

**Testing the hosted service:** use only accounts you own, do not access or
change other people's data, and stop and report as soon as you see something
you should not. Good-faith research that follows these rules is welcome, and
we will not pursue legal action over it.

## Security ownership

The maintainers named on the [governance page](GOVERNANCE.md#maintainers)
receive security@koru.do and own the response, including for the hosted
service.
