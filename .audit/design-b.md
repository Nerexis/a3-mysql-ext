# Candidate B. Actor-owned extension state

## Problem

Build a greenfield Rust 1.99 Edition 2024 cdylib that existing Exile SQF can call through the extDB3 wire protocol. Preserve observable responses while keeping database execution, response storage, and synchronous FFI calls bounded. No extDB3 implementation, installed credentials, or binaries were inspected for this design. The installed directory is only a prospective black-box fixture source. This is a design artifact, not a claim of compatibility.

## Usage

Existing callers retain these calls.

```sqf
"extDB3" callExtension "9:ADD_DATABASE:exile";
"extDB3" callExtension "9:ADD_DATABASE_PROTOCOL:exile:SQL_CUSTOM:SQL:exile.ini";
"extDB3" callExtension "9:LOCK";
private _direct = "extDB3" callExtension "0:SQL:getPlayer:76561198000000000";
private _ticket = "extDB3" callExtension "2:SQL:getPlayer:76561198000000000";
"extDB3" callExtension "1:SQL:updatePlayer:76561198000000000:Name";
```

The caller polls the returned ticket with type 4. A wait response preserves the ticket. A multipart response switches the caller to repeated type 5 calls whose raw fragments concatenate into the original response. The terminating empty response releases the ticket. A synchronous response that does not fit also returns a ticket.

Internally the FFI adapter makes one call, `engine.exchange(request, payload_capacity)`. Database workers never receive FFI pointers. The replay program calls exported symbols in a loaded library rather than importing Rust internals.

## Shape

One coordinator actor owns protocol registrations, setup lock state, admission, ticket allocation, and every result cursor. Worker tasks own their query execution and publish immutable completions. This choice follows separate-before-serializing-shared-state. Ticket reads and writes are a genuinely shared invariant, so the actor serializes them structurally.

The engine interface hides lifecycle, scheduling, and consumption behind one operation. Callers see request text, output capacity, and response bytes. `ffi.rs` owns pointers and ABI. `engine.rs` owns commands, admission, and response state. `sql_custom.rs` owns compiled configuration and parameter transformations. `database.rs` owns mysql_async execution, pool options, and bounded result encoding. `sqf.rs` owns literal parsing and serialization. These are knowledge boundaries, not sequential processing stages. Modules remain private.

```rust
struct Engine { commands: Sender<Command>, runtime: RuntimeOwner }
impl Engine {
    fn exchange(&self, request: &[u8], capacity: PayloadCapacity) -> EncodedReply;
}
struct PayloadCapacity(usize); // output_size minus one, validated by FFI
struct Ticket(NonZeroU64); // monotonic decimal wire representation, never reused
struct Epoch(u64);
enum Delivery { Discard, Immediate(ReplySender), Retained(Ticket) }
struct AcceptedJob {
    epoch: Epoch, plan: Arc<CallPlan>, inputs: BoundInputs,
    delivery: Delivery, budget: ResultReservation,
}
enum ResultState {
    Pending,
    Ready { bytes: EncodedSqf, offset: usize },
    AwaitingTerminator,
}
enum SetupState { Open, LockedPermanent, LockedWithSecret(SecretUnlockCode), Draining }
struct Coordinator {
    setup: SetupState, epoch: Epoch,
    protocols: HashMap<ProtocolName, Arc<CallPlanSet>>,
    databases: HashMap<DatabaseName, DatabaseDispatcher>,
    results: HashMap<Ticket, ResultEntry>,
    outstanding: usize, reserved_result_bytes: usize,
}
enum Command {
    Exchange { request: ParsedRequest, capacity: PayloadCapacity, reply: ReplySender },
    Completed { epoch: Epoch, job: JobId, outcome: QueryOutcome },
}
struct DatabaseDispatcher { queue: Sender<AcceptedJob>, pool: mysql_async::Pool }
struct CallPlan { statements: Vec<StatementPlan>, output: OutputPlan, retry: RetryPlan }
struct StatementPlan { sql: PreparedSql, binding: Vec<InputBinding> }
fn compile_custom(bytes: &[u8], limits: &Limits) -> Result<CallPlanSet, ConfigError>;
fn bind(plan: &CallPlan, input: ProtocolInput) -> Result<BoundInputs, InputError>;
async fn execute(pool: &mysql_async::Pool, job: AcceptedJob) -> QueryOutcome;
fn encode_cell(value: DatabaseCell, policy: &ColumnPolicy) -> Result<SqfValue, EncodeError>;
```

All signatures are sketches with unimplemented bodies. Parsed requests, validated capacities, bound parameters, and encoded SQF cannot be constructed outside their owning modules. This follows boundary-discipline. ResultState replaces separate completion, multipart, and consumed flags, per model-the-domain.

## State and delivery contracts

| Event | State and response |
| --- | --- |
| Admit type 0 | Reserve queue and result budget. Keep a deferred reply sender until completion. The actor continues handling other commands. |
| Complete type 0 | Return the entire encoded response if it fits. Otherwise create Ready ticket and return its ID. |
| Admit type 1 | Reserve queue capacity and transient encoding budget. Acknowledge acceptance. Allocate no ticket and retain no result. |
| Admit type 2 | Atomically reserve queue, ticket slot, and maximum result bytes. Publish Pending and return its ID. |
| Worker completion | Pending becomes Ready, including encoded errors. Release execution permits. Keep retained result reservation. |
| Type 4 Pending | Return `[3]`, no mutation. |
| Type 4 Ready with complete response fitting this call | Return complete bytes and remove entry. |
| Type 4 Ready exceeding this call capacity | Return `[5]`, no mutation. |
| Type 5 Pending | Return `[3]`, no mutation. |
| Type 5 Ready | Return the largest raw fragment that fits. Advance offset. Last nonempty fragment changes state to AwaitingTerminator. |
| Type 5 AwaitingTerminator | Return empty bytes and remove entry. |
| Unknown type 5 ID | Return empty bytes. |

After partial type 5 retrieval, type 4 returns `[5]` rather than a truncated remainder. Exact behavior for unknown type 4 IDs and cross-mode retrieval is a differential-test gate. Fragments split only at UTF-8 boundaries, unless black-box evidence establishes a different required byte convention. Capacity too small for a control response returns a deterministic empty NUL-terminated buffer and consumes nothing. Admission must reject before enqueue if even a ticket or acceptance response cannot fit. Concurrent retrieval calls linearize at the actor. Their outputs require caller ordering to reconstruct a multipart response.

Completion messages use reserved capacity separate from the external command mailbox. An admitted job therefore never loses its completion due to mailbox saturation. The actor never awaits a query, a full work queue, or a blocking synchronous caller. FFI caller threads wait on ordinary reply channels; the actor runs on its dedicated Tokio runtime. Query workers run concurrently up to each database's configured limit. One pool exists per database registration.

## Admission and resource bounds

Use explicit configured maxima for input bytes, argument count, protocols, databases, mailbox entries, queued jobs per database, active jobs, outstanding retained tickets, encoded bytes per result, total retained bytes, rows, nesting, and retry attempts. Set concrete defaults after replaying representative Exile fixtures. This design does not invent workload limits as compatibility facts.

Reserve maximum retained result bytes at acceptance. This wastes capacity for small results but guarantees a worker can publish without exceeding the global budget. Type 1 also needs bounded fetch or encoding even though its result is discarded. Stream database rows into a capped encoder rather than collecting an unbounded Vec first. On excess, discard the partial response and produce a bounded error. Queue admission uses try_reserve and rejection is explicit. Never spawn one waiting task per rejected or queued job.

Use separate bounded lanes for external calls and completions, with weighted service so polling cannot starve completions and completions cannot starve control requests. RESET enters Draining, rejects new SQL, lets accepted jobs settle, disconnects pools, then clears registrations and results. Epoch tags prevent stale completions from reviving cleared tickets. RESET requires unlocked state and its configuration gate. Do not automatically expire accepted results in compatibility mode. Unconsumed tickets eventually cause explicit admission rejection. An optional operational expiry policy must be separately documented as a compatibility deviation.

## SQL_CUSTOM and SQF safety

Compile configuration before publishing its protocol registration. Assemble SQL fragments in numeric order, retain ordered statements and input mapping, and reject unknown options or inconsistent placeholders. Prepared statements bind mysql_async values through parameters. User values never become SQL text. Resolve transformations into typed operations rather than evaluating names at query time. One connection executes an entire multi-statement call so session state and insert IDs belong to the same connection.

Raw SQL templates need a lexical conversion to prepared placeholders. Quoted whole-value tokens can become parameters; identifier substitutions, partial tokens, or ambiguous SQL must fail protocol registration with the affected section named. This may reject some legacy files. Do not silently approximate them or claim full compatibility before inventorying the actual SQL_CUSTOM file. mysql_escape remains unnecessary for prepared bindings; its legacy semantics need an explicit tested mapping.

SQF encoding builds an AST of safe literals. Quote strings by doubling embedded double quotes. Treat database text as data. A value that should represent an SQF array must parse as a bounded literal grammar, never pass arbitrary text to compile. Handle null, bool, dates, insert ID strings, numeric precision, and byte decoding through explicit policies. Reject nonfinite numeric output and unrepresentable control bytes. Configuration transforms cannot bypass final safe serialization. Raw fragments are slices of already encoded whole responses, never independently quoted wrappers.

Retries after connection loss can repeat writes whose commit status is unknown. No blanket retry of mutation sequences. Classify definite pre-execution failures separately from ambiguous outcomes, and replay documented retry behavior only where it is safe or the operator deliberately accepts it. Transaction wrapping is not implicit because it changes legacy autocommit behavior.

## ABI and TLS

Pin mysql_async exactly to `=0.37.1`, disable default features, and select `default-rustls-ring`. Rust 1.99 and Edition 2024 are prescribed targets, not toolchain availability claims. On Windows export RVExtension, RVExtensionArgs, and RVExtensionVersion using system ABI; on Linux use C ABI. Edition 2024 export attributes use `#[unsafe(no_mangle)]`. RVExtensionArgs has its specified integer return and argument pointer/count handling. Confirm x86 decoration and all exact signatures against engine documentation before implementation.

Validate positive output_size and reserve one byte for NUL. Capacity zero writes nothing. Never retain caller pointers, never write beyond the validated buffer, and contain every Rust panic at the exported boundary. Handle malformed UTF-8, invalid counts, and embedded NUL deterministically. Pointer readability remains the host contract; Rust cannot validate arbitrary pointer provenance.

Build pools only with TLS required and valid trust roots, certificate validation, and hostname validation. There is no insecure configuration mode. Database endpoint naming must match the certificate. A credential or TLS configuration error returns a sanitized bounded response; logs omit passwords, query parameters, and unlock secrets. Tests must prove plaintext refusal, unknown CA refusal, expired certificate refusal, hostname mismatch refusal, and accepted valid DNS certificate. The docs.rs 0.37.1 SslOpts page did not expose usable option signatures during this pass, so exact builders and enforcement must be verified from available dependency documentation before code is written.

## Testing the actual dynamic library

Build a standalone replay executable that loads the emitted DLL or SO, resolves all three exports, invokes them with platform ABI types, and records exact output bytes plus RVExtensionArgs return codes. Run each library in a fresh subprocess to isolate globals, crashes, and unload hazards. Use guard bytes around buffers and matrix capacities including 0, 1, just below response length, exact length plus NUL, and oversized buffers. Verify exports with platform tooling and run both engine architectures actually required by the deployment.

For differential tests, run the installed extDB3 binary and replacement in separate processes with disposable database clones and sanitized fixtures. Do not read implementation or production credentials. Normalize allocated ticket identities, not result bytes. Poll until terminal outcomes with a bounded test deadline; do not compare transient wait counts or thread scheduling. Compare result bytes, concatenated multipart bytes, consumption, lock/reset outcomes, SQL effects, insert IDs, transformations, duplicate polling, and failure responses. Replay all Exile call sections once an approved nonsecret fixture is available. Exercise writes only against disposable databases. Security tests assert the replacement's mandatory TLS policy separately if the original lacks it.

Actor tests cover simultaneous polls, completion racing RESET, saturation, tiny buffer retries, consumed ticket access, and partial-fragment transitions. Database tests cover bounded large rows, multiple statements, TLS refusal, invalid SQF literals, and ambiguous write failures. These test domain behavior; the release gate still requires the real loaded library and Exile caller replay.

## Synthesis decision

Candidate B recommends actor ownership. Final base selection belongs to the orchestrator after comparison with the other candidate.

## Tradeoffs accepted

- We accept one mailbox round trip for each poll in exchange for one owner of ticket consumption and RESET transitions.
- We accept conservative maximum-result reservations in exchange for a provable global memory bound.
- We accept coordinator throughput limits in exchange for auditable linearization. Database work stays concurrent.
- We accept rejection of unsafe raw substitutions in exchange for prepared parameter binding. This deviation requires actual configuration inventory.

## Alternatives considered

A lock registry gives FFI polling direct access to result state and avoids mailbox latency. Its interface can be equally small, but completion, polling, registration, and reset share mutation rules across threads. It wins if measured coordinator contention dominates and carefully scoped locks retain one source of truth. The actor wins here because the lifecycle proof is local and workers only publish facts.

One actor per ticket removes coordinator contention but requires a second owner for ID routing, lifetime, global admission, and RESET. It exposes more internal coordination while hiding little extra policy. It loses on ownership and task-count bounds.

## Implementation reconciliation

No implementation exists. No deviation has been accepted. The candidate passes the design red-flag screen because public callers use one operation, private domain owners hold each invariant once, and worker completion does not share registry mutation.

## Open questions and risks

Which actual SQL_CUSTOM sections require raw substitution, SQF expressions, uncommon transformations, or retry semantics that the safe prepared plan cannot preserve? Can the installed binary supply sanitized expected responses for unspecified unknown-ID, mixed retrieval, and very-small-buffer behavior? Does the mandatory TLS endpoint have a usable verified DNS name and trust chain? These are evidence gates, not reasons to guess compatibility.

## Next implementation step

Build the FFI replay executable and a minimal library with bounded byte copying before implementing database behavior.

## Sources

- [General wire contract](https://github.com/SteezCram/extDB3/wiki/extDB3---General).
- [SQL_CUSTOM registration](https://github.com/SteezCram/extDB3/wiki/extDB3---SQL-Custom).
- [SQL_CUSTOM configuration](https://github.com/SteezCram/extDB3/wiki/extDB3---sql_custom.ini).
- [System lifecycle commands](https://github.com/SteezCram/extDB3/wiki/extDB3---System).
- [mysql_async 0.37.1 SslOpts documentation endpoint](https://docs.rs/mysql_async/0.37.1/mysql_async/struct.SslOpts.html). Exact option names remain unverified.
