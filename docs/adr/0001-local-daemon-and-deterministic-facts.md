# Local daemon and deterministic facts

depot is a local daemon, not a hosted service, and every task state transition is driven by a deterministic fact rather than by anything a model wrote.
A fact is something the daemon observed - a worker turn, a submit, a validation result, a push, forge state, a coordinator decision - and it carries the time it was observed.
A model's prose is an annotation on a task; it never moves that task.

The alternative we rejected was letting a session narrate its own progress and having depot believe it.
That fails in the way multi-week agent work usually fails: stale self-reporting is indistinguishable from progress, and the person running the effort becomes the state machine reconciling it.
Deciding this now is what fixes the shape of the whole project, because it is the reason the lifecycle can be a pure reduce step, the reason it is testable without processes, filesystem, network or a clock, and the reason a restart can reconcile from records instead of guessing.

The consequence is a hard split.
Anything the daemon can observe deterministically is depot's to own, and no model gets a vote on it.
Anything that genuinely needs judgement arrives as a fact, or as a coordinator decision recorded as one.
