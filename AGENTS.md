# Modular architecture and regression-safety requirements

This repository follows the owner's **modular-by-default** engineering policy. Apply it to every new feature, bug fix and future contribution, including work by coding agents. Existing monolithic code should be migrated in small, testable steps, not rewritten wholesale.

## Before editing

- Read the current specifications, code structure, outstanding PRs, owner handoffs, build instructions and tests. Respect active file ownership and avoid duplicate branches or work.
- Identify the interfaces, file formats, user-facing behavior, audio/MIDI processing invariants (if applicable), performance, supported platforms, and expected error behavior that must be preserved.
- Establish a passing baseline. Add characterization tests covering current behavior before moving implementations.

## Module design

- Give each module **one cohesive responsibility** and a minimal, explicit contract. Prefer composition and dependency injection over global state and circular dependencies.
- Separate core algorithms and domain models from GUI, persistence, network, device, operating-system, plugin/DAW or framework adapters. UI should call services/interfaces, never own business algorithms.
- Preserve public APIs, function signatures, existing settings/presets, serialized state, binary/plugin IDs, command names, output and file formats unless a separately approved versioned migration is provided.
- Where existing code is intertwined, introduce adapters or facades and move one seam at a time. Make refactoring a separate PR from new features or changed behavior. Avoid arbitrary micro-modules and unnecessary abstractions.
- Explicitly own resources, concurrency, lifecycle, errors, cancellation, buffers and bounds. Add tests for limits and failures, not only successful paths.
- For **JUCE/VST/audio/MIDI**: keep real-time callbacks bounded and deterministic with no blocking, dynamic allocation, disk/network I/O, logging, mutex contention or UI access. Preserve parameter IDs, automation, state restoration, latency and sample-accurate behavior. Compare DSP results against reference tests before optimization.
- Document module purpose, public entry points, dependencies, invariants and migration notes in the repository's existing docs or a small module index.

## Required verification for every change

- Prove tests fail on a relevant regression before a bug fix, or establish behavior-equivalence tests before a refactor.
- Run targeted and full available tests, linters, static checks and build configurations applicable to changed code. Windows/plugin/standalone builds must be checked on real matching runners; never claim device/host verification from a Linux compile.
- Do not weaken tests, delete features, discard work or merge a failing/uncertain change merely to finish modularization. Prefer short-lived branches, reviewed passing PRs, incremental merges and easy rollback.
- If a safe seam cannot be established, leave code intact and record the prerequisite characterization tests and specification instead of risking a break.

This policy affects **future code immediately**. Legacy conversion is progressive and must remain feature-compatible.
