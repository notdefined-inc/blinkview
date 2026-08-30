# ADR-0012: The command layer is a grammar, not a model

Date: 2026-08-28
Status: Accepted

## Context

The Ask panel answers questions about the library. The next step is letting it *act*:
"move all my august photos to Trip", "rate these five stars".

The obvious implementation is a small local LLM translating a sentence into a call. It
is also the wrong one here, for a reason specific to what this app already has.

An LLM photo agent does two jobs: work out **which photographs**, and work out **what to
do with them**. blinkview already does the first, deterministically, in `parseQuery` —
dates in any combination, people, ratings, labels, types, and since ADR-0008 what a
photograph shows. That resolution is *exact* where a model would be approximate: asked
for August 2026, a parser is right and a model is probably right.

The second job is a vocabulary of about eight verbs. Eight is small enough to enumerate,
and enumerating beats generating when the output drives file moves.

Weighed against a model:

- **Footprint.** The smallest useful instruction model is ~300 MB and seconds per turn,
  against a parser that is kilobytes and instant. ADR-0008 already spent 121 MB on the
  fp32 text tower to keep search correct; spending 300 MB more to reword sentences is a
  poor trade.
- **Determinism.** The same sentence must produce the same plan every time. A model
  offers no such guarantee, and "move" is not an operation to be probabilistic about.
- **Failure shape.** A parser fails *loudly* — it does not know a word. A model fails
  *plausibly*, inventing a destination folder that sounds right. For an operation that
  moves files, loud beats plausible.
- **Slot filling.** Asked to "move my august photos" with no destination, a parser knows
  precisely which slot is empty and can ask. A model tends to guess.

## Decision

A **grammar**, layered on the existing query parser:

```
utterance := clause (("and" | "then") clause)*
clause    := verb selector? target?
verb      := move | rate | label | delete | show | save
selector  := <parseQuery> | referent
target    := ("to" | "into" | "in") name
```

Three rules make it usable rather than merely correct:

1. **Every clause produces a preview, never an action.** Mutations go through `Plan` →
   `Journal`, so an agentic command is previewable and undoable like everything else.
   This is what makes acting on a sentence safe without a model's judgement.
2. **Unknown words fall through to semantic search**, not to failure. "move the beach
   photos to Trips" works because "the beach" is already a thing the app can resolve.
3. **A missing slot is a question, not a guess.** No destination means asking for one.
   An empty selector is refused outright — "move everything" is never what was meant.

## Consequences

Good: instant, deterministic, no download, and testable — a grammar has cases, and each
one is a unit test. It composes with everything `parseQuery` already understands, so
every filter added to search becomes addressable by command for free.

Costly: **novel phrasings fail.** "Chuck the blurry ones" is not in the vocabulary and
will not be guessed at. This is mitigated by making the grammar visible rather than
hidden — suggestions in the panel, and errors that say what *was* understood instead of
a flat refusal — but it is a real ceiling, and the honest reason to revisit this decision
later is phrasing coverage, not capability.

Verb synonyms are a table, not a model: "bin", "trash" and "delete" map to one verb.
Extending the table is cheap and is the first response to a phrasing someone expected to
work.

This decision is reversible in the direction that matters. Because commands compile to
`Plan`s, a model could later replace the *parser* while everything downstream — preview,
apply, journal, undo — stays exactly as it is.
