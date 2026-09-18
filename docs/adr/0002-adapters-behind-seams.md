# Adapters behind seams

Depot talks to four things it does not own: boxr for sessions, treehouse for worktrees, GitHub for the forge, and project configuration for role profiles.
Each gets one thin adapter: a trait with the operations depot performs, an implementation that speaks the real protocol at the edge, and a fake in the tests that reproduces that protocol.
`docs/adr/0004-model-matched-rules-resolve-in-code.md` added a fifth on the same terms: Typesafe, the dispatch matcher.

The rule that shapes all of them is that an adapter holds no policy.
Whether a task may launch, when a lease returns to the pool, which profile a role falls back to and what a failed validation means are lifecycle rules, and they live in `depot-core` where they are table-tested without processes, filesystem or network.
An adapter turns a decision already made into a command, and turns the answer back into something deterministic.
When an adapter starts deciding, the same rule exists in two places and the pure core stops being the whole story of a task.

**Sessions.** Depot never launches a harness itself, and it never reads a harness transcript.
boxr launches the headless session and is the ledger, so depot reads boxr's own commands and its TOON output.
The surface depot requires is recorded in `docs/boxr-contract.md`, together with the minimum version, because detached sessions and resume do not exist yet.
The capability probe is the gate: a boxr that cannot do what the contract needs is refused by name, and a missing capability is never a silent fallback to something slower or vaguer.

**Worktrees.** Depot never creates a worktree and never resets one.
It leases from treehouse's pool and returns the lease, and it releases only when the lease's directory holds no uncommitted and no unpushed work.
A refusal there is deliberate: the cheap way to return a lease is `--force`, which cleans and resets, and a reset is how a day of agent work disappears.
So the adapter inspects the directory with git, refuses with the reason, and never passes a force flag.

**Forge.** Depot polls the GitHub API rather than shelling out to `gh`, because the decision ticket settled on the API and because the end-to-end suite needs a fake endpoint rather than a fake CLI.
The client's base URL is a parameter, so the tests point it at a local endpoint and GitHub Enterprise stays possible without touching the code.
The merge call names the validated head, so the forge refuses a branch that moved after validation; whether to merge at all is a lifecycle rule, not the adapter's, and the adapter holds no policy there either.
Credentials come from an authenticated `gh auth token` when that exists, otherwise from an owner-only `github-token` file in the depot home, and never from the project, because a project is a thing that gets pushed.
On Unix the file's mode must exclude group and other.
On Windows its DACL may grant only the current user, the owner, Administrators and SYSTEM.
Any other platform is refused, because the ACL cannot be proven.

**Profiles.** A role resolves to a profile from project configuration, with the configured fallback list behind it.
The adapter does not read a file and does not hold the default: it resolves a role against the map it is given and refuses an unmapped role, naming the roles that are configured.
A guessed model is a cost decision nobody made, which is exactly what a roster exists to prevent.

The alternative we rejected was testing against in-memory fakes of the traits alone.
That tests the code against our beliefs about the dependency, and it is the belief that is usually wrong: field names, exit codes and the difference between an empty list and an error.
So each adapter is driven through its real edge, a real child process or a real HTTP endpoint, and the traits stay thin enough that the seam is the protocol rather than an abstraction we invented.
