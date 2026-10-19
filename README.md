# Luanalysis

Static types for Lua: a type checker, a language server and an embeddable library.

Luanalysis brings a complete static type system to Lua, written in documentation comments, so
your code stays plain Lua. It started life as an
[IntelliJ plugin](https://github.com/Benjamin-Dobell/IntelliJ-Luanalysis), where it pioneered
generics, shapes, literal types, callable types and type-safe casts for Lua. This standalone
version keeps all of that, adds TypeScript-grade control-flow analysis, and runs anywhere: on the
command line, in any editor with a language server client, or inside your own application.

```lua
---@shape Player
---@field name string
---@field score number

---@param players Player[]
---@return Player | nil
local function leader(players)
    local best = nil
    for _, player in ipairs(players) do
        if not best or player.score > best.score then
            best = player
        end
    end
    return best
end

local winner = leader({ { name = "ada", score = 3 } })
print(winner.name) -- error: Property 'name' does not exist on type 'nil | Player'
```

## Contents

* [Type system](#type-system)
* [Flow analysis](#flow-analysis)
* [Standard library](#standard-library)
* [Projects and modules](#projects-and-modules)
* [Command line](#command-line)
* [Editors](#editors)
* [Library](#library)
* [Performance](#performance)
* [Compatibility](#compatibility)
* [Building from source](#building-from-source)

## Type system

### Literal types and unions

String, number and boolean literals are types, and unions combine any types.

```lua
---@alias Direction "north" | "south" | "east" | "west"

---@param direction Direction
local function turn(direction) end

turn("north")
turn("up") -- error: '"up"' is not assignable to '"east" | "north" | "south" | "west"'
```

### Intersections

`A & B` is a value that is each of its types at once; `&` binds more tightly than `|`. As in
TypeScript, a member that one of its types declares has that declaration's type, and only other
names fall to an index signature. So a namespace whose lookup falls back for unknown names
keeps each known member's own type, which one type's index signature cannot, as it must admit
every member's type. In a declaration file:

```lua
---@class ColorStatics
---@field Red ColorValue
---@function ColorStatics.fromString(value: string): ColorValue

---@alias ColorNamespace ColorStatics & { [string]: ColorValue | nil }

---@type ColorNamespace
Color = nil
```

```lua
local red = Color.Red                -- ColorValue
local parse = Color.fromString       -- fun(value: string): ColorValue
local named = Color[playerColor]     -- ColorValue | nil
```

### Shapes and classes

A `@shape` is structural: any table with compatible fields is one. This suits data, such as
JSON, configuration and messages.

```lua
---@shape Point
---@field x number
---@field y number

---@param point Point
local function length(point) return math.sqrt(point.x ^ 2 + point.y ^ 2) end

length({ x = 3, y = 4 })
length({ x = 3, y = "4" }) -- error: not assignable to 'Point'
```

A `@class` is nominal: an object is one by declaration, inheritance included. `self` is a real
type, so a method that returns `self` returns the subclass when called on one.

```lua
---@class Animal
---@field name string
local Animal = {}

---@return self
function Animal:rename() return self end

---@class Dog : Animal
local Dog = {}

---@param dog Dog
local function walk(dog)
    local same = dog:rename() -- Dog
end
```

### Generics

Functions, classes, shapes and aliases take type parameters, which may be constrained, and
generic parameters may refer to one another. Type arguments are inferred at each call.

```lua
---@shape Stack<T>
---@field items T[]

---@generic T
---@param stack Stack<T>
---@param item T
local function push(stack, item) stack.items[#stack.items + 1] = item end

---@type Stack<string>
local names = { items = { "ada" } }
push(names, "grace")
push(names, 42) -- error: '42' is not assignable to 'string'
```

### Indexed fields

Fields are not limited to names. Number literals, booleans and whole types can be keys, so
dictionaries and other data structures are typed precisely.

```lua
---@shape Dictionary<K, V>
---@field [K] V

---@type Dictionary<string, number>
local ages = { ada = 36 }
ages.grace = 85
ages.bob = "old" -- error: '"old"' is not assignable to 'number'
```

Tables can also be written inline (`{ name: string, [number]: boolean }`), and tuples as
`[number, string]`.

### Functions

Function types describe parameters, optional parameters, variadic parameters and multiple or
variadic results: `fun(name: string, greeting?: string, ...: any): boolean, string...`. A `fun`
with only some of that written is partially typed, which suits callbacks and event handlers.

Overloads are declared with `@overload`. As in TypeScript, a function that has overloads is
called through them.

```lua
---@overload fun(point: Point): Point
---@overload fun(x: number, y: number): Point
---@param x number | Point
---@param y? number
---@return Point
local function point(x, y)
    if type(x) == "table" then return x end
    return { x = x, y = y or 0 }
end
```

### Callable types

A table whose metatable has `__call` is described with `@overload` on its class, and calls to
it are checked like calls to a function.

```lua
---@class Counter
---@overload fun(step: number): number
---@field total number
local Counter = setmetatable({ total = 0 }, {
    ---@param self Counter
    ---@param step number
    ---@return number
    __call = function(self, step)
        self.total = self.total + step
        return self.total
    end,
})

local next = Counter(1)   -- number
Counter("one")            -- error: '"one"' is not assignable to 'number'
```

### Operators

A class declares the operators its metatable implements. The other operand's type selects the
declaration, so operators are as type-safe as calls.

```lua
---@class Vector
---@field x number
---@field y number
---@operator add(Vector): Vector
---@operator mul(number): Vector
---@operator unm: Vector
---@operator lt(Vector): boolean

---@param a Vector
---@param b Vector
local function combine(a, b)
    local sum = a + b      -- Vector
    local scaled = 2 * sum -- Vector
    return scaled + "1"    -- error: Operator '+' cannot be applied to types 'Vector' and '"1"'
end
```

### Recursive aliases

Aliases are resolved lazily, so recursive data structures can be typed.

```lua
---@alias JSON nil | boolean | number | string | JSON[] | table<string, JSON>

---@type JSON
local document = { name = "ada", tags = { "x", "y" }, nested = { ok = true } }
```

### Multi-line annotations

An annotation continues on the `---` lines after it until the next tag, so a long union, a
record or a function type can be split over lines, in line comments or a `--[[--- ]]` block.

```lua
---@type {
---    name: string,
---    score: number,
---}
local entry = { name = "ada", score = 3 }

---@param mode
---| "r" # read
---| "w" # write
local function open(mode) end
```

### Casts

When you know more than the checker, say so in a block comment. `@type` replaces an
expression's type; `@not` removes types from a union, which is the safer choice for removing
`nil`. Both accept type lists for multiple results.

```lua
---@return string | nil
local function find() return "x" end

local definite = --[[---@not nil]] find() -- string
```

A `@type` on a `return` statement is a checked annotation, not a cast:

```lua
local function lambda()
    ---@type number
    return "nope" -- error: not assignable to the written return type
end
```

## Flow analysis

Luanalysis's checker is a port of TypeScript's, so types follow your code the way they do in
TypeScript.

### Narrowing

Comparisons, `type()`, `assert`, truthiness and early returns narrow a value's type for the code
they guard.

```lua
---@param value string | number | nil
local function describe(value)
    if value == nil then
        return "nothing"
    elseif type(value) == "string" then
        return value:upper() -- value is a string
    end
    return value + 1         -- value is a number
end
```

Declare a type predicate when a boolean helper tests a parameter's type. The predicate narrows
that argument in both branches, including negation and early returns:

```lua
---@param value string | number
---@return value is string
local function isString(value)
    return type(value) == "string"
end

---@param value string | number
local function describe(value)
    if isString(value) then
        return value:upper() -- string
    end
    return value + 1         -- number
end
```

Predicate functions return booleans. Their target must be assignable to the named parameter's
type. Function types and overloads support the same syntax: `fun(value: unknown): value is string`.
Assertions such as `assert(isString(value))` and comparisons such as `isString(value) == false`
also narrow the argument. The [checked example](examples/features/type-predicates.lua) covers
primitive unions and nominal subclasses.

### Correlated results

Lua functions commonly return a success flag and then either a result or an error. Declare each
outcome, and checking the flag narrows the rest.

```lua
---@return (true, number, nil) | (false, nil, string)
local function parse() return true, 1, nil end

local ok, number, problem = parse()
if ok then
    local doubled = number * 2 -- number
else
    print(problem)             -- string
end
```

### Inferred locals and tables

Locals take the type of what is assigned to them, at each point in the code. A table's fields are
inferred from its constructor and the assignments made to it, so undeclared tables are checked
too.

```lua
local config = { host = "localhost" }
config.port = 8080

---@type number
local port = config.port
local wrong = config.prot -- error: Property 'prot' does not exist
```

### Inspections

In editors, Luanalysis also points out code that type checks but is probably wrong: unreachable
code, conditions that are always true or always false, comparisons that can never match, unused
locals, and parameters without types.

## Standard library

A host that embeds Lua often provides only part of the standard library, as a sandbox does.
`--stdlib` names the parts it provides: whole libraries (`basic`, `coroutine`, `debug`, `io`,
`math`, `os`, `package`, `string`, `table`, `utf8`, `bit32`) or single members (`print`,
`os.time`, `require`). Everything else is undeclared, so using it is an error.

```sh
luanalysis check --lua-version 5.2 --stdlib basic,string,table,math,coroutine,bit32 \
    --stdlib os.time,os.date,os.clock,require src
```

`--no-stdlib` declares none of it, for a host that declares its own with `--library`. `type`,
`assert` and `setmetatable` are always declared: without them, it would not be Lua.

A literal pattern types what `string.match`, `string.gmatch` and `string.find` return: each
capture is a string, and an empty capture, `()`, is a position.

```lua
local key, value = line:match("(%w+)=(%w+)") -- nil | string, nil | string
for word in text:gmatch("%a+") do end      -- word: string
```

## Projects and modules

Luanalysis checks a project as a whole. `require` calls are resolved through module roots and
`package.path` templates, and each module's exports are inferred, so a value keeps its type as it
crosses files. Declaration files (`.def.lua`) describe globals and libraries, and standard library
declarations are included for Lua 5.0 through 5.4.

## Command line

```sh
luanalysis check --lua-version 5.4 src
```

Directories are searched for `.lua` and `.ttslua` files.

| Option | Effect |
| --- | --- |
| `--lua-version 5.0`…`5.4` | Use that Lua version's standard library declarations. |
| `--stdlib <list>` | Declare only these standard libraries and members, as a sandbox provides them (repeatable). |
| `--no-stdlib` | Declare none of the standard library, for a host that declares its own. |
| `--library <path>` | Include declaration files or directories. |
| `--module-root <dir>` | Search this directory for `require`d modules (repeatable, in order). |
| `--package-path <templates>` | `package.path` templates, such as `?.lua;?/init.lua`. |
| `--module-map <manifest.json>` | Resolve `require`s with a host's own module map instead (below). |
| `--jobs <n>` | Worker threads; by default, one per core. |
| `--watch` | Check again whenever sources or declarations change. |
| `--no-implicit-any` | Require types for parameters and for locals whose type cannot be inferred. |
| `--no-strict-nil` | Permit nil assignments for v1 compatibility; preserve runtime nil flow and physical result counts. |
| `--format text\|json` | Report format. JSON reports include exact source ranges. |

A host whose modules are not files that `package.path` finds, such as a game's scripts, can
supply its own module map: each source's modules, by their exact, case-sensitive names.

```json
{
  "version": 1,
  "sources": [
    { "file": "main.lua", "imports": { "Case": "a.lua", "case": "b.lua" } }
  ]
}
```

Paths are relative to the manifest, or absolute, and name existing files. With a map, a
module is what the map names for the importing source, or unresolved: neither `package.path`
nor a `---@meta` declaration provides one. The manifest is read again for each check and each
language-server query, so a host can change it as its scripts change.

## Editors

```sh
luanalysis lsp --lua-version 5.4
```

runs a language server over stdio, for any editor with a language server client. It accepts the
options of `check`, and without paths it checks the editor's workspace. It provides:

* Diagnostics as you type, and inspections: unreachable code, conditions that are always true or
  false, comparisons that can never match, unused names.
* Hover, with a name's type and what its doc comments say, for values and for type names in
  annotations.
* Completion of names, members, and in annotations, tags and types, with documentation. A function
  inserts its call with its required parameters as placeholders. Where a function is expected (an
  argument, a field, an assigned or returned value), its implementation is offered with an empty
  body, as are the literals an expected type allows, and a host's callbacks complete as their
  implementations.
* Signature help, which highlights the parameter being typed and selects the overload that the
  arguments typed so far resolve to. Each overload retains its own description and parameter
  documentation; hover shows distinct documentation beside the corresponding overload.
* Go to definition and type definition, find references, highlights and rename, for locals,
  globals (including `_G.name`), fields and methods (through the receiver's type, its bases and
  each alternative of a union, and a table's fields from its constructor's keys through aliases,
  writes, `t["name"]` and modules), and type names in annotations, across files. Renames
  accept Unicode identifiers using the same rules as Lua source, including UTF-16 editor
  ranges and unsaved buffers. A rename that would change what a name refers to, merge two
  names, or edit a library's declarations is refused, with why. Rename errors include structured
  reasons and related source locations, so embedded editors can map them to authored files;
  see [rename errors](#rename-errors).
* Full-document and range formatting, and newline-triggered indentation, for Lua and `.ttslua`.
* The document outline, with class members and nested functions/locals, workspace symbol search,
  call and type hierarchies, native inferred-type and parameter-name inlays, semantic
  highlighting, folding (including long strings, `--region` and `-- {{{ title`), selection
  ranges, and links for resolved standard-loader `require` calls.
* Quick fixes for misspelled names/members and unused names, plus `refactor.rewrite` actions for
  LuaDoc and inferred annotations, optional parameter annotations, call parentheses, boolean
  inversion, proven literal logical expressions, and assigning method parameters to fields.
* File move/rename reference edits through `workspace/willRenameFiles`, followed by
  `workspace/didRenameFiles`. The editor applies the edits and performs the filesystem operation.

Editors' settings, in `initializationOptions` or `workspace/didChangeConfiguration` (directly or
under `luanalysis`), apply on top of the command line's options: `luaVersion`, `libraries`,
`moduleRoots`, `packagePath`, `stdlib`, `noStdlib`, `noImplicitAny` and `strictNil` (default true), and
`inlayHints.types`, `inlayHints.parameterNames` and `completion.callSnippets` to turn those off.
`inlayHints.expressionParameterNames`, `inlayHints.parameterTypes` and `inlayHints.returnTypes`
opt into wider expression argument names, contextual parameter types and inferred function
results; these three default to false. Explicit annotations and redundant hints are suppressed.
Every workspace folder is checked, and a check in progress is shown as work-done progress.

Formatting reads current unsaved text and respects negotiated UTF-8/UTF-16 positions. It changes
whitespace between native lexer tokens, preserving executable tokens, strings and casts.
Defaults follow authored `ge_tts` style: four spaces, one level for table fields and callback
bodies, compact simple anonymous functions, and existing table interior spacing. Standalone
long prose comments become Lua block comments; long documentation and type annotations wrap
onto `---` continuation lines. Wrapping preserves words, quoted contents, native annotation
structure and attachment, and leaves inline casts, directives and structured comments intact.
Full/range formatting declines invalid Lua; range edits stay within the selection, and wrapping
requires the complete comment token to be selected.
It supports `tabSize` (1–32), `insertSpaces`, `trimTrailingWhitespace`, `insertFinalNewline` and
`trimFinalNewlines`. Extensions in the request's formatting options are `newline: "lf"` or
`"crlf"` (otherwise retain the first line ending) and `keepMultipleStatementsInOneLine: true`
(default false), plus `maxLineLength` (default 120; 0 disables comment wrapping). Width applies
to comments and annotations; unbreakable quoted contents can exceed it. Whitespace inside
strings and existing long comments stays intact even when trimming is enabled.
Only `\n` is advertised for on-type formatting; it changes line indentation, including in
unfinished blocks, and inserts no block-closing tokens. Editors must opt into that trigger.

File-operation capabilities are advertised when the client opts into `workspace.fileOperations`
`willRename`/`didRename`. Edits use current overlays, proven standard-loader identity and resolved
module targets, and validate names against the post-move search order across all module roots.
Local loader shadows and unrelated strings are preserved. Batches and directory moves are
supported; collisions and operations with no safe module name are refused. Open document URIs
and versions are retained; `documentChanges` is used when supported. Module roots/package paths
remain host configuration. Moves affecting `--module-map` manifests require host updates, and
renaming symbolic links is declined. The server never moves files itself.

### Rename errors

`textDocument/prepareRename` and `textDocument/rename` reject unsafe renames with JSON-RPC error
code `-32803` and a readable `message`. Their `error.data` contains `kind: "rename"`, a `reason`
such as `invalidName`, `capture` or `libraryOwned`, and a `relatedInformation` array. Each related
entry has `location.uri`, `location.range`, `message` and compiler `origin` metadata.

Locations refer to the original source before the proposed edit, including unsaved text, and
use the negotiated position encoding (UTF-16 by default). Open documents retain the exact URI
the client supplied. Hosts analyzing generated source can map this URI/range through their
source map without parsing the readable message. Errors without a related location have an
empty `relatedInformation` array.

`origin.kind` is `filesystem` or `declarationLibrary`; `origin.role` is `implementation` or
`declaration`. Bundled declaration libraries also carry their compiler resource `identity`.
This describes compiler provenance; mapping to authored files remains the host's responsibility.

## Library

Luanalysis is also a C library, so applications that host Lua, such as games and editors, can
check scripts and run a language server in their own process, including scripts that exist only
in memory. See [`luanalysis.h`](crates/luanalysis-ffi/include/luanalysis.h).

```c
const char *arguments[] = {"--lua-version", "5.4", "scripts"};
char *report = luanalysis_check(arguments, 3); /* JSON, as `luanalysis check --format json` */
luanalysis_string_free(report);

luanalysis_server *server = luanalysis_server_start(arguments, 3, on_message, user_data);
luanalysis_server_send(server, message, length); /* JSON-RPC from the client */
luanalysis_server_stop(server);
```

Options are those of the command line, so a host passes `--stdlib` or `--no-stdlib` the same
way. Rust callers set them in `ProjectCheckOptions`, and find definitions, references and renames
in a checked project with `SymbolIndex`.

The server delivers its messages to `on_message` on a server thread. From C#, declare the
callback with `[UnmanagedFunctionPointer(CallingConvention.Cdecl)]` and keep the delegate alive
while the server runs.

Each release has two archives for each of Linux, macOS and Windows on x86-64 and ARM64:
`luanalysis-<platform>` holds the command line tool, and `luanalysis-library-<platform>` holds the
shared library (`libluanalysis.so`, `libluanalysis.dylib` or `luanalysis.dll`), a static library
and the header.

To link the static library, also link the system libraries it uses, which the archive lists in
`lib/native-static-libs.txt`: on Linux `-lgcc_s -lutil -lrt -lpthread -lm -ldl -lc`, on macOS
`-liconv`, and on Windows `kernel32.lib ntdll.lib ws2_32.lib advapi32.lib userenv.lib dbghelp.lib
synchronization.lib legacy_stdio_definitions.lib`, compiling with `/MT` and defining
`LUANALYSIS_STATIC`.

## Performance

Luanalysis's checker is built on Bun's Rust port of the TypeScript compiler's checker, and it
checks a project's files in parallel.

Checking 434 Lua files of
[lua-language-server](https://github.com/LuaLS/lua-language-server) 3.19.2's own repository,
using Lua 5.4 on an Apple M1 Max (19 October 2026):

| Checker | Wall time | CPU time |
| --- | --- | --- |
| lua-language-server 3.19.2 `--check` | 18.4 s | 17.9 s |
| Luanalysis, one thread | 2.9 s | 2.9 s |
| Luanalysis, ten threads | 1.4 s | 4.7 s |

These are medians of five fresh-process runs after one warmup, using release builds;
CPU time includes user and system time. Both checkers receive `script/`, `test/`, `tools/`
and the four root Lua files. LuaLS uses its repository configuration with Lua 5.4 and
`lua/` (a symlink to `script/`) and `make/` excluded, so it checks those same 434 files.
The checkers report different diagnostics. Other development work was running on the machine.

Member completions in a 1,546-line Prototype Inertia file, with the buffer changed before
every request and background checking active:

| Completion item details | Median | 95th percentile |
| --- | --- | --- |
| Included in the list | 31 ms | 69 ms |
| Resolved separately by the client | 32 ms | 80 ms |

Each row measures 100 requests across five language-server sessions, excluding each session's
first request. Times include sending the changed buffer and receiving the completion list
over stdio; separately resolved item details are excluded.

## Compatibility

Luanalysis reads the annotations of Luanalysis for IntelliJ and EmmyLua, and the annotation
syntax of lua-language-server, so existing annotated code can be checked as it is.

## Building from source

Luanalysis builds with the nightly Rust toolchain pinned in `rust-toolchain.toml`:

```sh
cargo build --release
cargo test --workspace
```

The tool is `target/release/luanalysis-cli`, named `luanalysis` in release archives, and the
libraries are `target/release/libluanalysis.*` (`luanalysis.dll` on Windows). Release archives are built with `cargo build --profile dist`, which
optimises the whole program and strips symbols, into `target/dist`.

## License

MIT. The vendored checker in `vendor/bun` keeps its own licenses.

Luanalysis by [Benjamin Dobell](https://github.com/Benjamin-Dobell).
