# Accepted implementation contract

Candidate B is the base, selected by the independent comparison in design-judgment.md.
The coordinator owns configuration, lock state, admission, IDs, and result cursors.
Workers publish immutable completions through a separate bounded lane.
The ABI adapter provides one exchange operation and never retains host pointers.
Grafts from A preserve separate legacy and argument call forms and final-statement SQL_CUSTOM output.

Concrete initial bounds are 256 incoming calls, 256 queued jobs, 16 active jobs, 128 retained or synchronous result reservations, and 1 MiB per response.
Request input is capped at 256 KiB. The limits are operational defaults, not facts about extDB3.
Every saved result reserves the maximum result bytes at admission. IDs never expire or reuse.
Unknown retrieval IDs and exact formatting follow black-box captures once available.
RESET remains disabled in this first server subset and returns an explicit error.
No unsafe raw SQL templates are accepted. The active SQL_CUSTOM file uses prepared statements.
No query retries occur after execution starts because commit status may be ambiguous.

Acceptance requires actual-library loading, guard buffers, isolated database effects and reference comparison.
Mandatory verified TLS is a deliberate requirement even where the old server configuration differs.
