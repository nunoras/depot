# Model-matched rules resolve in code

Depot resolves a role to a profile from the project's `[profiles]` map, but the role arrived as an argument to `depot task add`, so deciding which role a task needed was a coordinator turn.
The decision is judgement, and this ADR records where it moved: into the daemon, driven by a rule layer in `.depot.toml` and one question to Typesafe.

## The model chooses a rule, never a profile

Each rule declares the condition it matches, the role a matching task gets, and an optional ordered list of candidate profiles, and all three are written by a person.
The model is asked one Choice question whose options are the rules' `when` strings plus one fixed neutral option for work that matches no rule, and whose state is the project name and the task's own text.
Nothing else reaches it: not the roles, not the candidate lists, not the `[profiles]` map, not the configured fallback chain.
The adapter asks the question and turns the answer into a choice, a confidence and a model version; it holds no rule about what that answer means.

The line is deliberate.
Choosing a profile is choosing what the work costs, and a model that can name its own replacement can route around a roster.
The candidate list is a preference a person set; the model only says which condition the work satisfies.
Everything after the answer is code: the confidence floor, the rule's role and the candidate order are applied by `depot-core` and `depotd`, so a rule that names no candidates still resolves through the existing role-to-profile map.
Only the first candidate in configured order is read: it is the profile the task is pinned to.
The ordered tail is carried for the ticket that ranks candidates by quota, which is the ticket that will choose among them, so a rule's candidate list is not a fallback chain and is deliberately absent from the retry path.
Failover stays where it was: the retry path still takes its profile from the machine-local `fallback_profiles`, untouched by anything a rule says.
Every candidate is still checked against machine-local settings when the task is filed, so a candidate that names no profile refuses creation by name rather than failing at launch.

## The decision is a judgement, not an observation

A fact in depot is normally something the daemon observed, and `docs/adr/0001-local-daemon-and-deterministic-facts.md` says a model's prose never moves a task.
A dispatch decision is the case that ADR anticipated: judgement that genuinely needs a model, recorded as a fact so the state machine stays deterministic.
Its payload carries `source = "model_judgement"` and its kind is `task_dispatch_judged`, so the journal tells it apart from an observation at a glance.

It carries the chosen rule, the confidence, the model id and version, and a hash and snapshot of the rule set that was in force, so a surprising dispatch is explainable from the journal alone.
It is written in the same transaction as, and immediately before, the task's proposal fact, so the role and the pinned profile the task records are the ones the decision produced.

Because it is a judgement rather than an observation, nothing reconciles it away.
`docs/adr/0003-reconciliation-derives-pending-work.md` derives pending work from the records, and a judgement leaves no pending work: reducing it changes no state and intends no action.
A match also happens once, at creation, and the chosen profile is pinned on the task, so a running task never has its model swapped underneath it when the rules change.

## Failure is named, and there is no fallback

No key, no rules, a malformed rules table, no matching rule, a confidence below the floor and an API error each refuse task creation with the concrete reason, and an explicit `--role` is always the override.
There is deliberately no fallback profile: a guessed model is a cost decision nobody made, which is the same reason `docs/adr/0002-adapters-behind-seams.md` gives for refusing an unmapped role.
A refusal creates no task and no proposal, so it is not journalled and the reason is the CLI's to print.

Without a key the layer is off, and dispatch behaves exactly as it did before: no rules are read and no request is made.
The key lives in an owner-only `typesafe-key` file in the depot home under the same permission rules ADR 0002 sets for the GitHub token, and never in the project, because a project is a thing that gets pushed.
The Typesafe base URL is machine-local configuration, so the tests point the client at a local endpoint the way the forge client already does.
