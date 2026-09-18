# The coordinator contract is versioned artifacts, and its inbox joins facts to records

The coordinator is the one session the user talks to, so what it is told and what it reads are product decisions rather than prompt improvisation.
This note records three of them together, because they only work as a set.

**The coordinator's texts are files in the repository.**
The policy prompt, the brief template and the session kickoff live under `assets/` and are included in the binary, so they are reviewed, versioned and diffed like code, and so a session gets the same words on Linux, Windows and macOS.
None of them names a harness or a model.
The profile those words run under is project configuration, and a test asserts that a shipped artifact never names one.

**The inbox is the fact journal joined to the records.**
The coordinator starts every turn with `depot inbox`, which prints the facts recorded since its previous turn.
The journal alone says what changed and when, and the records say where each task stands now, so an entry is built from both.
A fact on its own is not complete enough to act on, and current state on its own is not what happened since the last turn.
Classifying an entry by the state a task is in now rather than at the moment of the fact is deliberate: a question that has since been answered stops asking for anyone.
One line is the exception: a worker that ends its turn to ask a question is not a dead worker, so a task with an open question stays in flight when boxr reports the session gone, and the record and the fact disagree about the same moment.
That liveness line is rendered from the `liveness` value the fact carries, so a coordinator cannot read a gone worker as live; the entry is still classified by the current state like every other one, and `crates/depot/tests/golden_path.rs` pins the line and the in-flight task that goes with it.
A poll that observed nothing is not reported at all, because the change it observed is its own fact.

**Rotation is a rule, not a judgement.**
A session that grows past the configured context size is rotated by a rule in the pure core.
The size arrives as a fact, the limit is machine-local configuration, and the session the daemon must stop comes from the state.
A model's impression of how long the conversation feels is not an input.

The rejected alternative everywhere here is the coordinator owning its own narrative.
Summarising its own turn, deciding for itself how much context it has left, or writing the task state down in prose all reintroduce the failure depot exists to prevent: a model's description of the work mistaken for the work.
The consequence is that every coordinator-facing text is an artifact in this repository and every coordinator-facing fact is a record, so a surprising session can be explained after the fact without reading a transcript.
