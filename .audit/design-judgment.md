# Design judgment

Scores use 0 for absent or unsafe and 5 for the strongest evidence in the design.

| Criterion | A | B |
| --- | ---: | ---: |
| Lifecycle correctness and consume semantics | 4 | 4 |
| Bounded memory, queue, and inflight work | 4 | 5 |
| Small interface and maintainability | 4 | 4 |
| Legacy Exile SQL_CUSTOM fit | 3 | 3 |
| Actual ABI and differential verifiability | 4 | 5 |

**Lifecycle and consume semantics.** A explicitly defines queued/running/ready states, one-shot type 4 consumption, multipart type 5 consumption, and the required empty terminator. Its contract rejects type 5 against a single result, but that behavior and its fixed overflow error need black-box confirmation. B gives a more complete capacity-sensitive lifecycle, including type-0 overflow becoming a ticket and non-consuming small-buffer responses. It also names RESET draining and epochs. Both correctly keep pending results pollable and retain multipart tickets through the terminating empty response. B's simultaneous-poll linearization is stronger; both need reference captures for unspecified cases.

**Bounds.** A bounds the work queue, worker count, tracked-job count, per-result and aggregate retained bytes, and input/config sizes. It does not state a per-database queued limit or how type-1 row fetching and transient encoding stay bounded. B explicitly bounds those, reserves result capacity before admission, keeps completion capacity separate from external commands, and avoids spawning waiting tasks. Its maximum-size reservation is conservative but makes retained memory provable. Both still defer concrete limits to workload evidence.

**Interface and maintainability.** A's private domain types and distinct configuration, runtime, and result owners are clear. `poll_single` and `poll_multipart` separate protocol actions without exporting wire representations, though ownership spans a mutex-backed registry and runtime completion path. B's `engine.exchange` is a particularly small caller surface and the coordinator gives shared ticket and reset invariants one owner. Its actor adds mailbox scheduling and reply plumbing, but those hide policy rather than form a pass-through chain. Both avoid shallow modules and public protocol leakage; neither has a decisive maintainability advantage.

**SQL_CUSTOM fit.** Both retain numbered statement order, typed transforms, prepared binding, safe SQF output, and explicitly reject unknown raw interpolation semantics. That is a sound safety baseline, but it can reject real Exile configurations using legacy substitution, uncommon options, or SQL_CUSTOM-specific output behavior. A is clearer about `mysql_escape` being legacy-only and final-step output behavior; B adds same-connection multi-statement execution and retry caution. Neither establishes compatibility against an actual Exile `exile.ini` fixture, so both score 3.

**ABI and differential verification.** A calls for a subprocess loader of all three exports, byte comparisons, and SQL_CUSTOM replay against reference extDB3. It recognizes ABI signatures and `RVExtensionArgs` normalization as evidence gates, though its proposed fixed overflow response is not yet grounded. B strengthens the replay with guard bytes, capacity matrices, subprocess isolation, ABI return-code capture, concurrency/saturation cases, and broader SQL side-effect comparison. Both correctly avoid claiming parity without captures. B's verification plan is more executable and directly tests the loaded ABI.

## Recommendation

Use B as the base. Graft A's concise `CallInput` distinction for legacy string calls versus `RVExtensionArgs`, its explicit final-numbered-statement output rule, and its clear one-shot type-4/multipart type-5 contract. Preserve B's actor-owned lifecycle, capacity-aware overflow ticket, bounded transient type-1 encoding, completion lane, reset draining, and expanded loaded-library replay plan. Keep both candidates' prepared-binding default and make actual SQL_CUSTOM inventory plus reference differential replay a release gate.

The red-flag screen finds no disqualifying shallow module, information leakage, temporal decomposition, pass-through layer, split result ownership, duplicate task path, importable internals, or hand-synced list in either proposed shape. The unresolved risk is compatibility evidence, especially real SQL_CUSTOM semantics and exact engine ABI behavior, not a design-screen failure.
