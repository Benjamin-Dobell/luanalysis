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
| `--jobs <n>` | Worker threads; by default, one per core. |
| `--watch` | Check again whenever sources or declarations change. |
| `--no-implicit-any` | Require types for parameters and for locals whose type cannot be inferred. |
| `--format text\|json` | Report format. JSON reports include exact source ranges. |

## Editors

```sh
luanalysis lsp --lua-version 5.4
```

runs a language server over stdio, for any editor with a language server client. It accepts the
options of `check`, and without paths it checks the editor's workspace. It provides diagnostics
as you type, hover types, go to definition, find references, rename, completion, signature help
and inspections. Definitions, references and renames follow globals across files, including
`_G.name`. Signature help shows each overload of the call being typed, highlights the argument's
parameter, and selects the overload that the arguments typed so far resolve to.

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

Release archives include the shared library (`libluanalysis.so`, `libluanalysis.dylib` or
`luanalysis.dll`), a static library and the header, for Linux, macOS and Windows on x86-64 and
ARM64.

## Performance

Luanalysis's checker is built on Bun's Rust port of the TypeScript compiler's checker, and it
checks a project's files in parallel.

Checking the 434 Lua files of [lua-language-server](https://github.com/LuaLS/lua-language-server)
3.19.2's own repository, on an Apple M1 Max:

| Checker | Wall time | CPU time |
| --- | --- | --- |
| lua-language-server 3.19.2 `--check` | 40.2 s | 39.3 s |
| Luanalysis, one thread | 2.9 s | 2.6 s |
| Luanalysis, ten threads | 1.6 s | 4.9 s |

In the language server, completions typically answer in under 75 ms.

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
