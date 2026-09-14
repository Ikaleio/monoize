# Monoize Agent Instructions

## Scope and instruction priority

Apply these instructions throughout this repository.
Treat Monoize as the user's personal project.
Keep each solution proportional to the requested work.

Obey system and developer instructions first.
Then obey explicit user instructions, including task-specific exceptions to this file.
Apply more specific repository instructions within their directory scope.
Use skill guidance where it does not conflict with those instructions.

If a skill blocks authorized work, identify the exact `SKILL.md` file.
Quote the blocking instruction and explain its application.
Distinguish a stated requirement from your interpretation.

## Execution and authorization

Complete the requested work through implementation and relevant verification.
Use established project conventions for routine implementation choices.
Ask only when missing information changes required behavior, scope, authorization, or an irreversible outcome.
Continue independent, authorized work while an answer is pending.
Do not request approval again when the conversation already supplies it.
If approval is necessary, first prepare the authorized work for review.

Preserve unrelated files and existing user changes.
Keep ordinary code, test, and tooling writes inside the project root.

Deploy only when the user explicitly requests deployment.
For an authorized deployment, use project-owned scripts or commands.
These commands may write documented deployment targets outside the project root, such as `/opt/monoize`.
They may also restart the configured process manager.
Do not modify unrelated external paths.

Use subagents only when the user or applicable instructions request delegation.
Give each delegated task a defined scope, file ownership, and completion condition.

## Specifications

Treat `spec/` as the source of truth for expected system behavior.
Maintain one corresponding specification for each subsystem.
Name each file `<subsystem>.spec.md`, such as `config-system.spec.md` or `billing-engine.spec.md`.
Place every specification directly under `spec/`.
Do not create subdirectories under `spec/`.

Write specifications in concrete English that approximates mathematical language.
State preconditions, inputs, outputs, limits, and postconditions explicitly.
Define state, invariants, and transitions.
Make each requirement testable.
Quantify performance or quality claims.
Avoid hidden assumptions, subjective descriptions, and marketing language.

### New features and behavior changes

1. Read the relevant specification.
2. Resolve missing behavioral requirements before finalizing the specification or implementation.
3. Update or create the specification before changing the implementation.
4. Implement the specified behavior.
5. Keep the specification and implementation aligned in the same change or pull request.

Use existing specifications and conversation context to resolve requirements when they contain the answer.
If required limits, timing, or edge cases remain unresolved, ask the user.

### Bug fixes

1. Read the relevant specification before investigating the implementation.
2. Determine the expected behavior from the specification.
3. If the specification is incorrect or incomplete, correct it before changing the implementation.
4. Fix the implementation to satisfy the specification.

For each observable behavior change, update the corresponding specification in the same change.

### Code Review Rules

Compare the implementation with its specification.
If a specification is missing, derive it from the current implementation before reviewing both.
When the review permits edits, write that specification to `spec/<subsystem>.spec.md`.
For a read-only review, present the derived specification in the review instead of writing a file.
Identify inferred behavior as current behavior, not approved product intent.
Reject specifications that lack concrete, testable requirements.
Request a specification rewrite before accepting the change.

## Canonical URP ownership

Use typed URP fields whenever URP can represent the value.
Do not use internal fields or native replay snapshots to bypass typed URP.
Add a typed URP field when shared protocol semantics need a new representation.
Keep only unknown fields, source provenance, and native shape metadata in adapter extras.
Typed values, including deletion and absence, take precedence over replay metadata.
Do not retain a second text, summary, instruction, or request-control copy in internal fields.

## Provider and Channel migration

For the Provider/Channel model-routing migration, remove obsolete API fields, database columns, tables, entities, and stores.
Do not retain compatibility fields, tables, or aliases.

## Tools and code changes

Use `rg` for text searches and `rg --files` for file searches.
Prefer `ast-grep` (`sg`) for syntax-aware searches when regular expressions would be fragile.

Use the supported CLI for dependency management, component installation, and generated files.
Use Bun for frontend dependencies and scripts.
Run frontend commands from `frontend/`.

| Operation | Command |
| --- | --- |
| Install dependencies | `bun install` |
| Add a dependency | `bun add <package>` |
| Run a script | `bun run <script>` |
| Add a shadcn component | `bunx --bun shadcn@latest add <component>` |

Do not edit `package.json` manually to install a dependency.
Do not copy shadcn component code manually to install a component.
Preserve existing component customizations when the CLI proposes an overwrite.

Use the existing SeaORM migration structure under `src/migration/`.
Register migrations in `src/migration/mod.rs`.
Do not introduce Drizzle commands into this Rust migration workflow.

If a required generator operation is unsupported, verify that limitation in its documentation.
Then make the smallest necessary manual change.
Ensure subsequent generator runs preserve that change or remain compatible with it.
This restriction applies to generated or tool-managed content.
Edit application code and prose directly when the task requires it.

## Code comments

Write all code comments in English.
Add a comment only when it explains reasoning, counter-intuitive behavior, or a non-obvious invariant, complexity guarantee, or trade-off.
Do not add comments that repeat the code.
Use public API docstrings to document inputs, outputs, and behavior.

## Frontend data fetching

Prefer SWR for frontend data fetching.
For each UI surface that fetches data, provide a skeleton during loading or hydration.
For each user-triggered mutation on that surface, provide an optimistic update.
Display fresh data without requiring the user to reopen the surface or refresh the page.

## Documentation

Apply this section to the documentation site, README, and changes to behavior described by the documentation.
Read `spec/docs-site.spec.md` before changing the documentation site under `docs/`.
For documentation UI changes, obey `DESIGN_SYSTEM.md`.

### Language

Write documentation prose in Simplified Technical English (STE).
Use active voice and one term per concept.
Use imperative verbs for instructions.
Keep one instruction per sentence.
Target at most 25 words per English sentence.
Do not use marketing terms such as "seamless", "powerful", "revolutionary", "blazing", "effortless", or "world-class".

Apply STE-derived clarity principles to translations through each language's native grammar.
Keep translations natural and technically precise.
Keep Provider, Channel, transform `type_id` values, environment variable names, and endpoint paths in canonical English.

### Locales

Support exactly `en`, `zh`, `zh-TW`, and `ja`.
When documented behavior changes, update the affected pages in all four locales in the same change.
When adding a built-in transform, add its matching pages in all four locales.
When removing a built-in transform, remove its matching pages in all four locales.
Update the transforms overview pages in the same change.

### Screenshots

Store dashboard screenshots as WebP files.
Use `docs/public/images/en/` for English UI screenshots.
Use `docs/public/images/zh/` for Simplified Chinese UI screenshots.
Reference the `zh` screenshots from `zh` pages.
Reference the `en` screenshots from `en`, `zh-TW`, and `ja` pages.
When a UI change alters a documented flow, recapture both screenshot sets in the same change.

### Build and links

Before merging a documentation-site change, run `cd docs && bun install && bun run build` successfully.
Update README links when documentation URLs change.

## Verification and completion

Do not write or add tests unless the user explicitly requests tests.
Run existing checks that verify the affected behavior and satisfy applicable project requirements.
Choose checks proportional to the change.
Repeat or broaden checks only after further edits, failures, or unresolved evidence gaps.
For instruction-only edits, review the diff and run `git diff --check`.

Do not equate a successful build with verified runtime behavior.
Report failed checks, blockers, and behavior that remains unverified.

Use concise technical prose in progress updates and final responses.
State the result first.
Then describe relevant changes, verification, and remaining limitations.
Use lists for steps or parallel facts.
Avoid filler, invented terminology, and unsupported claims.

## Guidance sources

This file adapts the following OpenAI guidance, checked on 2026-09-07:

- [GPT-6 Astra prompting guidance](https://developers.openai.com/api/docs/guides/latest-model#prompting-best-practices)
- [Custom instructions with AGENTS.md](https://learn.chatgpt.com/docs/agent-configuration/agents-md)

The writing style applies the `ste-writing` skill's controlled-language principles.
