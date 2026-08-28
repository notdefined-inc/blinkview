# The command layer: acting on a sentence

Status: active · 2026-08-28
Decision: ADR-0012 (the command layer is a grammar, not a model)

## Intent

Let the Ask panel act, not only answer. "move all my august photos to Trip/Greece Day3"
should select the right photographs and show what it is about to do, with one button to
do it and ⌘Z to undo.

The measure of success: **someone who has never read documentation can reorganise a
library by describing what they want**, and is never surprised by what happens, because
nothing happens until they see it listed.

## Scope

Core: a `plan_move` primitive building a `Plan` from an arbitrary set of hashes.
App: the command grammar, a preview card, slot-filling follow-ups, and referents across
turns.

Out of scope: standing rules that run on import, and saved recipes. Both reuse this
grammar and are worth their own spec once this is real.

## Grammar

```
utterance := clause (("and" | "then") clause)*
clause    := verb selector? target?
verb      := move | rate | label | delete | show | save
selector  := <parseQuery> | referent
referent  := them | those | these | it | the results | there
target    := ("to" | "into" | "in") name
```

Verbs are matched through a synonym table (`bin`/`trash` → delete, `put`/`file` → move,
`star` → rate), because the first response to a phrasing someone expected to work is a
table entry, not a model.

## Acceptance criteria

1. "move my august photos to Trip" resolves the selector with `parseQuery`, the
   destination as a folder, and shows a preview naming both — without moving anything.
2. Applying the preview goes through `Plan` → `Journal`; ⌘Z restores every file *and*
   its metadata, per ADR-0010.
3. "move the beach photos to Trips" resolves "the beach" semantically, because no folder,
   person or date matches it.
4. A missing destination asks for one rather than guessing; answering the question
   completes the original command.
5. An empty selector is refused. "move everything to X" with no filter must not silently
   plan the whole library.
6. An unknown verb reports what *was* understood, and names the nearest verb it knows.
7. "create Greece and move the august photos there" runs as two clauses; `there` binds
   to the folder named by the previous clause.
8. "rate them 5 stars" after a question applies to that answer's photographs.
9. "delete" always previews, never acts immediately, whatever the phrasing.
10. Every command is expressible in the omnibar's existing syntax too — the grammar adds
    verbs, it does not fork the query language.

## Tasks

- [x] 1. `plan_move(hashes, dest)` core primitive + Tauri command — criteria 1, 2
- [x] 2. Verb grammar and synonym table, over `parseQuery` (app) — 1, 3, 6, 10
- [x] 3. Preview card with counts, destination and skipped items (app) — 1, 5, 9
- [x] 4. Apply, with journal id surfaced for undo (app) — 2
- [x] 5. Slot filling: missing destination becomes a question (app) — 4
- [x] 6. Referents and multi-clause utterances (app) — 7, 8
- [x] 7. Grammar unit tests (app) — every criterion
- [x] 8. Doc sync: STATUS.md, the Ask panel section

## Risks

**A wrong selector moves the wrong files.** Mitigated by the preview being mandatory —
there is no path from sentence to disk that does not pass through a listed plan — and by
`Plan::validate` refusing to apply anything unsafe.

**Phrasing coverage disappoints.** The ceiling ADR-0012 accepts. Mitigated by visible
suggestions and errors that name what was understood; measured by how often the fallback
message appears.

## Outcome

Shipped. All ten acceptance criteria met, verified in the running app against a nested
40-photo library: a sentence selected ten photographs, previewed the move, applied it,
wrote a journal entry, and undid it back to the exact starting state.

The work found three defects that had nothing to do with the grammar, one of them
serious:

- **`Plan::apply` recorded changes after making them.** A plan label containing `/`
  produced an invalid journal filename; the write failed *after* twenty-three
  photographs had moved, leaving them unreachable by undo while the UI said the
  operation had failed. Files, journal, metadata — with rollback on any failure — is now
  the order, and the journal id is sanitised where it becomes a path.
- **`answerQuestion` offered to index the library whenever leftover words existed**, even
  when those words matched a folder name, so "the swiss photos" could not find
  `Swiss Day1`.
- **An empty selector inherited the previous result**, so "move to Trip" planned ten
  photographs without being asked to.

Two grammar lessons worth keeping: a shared-prefix score cannot see a transposition, and
"mvoe" shares exactly one letter with "move"; and a verb's value has to be lifted out of
the selector during parsing, or "rate them 5 stars" is searched for as a phrase.

## Not done here

Standing rules that run on import, and saved recipes. Both reuse this grammar unchanged
and are worth their own spec.
