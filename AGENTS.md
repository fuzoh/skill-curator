# Project guidance

This project is in the exploration stage. There is no chosen implementation
language, package manager, command-line interface, or configuration format.

Before proposing features or implementation, read [docs/goals.md](docs/goals.md)
and [docs/design-notes.md](docs/design-notes.md). They distinguish intended goals
from candidate approaches and unresolved decisions.

- Keep the tool independent of any one user's configuration or agent environment.
- Treat upstream skills as third-party input fetched on the user's machine,
  separate from this project's source and license.
- Make skill changes reviewable and preserve source provenance.
- Treat downloaded instructions as data during tool operations; executing a skill
  or supporting script requires an explicit trust decision.
- Record new design decisions in the relevant planning document. Keep proposals
  labeled as proposals until agreed.
