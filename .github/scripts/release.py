"""Plan an automatic release after a merge to main (see auto-release.yml).

    release.py plan VERSION CHANGES   -> prints the next version, or nothing
    release.py changelog VERSION DATE CHANGES CHANGELOG.md

CHANGES has one merged change per line: "<pull request or commit title>\t<#number or sha>".
Titles follow Conventional Commits; the bump is the largest one among them:

    feat                                 -> minor (0.3.0 -> 0.4.0)
    fix, perf, refactor, revert, build,
    chore(deps), deps                    -> patch (0.3.0 -> 0.3.1)
    a "!" after the type (feat!: ...)    -> major, or minor before 1.0
    docs, ci, test, style, other chore   -> no release on their own
"""

import re
import sys

TITLE = re.compile(r"^(?P<type>[a-z]+)(?:\((?P<scope>[^)]*)\))?(?P<bang>!)?:\s*(?P<desc>.+)$", re.I)
NONE, PATCH, MINOR, MAJOR = 0, 1, 2, 3


def bump_for(title):
    m = TITLE.match(title.strip())
    if not m:
        # not a Conventional Commit title: release it as a fix rather than drop it
        return PATCH
    kind, scope = m["type"].lower(), (m["scope"] or "").lower()
    if m["bang"]:
        return MAJOR
    if kind == "feat":
        return MINOR
    if kind in ("fix", "perf", "refactor", "revert", "build", "deps"):
        return PATCH
    if kind == "chore" and scope == "deps":
        return PATCH
    return NONE


def next_version(version, bump):
    major, minor, patch = (int(x) for x in version.split("."))
    if bump == MAJOR and major == 0:
        bump = MINOR  # before 1.0 a minor release may break things (CONTRIBUTING.md)
    if bump == MAJOR:
        return f"{major + 1}.0.0"
    if bump == MINOR:
        return f"{major}.{minor + 1}.0"
    if bump == PATCH:
        return f"{major}.{minor}.{patch + 1}"
    return None


def read_changes(path):
    out = []
    for line in open(path, encoding="utf-8"):
        line = line.rstrip("\n")
        if line.strip():
            title, _, ref = line.partition("\t")
            out.append((title.strip(), ref.strip()))
    return out


def plan(version, changes):
    return next_version(version, max((bump_for(t) for t, _ in changes), default=NONE))


def describe(title):
    """`fix(explorer): lay out only the rows on screen` -> `Lay out only the rows on screen`."""
    m = TITLE.match(title.strip())
    desc = m["desc"] if m else title.strip()
    return desc[:1].upper() + desc[1:]


def roll_changelog(text, version, date, changes):
    """Move the Unreleased entries under `## [version] - date`. When the
    changes didn't add any (e.g. dependency updates), list them instead."""
    head = "## [Unreleased]"
    start = text.index(head) + len(head)
    nxt = text.find("\n## [", start)
    end = len(text) if nxt == -1 else nxt
    body = text[start:end].strip("\n")
    if not body.strip():
        lines = []
        for title, ref in changes:
            if bump_for(title) == NONE:
                continue
            lines.append(f"- {describe(title)}" + (f" ({ref})" if ref.startswith("#") else ""))
        section = "Added" if any(bump_for(t) >= MINOR for t, _ in changes) else "Changed"
        body = f"### {section}\n\n" + "\n".join(lines)
    return text[:start] + f"\n\n## [{version}] - {date}\n\n{body}\n" + ("\n" + text[end:].lstrip("\n") if nxt != -1 else "")


def main(argv):
    if argv[1] == "plan":
        v = plan(argv[2], read_changes(argv[3]))
        if v:
            print(v)
    elif argv[1] == "changelog":
        version, date, changes, path = argv[2], argv[3], read_changes(argv[4]), argv[5]
        text = open(path, encoding="utf-8").read()
        open(path, "w", encoding="utf-8").write(roll_changelog(text, version, date, changes))
    else:
        sys.exit(f"unknown command {argv[1]}")


if __name__ == "__main__":
    main(sys.argv)
