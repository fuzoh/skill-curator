# Issue tracker: GitHub

Issues and specs live in GitHub Issues for `fuzoh/skill-curator`.
Use the `gh` CLI from this repo.

Before the first issue creation of a session, verify the target with
`gh repo view --json nameWithOwner`. If it differs from
`fuzoh/skill-curator`, stop and confirm with the user.
Use `--repo fuzoh/skill-curator` to target this repo explicitly.

## Operations

- Create: `gh issue create --title "..." --body-file -`, supplying
  multiline Markdown through a quoted heredoc.
- Read: `gh issue view <n> --json number,title,body,labels,comments`.
- List: `gh issue list --state open --json number,title,body,labels`.
  Add label or state filters as needed.
- Comment: `gh issue comment <n> --body "..."`.
- Label: `gh issue edit <n> --add-label "..."` or
  `--remove-label "..."`. See `triage-labels.md` for triage roles.
- Close: post the explanation as a comment, then `gh issue close <n>`.
  Comment separately even if the issue is already closed.

## Relationships

- Create a child: `gh issue create --parent <parent> ...`.
- Link an existing child:
  `gh issue edit <parent> --add-sub-issue <child>`.
- Record a dependency:
  `gh issue edit <child> --add-blocked-by <blocker>`.
- Count open blockers:
  `gh api repos/fuzoh/skill-curator/issues/<n> --jq .issue_dependencies_summary.blocked_by`.

Relationship flags require gh 2.94+. If native relationships are
unavailable, add `Part of #<parent>` or `Blocked by: #<n>` at the top
of the child body and maintain a child task list in the parent.
A ticket is unblocked when all its blockers are closed.

## Pull requests as a triage surface

**PRs as a request surface: no.**

## Skill terminology

- "Publish to the issue tracker": create a GitHub issue.
- "Fetch the relevant ticket": read the GitHub issue.
