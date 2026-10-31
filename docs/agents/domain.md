# Domain docs

## Read before domain exploration

This repo uses a single context:

- `GLOSSARY.md` at the repo root: domain terms.
- `docs/adr/`: architectural decision records. Read those relevant
  to the area being explored.

If either is absent, proceed silently. Create documentation lazily
through domain modeling when terms or decisions are resolved, rather
than scaffolding empty files.

The project's exploratory goals and proposals remain in
`docs/goals.md` and `docs/design-notes.md`; these are not accepted ADRs.

## Vocabulary

Use glossary terms in issue titles, proposals, hypotheses, and tests.
Respect explicitly avoided synonyms. If a needed concept is missing,
reconsider the term or note the gap for domain modeling.

## Decision conflicts

If a proposal contradicts an ADR, identify that conflict explicitly
and explain why the decision may be worth reopening.
