# Candidate design A: bounded dispatcher with explicit request and result states

## Problem

Build a greenfield Rust 2024 `cdylib` that can replace the Exile server's extDB3 extension on Windows and Linux. The extension must preserve the documented call protocol while moving MySQL work off the Arma callback thread for async calls. The hard constraints are the `0`, `1`, `2`, `4`, and `5` call types; a bounded Tokio worker queue; a pool per named database; mandatory TLS certificate and hostname checks; and the Arma output buffer's trailing NUL. No extDB3 implementation was inspected. The protocol facts below come from the [extDB3 General wiki](https://github.com/SteezCram/extDB3/wiki/extDB3---General), [SQL_CUSTOM wiki](https://github.com/SteezCram/extDB3/wiki/extDB3---SQL-custom), [SQL_CUSTOM INI wiki](https://github.com/SteezCram/extDB3/wiki/extDB3---sql_custom.ini), and [System wiki](https://github.com/SteezCram/extDB3/wiki/extDB3---System).

## Usage (caller's view)

The consumer keeps the extDB3 style of calling the library. System setup names a configured database and associates an SQL_CUSTOM file with a protocol name.

```sqf
"a3-mysql-ext" callExtension "9:ADD_DATABASE:main";
"a3-mysql-ext" callExtension "9:ADD_DATABASE_PROTOCOL:main:SQL_CUSTOM:sql:exile.ini";
"a3-mysql-ext" callExtension "9:LOCK";
```

The query ID is looked up in the loaded file, and each input is bound as a database parameter.

```sqf
"a3-mysql-ext" callExtension "0:sql:findPlayer:76561198000000000";
```

Discard-only async calls return an acknowledgement. Tracked async calls return an opaque numeric ID, which is polled until ready. A single-part ready result is consumed by type 4. A multipart result switches to type 5 and is consumed through its empty-string terminator.

```sqf
private _id = "a3-mysql-ext" callExtension "2:sql:savePlayer:76561198000000000";
private _single = "a3-mysql-ext" callExtension format ["4:%1", _id];
// If _single is [5], retrieve fragments with 5:<id> until the extension returns "".
```

The ABI caller sees only the extDB3 response text. Rust domain types and database driver types stay private.

## Shape

### Module map

```text
src/
  abi.rs       exported ABI shims, pointer/size checks, NUL-terminated copy
  protocol.rs  parse call syntax into typed commands; render extDB-compatible results
  runtime.rs   singleton lifecycle, bounded mpsc sender, worker tasks
  registry.rs  database/pool registry, SQL_CUSTOM query registry, lock state
  query.rs     config parsing, positional input mapping, result rendering
  results.rs   tracked job state machine, bounded result storage, poll and consume
  main.rs      private module declarations and exported entry points
```

Each module owns one body of knowledge. `abi.rs` is the only unsafe boundary. `protocol.rs` converts C strings into typed commands before dispatch. `runtime.rs` owns work admission and worker lifetime. `registry.rs` owns immutable loaded configuration and its per-database pools. `results.rs` is the sole writer of tracked-job state. `query.rs` binds parameters and converts MySQL values to SQF-compatible output.

### Core types

```rust
struct Extension {
    lifecycle: Mutex<Lifecycle>,
    runtime: OnceLock<Runtime>,
}

enum Lifecycle {
    Uninitialized,
    Configuring(Builder),
    Locked(Arc<Registry>),
}

struct Runtime {
    submit: tokio::sync::mpsc::Sender<Work>, // bounded; try_send only
    results: ResultRegistry,
    next_id: AtomicU64,
    handle: tokio::runtime::Handle,
}

struct Registry {
    databases: HashMap<DatabaseName, Arc<Database>>,
    protocols: HashMap<ProtocolName, Protocol>,
}

struct Database {
    pool: mysql_async::Pool,
    queries: HashMap<QueryName, QueryPlan>,
}

struct QueryPlan {
    steps: NonEmptyVec<PreparedStep>,
    output: OutputPlan,
    return_insert_id: bool,
}

struct PreparedStep {
    sql: String,                 // placeholders validated during config load
    bindings: Vec<InputBinding>, // positional source slots and option transforms
}

enum InputBinding {
    Parameter(InputIndex),
    Constant(SqlValue),
}

enum Work {
    Execute { request: QueryRequest, completion: Completion },
}

enum Completion {
    Discard,
    Synchronous(tokio::sync::oneshot::Sender<Result<EncodedResult, DbError>>),
    Tracked(JobId),
}

enum JobState {
    Queued,
    Running,
    Ready(ResultPayload),
}

enum ResultPayload {
    Single(String),
    Multipart(VecDeque<String>),
}

struct JobId(NonZeroU64);
struct DatabaseName(String);
struct ProtocolName(String);
struct QueryName(String);
struct InputIndex(usize);
struct QueryRequest { database: DatabaseName, query: QueryName, inputs: Vec<SqlValue> }
struct EncodedResult { parts: NonEmptyVec<String> }
struct NonEmptyVec<T> { first: T, rest: Vec<T> }

enum CallInput<'a> {
    LegacyString(&'a CStr),
    FunctionAndArgs { function: &'a CStr, args: &'a [*const c_char] },
}
```

The sketch uses names and shapes, not extDB3 wire types, as internal types. Concrete `SqlValue` variants should cover NULL, bool, signed/unsigned integer, finite float, bytes, and text; reject non-finite floats at the input boundary. Each variant maps to the corresponding `mysql_async::Value` without interpolating SQL. Where the INI format defines SQL text rather than prepared statements, support it only by parsing and binding its documented placeholders; do not implement string concatenation or an ad hoc escaping substitute for prepared binding. A query with multiple numbered SQL lines is an ordered `NonEmptyVec`; only its final step's output plan applies, matching the wiki's stated output behavior.

### Entry points and principal operations

```rust
// Windows exports use system ABI; the Linux build exports the C ABI.
unsafe extern "system" fn RVExtension(output: *mut c_char,
    output_size: u32, function: *const c_char);
unsafe extern "system" fn RVExtensionArgs(output: *mut c_char,
    output_size: u32, function: *const c_char,
    args: *const *const c_char, argc: u32) -> c_int;
unsafe extern "system" fn RVExtensionVersion(output: *mut c_char,
    output_size: u32);

fn dispatch(command: Command, extension: &Extension) -> WireResponse;
fn submit(request: QueryRequest, mode: RequestMode, runtime: &Runtime)
    -> Result<WireResponse, AdmissionError>;
fn run_query(registry: &Registry, request: QueryRequest)
    -> impl Future<Output = Result<EncodedResult, DbError>>;
fn complete(completion: Completion, result: Result<EncodedResult, DbError>,
    results: &ResultRegistry);
fn poll_single(id: JobId, results: &ResultRegistry) -> WireResponse;
fn poll_multipart(id: JobId, results: &ResultRegistry) -> WireResponse;
fn write_abi_response(dst: *mut c_char, capacity: usize, response: &str)
    -> Result<(), AbiError>;
```

On Linux, compile the same declarations with `extern "C"`. The Windows build must export the expected symbol names with the system ABI. `RVExtensionArgs` returns an integer status as required by the Arma extension ABI; the compatibility path should return the documented success code after writing the normal extension response. Each ABI shim validates pointers and the unsigned buffer/argument counts before constructing slices or C strings. `write_abi_response` reserves one byte for `\0`, copies at most `capacity - 1` bytes only when the whole UTF-8 response fits, and always writes a terminator when capacity is positive. It returns a fixed short error response if the complete response cannot fit; it never truncates inside UTF-8 or SQF syntax. The handler must ensure even that fallback fits, otherwise write an empty NUL-terminated response. Keep the version string within the documented 32-byte buffer.

Both ABI call forms become a `CallInput` at the boundary. The string form goes through the extDB3 colon parser. The argument form validates every pointer and passes the function and argument list to its own normalization function; it must not silently join values with colons, since that would turn distinct array elements into ambiguous protocol data. Captures and the installed extension headers determine the exact normalization contract before implementation.

`RequestMode` is a closed enum derived once from the call type: `Sync`, `DiscardAsync`, or `TrackedAsync`. `Command` is a closed enum covering query submission, result retrieval, version, output size, database/protocol setup, lock status, lock, and supported system/time commands. Unsupported commands return a protocol-shaped error.

### Request and result state machine

```text
parse input -> validate mode and query -> try_send bounded work
   Sync:          wait on oneshot -> encode one response -> return
   DiscardAsync:  worker runs query -> drop result -> return acknowledgement
   TrackedAsync:  allocate JobId -> insert Queued -> worker runs -> Ready

type 4:<id>:
   Queued | Running        -> [3]                         (retain ID)
   Ready(Single)            -> [1,<encoded result>]        (remove ID)
   Ready(Multipart(parts))  -> [5]                         (retain as multipart)
   unknown/consumed ID      -> [0,<error>]                 (single request error)

type 5:<id>:
   Queued | Running          -> [3]                        (retain ID)
   Ready(Multipart(nonempty))-> one complete part verbatim  (consume one part)
   Multipart(empty after pop) -> ""                         (remove ID)
   unknown ID                -> ""                         (matches documented terminator)
```

The type-4 transition is serialized under the result registry lock. On a multipart marker, the first type-5 poll may follow immediately. Each complete fragment is emitted unchanged; the last data fragment does not free the ID, because the caller must observe the following empty-string terminator. Empty-ID termination and unknown-ID termination are indistinguishable per the wiki. A type-4 poll consumes a single result once; concurrent polls therefore have exactly one winner. Type 5 is allowed only after type 4 reports `[5]`, so a type-5 request against a single result is an error.

### Bounds and concurrency

Use a bounded Tokio `mpsc` queue with nonblocking `try_send` at the ABI boundary. Queue-full and closed-runtime states return an explicit busy/error response immediately. Never await queue capacity while holding the configuration or result lock. A configurable fixed worker count drains the same queue; database pools cap per-database connections independently. MySQL futures are async, so one worker does not reserve a thread while a query waits on the network.

Tracked submissions reserve a result slot before enqueue. Registry admission rejects when the configured tracked-job count is full. Enforce maximum input bytes and argument count before parsing, maximum config/query count while loading, maximum result bytes per job, and maximum retained result bytes across jobs. Account result bytes before publishing `Ready`; on budget exhaustion, publish a bounded error result and release the failed payload. Type 1 reserves no result slot. IDs are monotonic nonzero `u64`s and checked against the registry on insertion; refuse exhaustion rather than reuse a live ID. Store timestamps only if an explicit expiry policy is implemented; silent expiry would break clients polling slow but valid work.

The result registry is shared because multiple Arma callbacks must poll jobs created by other callbacks. It owns the one mutable state per ID and uses a short mutex only for insert, transition, and part removal. Query execution and encoding happen outside that mutex. The async worker sends completion to the registry; it does not mutate registry internals directly. Pools are shared as cloneable handles and owned once by the immutable `Registry`.

### SQL_CUSTOM config and safe SQF encoding

At setup time, load the named SQL_CUSTOM INI into a private `QueryPlan` map. Reject duplicate query names, invalid numbered-line sequences, invalid input indexes, malformed options, unsupported config versions, and placeholder/parameter count mismatches before locking configuration. Validate setup transitions once: databases and protocols can be added only in `Configuring`; `LOCK` atomically installs an immutable `Arc<Registry>` and makes further configuration calls fail. Database TLS is mandatory at pool construction. Configuration must provide a CA trust root or use the platform root store, and the TLS connector must verify both certificate chain and hostname. No insecure toggle or fallback exists.

Prepared queries bind each SQF input into `mysql_async::Params`. SQL identifiers and query structure come only from the trusted server-side INI. Input option transforms such as `bool`, `null`, and `time` convert values into typed `SqlValue`s. `mysql_escape` is valid only for a legacy raw-SQL plan; it is not applied to prepared statements. The default and per-query strip rules are input validation/compatibility behavior from the INI, not a security barrier. The system must reject a query that requires the legacy raw interpolation semantics unless that compatibility behavior is explicitly implemented and separately validated; no input is spliced into SQL by default.

The result encoder has one SQF string-escaping function that doubles embedded quotes per SQF string syntax, formats scalars explicitly, and renders arrays recursively. It rejects unsupported values rather than using Rust debug formatting. SQL NULL maps through the configured output option to the documented SQF null form. Every generated part is a complete, valid protocol fragment within the extension response cap, and all output-size decisions count the final NUL byte.

### Interface depth

The exported ABI is necessarily broad because Arma calls named functions and the legacy colon command. Internally, there is one `dispatch` operation over a typed `Command`, one bounded submission path, and two retrieval operations. Callers do not coordinate queue mechanics, pools, SQL parsing, or result locks. The design keeps boundary syntax private, while the registry and result store hide meaningful protocol and concurrency policy behind small types.

## Synthesis decision

This is candidate A for comparison. It chooses one bounded dispatcher, one immutable configuration owner, and one explicit result-state machine. No synthesis across other candidates has occurred in this package.

## Tradeoffs accepted

- We accept that type-0 callbacks block their caller until the database returns in exchange for preserving synchronous protocol behavior while the worker owns network execution.
- We accept immediate busy responses when bounded queue or result capacity is exhausted in exchange for a hard memory ceiling and predictable ABI callback latency.
- We accept one shared result registry and a short mutex in exchange for poll IDs that work across concurrent Arma callbacks; execution never holds this lock.
- We accept rejecting legacy raw SQL constructs whose semantics are not known in exchange for prepared binding as the default safety rule.
- We accept complete-fragment size limits in exchange for never returning truncated text that Arma cannot parse.
- We accept mandatory verified TLS failing closed when CA or hostname validation cannot succeed in exchange for preventing silent plaintext or unverified fallback.

## Alternatives considered

- A pool of independently owned result actors, one per job, could avoid a central result mutex, but it adds actor handles and lifecycle coordination to every poll while the public behavior still needs a shared ID lookup. The central registry has greater interface depth because it hides allocation, lookup, readiness, and consume rules behind `poll_single` and `poll_multipart`.
- An unbounded task spawn per request with per-database pool limits would simplify submission, but exposes memory and scheduler growth to traffic bursts. The bounded queue exposes one explicit capacity outcome and keeps overload policy inside `Runtime`.
- Direct SQL interpolation with escaping could mimic more legacy SQL_CUSTOM options, but duplicates MySQL quoting rules and makes safe handling depend on string context. Typed parameters hide value serialization in the driver and leave only the trusted SQL template visible in configuration.

## Implementation reconciliation

No implementation deviations have been accepted. During implementation, update the usage, state transitions, signatures, and ownership here whenever an accepted change alters a caller-visible outcome or shared contract. Record the source that accepted each deviation.

## Open questions and risks

- Which exact `sql_custom.ini` files and captured SQF request/response examples define the compatibility subset, especially legacy non-prepared SQL and uncommon output options?
- Which MySQL server versions, CA delivery method, and hostname form will production use, so the TLS connector can be pinned to supported behavior and tested against the real certificate chain?
- What are the actual Windows and Linux Arma callback signatures, symbol decoration rules, and input text encoding for the target server versions, so the ABI can be verified against the right headers and captures?
- Which `RVExtensionArgs` argument transformation does the target server use, so it can be implemented as a true equivalent instead of an assumed wrapper?
- What output-size and burst limits should be set from actual server captures and workload, because the wiki does not specify practical maxima?
- How should type-1 database errors be reported when the caller discards results, because the public call returns before the worker sees the database outcome?
- The System wiki describes synchronous setup commands including `ADD_DATABASE`, `ADD_DATABASE_PROTOCOL`, and `LOCK`; should reset/unlock and every time/system command be part of the first compatibility release, or should unsupported commands return explicit errors?

## Next implementation step

Build `tests/abi_replay` as a separate process that loads the compiled `.dll` or `.so` with `LoadLibrary`/`dlopen`, resolves `RVExtension`, `RVExtensionArgs`, and `RVExtensionVersion`, invokes the actual exported symbols, and checks NUL termination and capacity handling. Feed it captured extDB3 input/output pairs and compare responses byte-for-byte where behavior is specified. Add a second mode that runs the same SQL_CUSTOM fixture against the replacement and a reference extDB3 process connected to a disposable MySQL instance, then compares success/error shape, row order, scalar conversion, insert ID, and type-4/type-5 consume sequences. Keep unknown capture cases as explicit compatibility gaps. Start with the ABI replay and a checked-in synthetic fixture because production captures, credentials, and server version are still pending; do not claim full differential parity until those inputs are supplied.
