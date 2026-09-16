# {{title}}

Task `{{task}}` of project `{{project}}`, role `{{role}}`.

## Intent

{{intent}}

## Output destination

{{output}}

## Done criteria

{{done}}

## Dependencies

{{dependencies}}

## Validation

{{validation}}

## Worker context

Before using either worker call, set these environment variables in your shell:

    {{worker_context}}

## The store

Everything depot owns lives outside the project repository.
The store for this project is `{{store}}`.
The checklist is `{{checklist}}`, rendered from depot's records: read it, never edit it.
Put evidence, notes and artifacts in `{{scratch}}`.

## The two calls

Ask a question when something is not already settled:

    {{ask}}

That records the question on this task, ends your turn, and leaves the session addressable.
The coordinator or the user answers it and the session resumes with the answer, so ask instead of guessing.

Declare the assignment finished when it is:

    {{submit}}

That triggers validation against the commit you submit.
The daemon runs the project's validation command, pushes the branch and opens the pull request after a pass, so do not run forge commands or push yourself.

Everything else you write is commentary.
It is attached to the task and it never moves its state.
