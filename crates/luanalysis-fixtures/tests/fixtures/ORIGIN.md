# Maintained checking inputs

These Lua files are owned fixtures for the standalone checker. Their source,
markers and expected behavior are maintained together with the implementation.

The complete `local_def_assignment.lua`, `lambda_params.lua`, `trailing_type.lua`
and `snippet.lua` inputs now live in `../legacy/resources/inspections/`, with
upstream provenance documented in `../legacy/ORIGIN.md`. Marker removal preserves every
non-marker byte, and expected ranges refer to the resulting clean Lua. Negative
cases also run with temporary in-memory corrections; those corrections leave the
stored marked input intact.

Three files exercise deliberately smaller primitive contracts:

- `number-assignment.lua`: immediate numeric initialization followed by another
  compatible assignment. It omits calls, arithmetic and literal-only types.
- `boolean-assignment.lua`: immediate initialization of a boolean value. It
  isolates the primitive contract from uninitialized-local and literal policies.
- `string-assignment.lua`: immediate initialization followed by another
  compatible string assignment. It omits literal-only declarations.

These reductions establish their own assignments and displayed types. Broader
operator, inference, literal and declaration behavior needs additional cases;
a reduction is not evidence that an entire feature family is complete.
