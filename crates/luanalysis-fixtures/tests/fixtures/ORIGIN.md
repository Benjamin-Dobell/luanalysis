# Fixture provenance

Source: Benjamin Dobell's IntelliJ-Luanalysis, retaining its EmmyLua ancestry.
Repository: https://github.com/Benjamin-Dobell/IntelliJ-Luanalysis
Revision: `d796bb67d5135d152b9f627b69e57377c83d1a66`.
Original repository license: Apache 2.0, reproduced in `LICENSE.txt`.

`local_def_assignment.lua`, `lambda_params.lua` and `trailing_type.lua` are copied byte-for-byte from
their respective `src/test/resources/inspections/` paths. Marker removal is
performed by the tested importer; the expected range is in the cleaned Lua.

The three other files are reduced/adapted positive cases, not whole legacy
fixture passes:

- `number-assignment.lua`: `numbers.lua:1,8`, with immediate initialization and
  another compatible assignment. Calls, arithmetic and literal-only types are omitted.
- `boolean-assignment.lua`: `boolean.lua:1,16,17`, with immediate initialization
  rather than an uninitialized annotated local. Literal-only declarations are omitted.
- `string-assignment.lua`: `string_literals.lua:1,8`, with immediate
  initialization and another compatible string assignment. Literal-only types are omitted.

Their reductions avoid unresolved declaration/inference policies; those whole
fixtures remain separate pending coverage. Corrections of the negative case
are temporary in-memory sources and do not modify its imported bytes.
