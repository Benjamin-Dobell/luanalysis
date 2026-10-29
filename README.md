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

### Template literal types

A template literal type, as in TypeScript, is the strings with its text around each `${…}`
type. A union in a placeholder gives each combination, and a call can infer a type parameter
from the text a string has:

```lua
---@alias Side "left" | "right"
---@alias Edge `${Side}_${"top" | "bottom"}` -- "left_top" | "left_bottom" | ...

---@type `on_${string}`
local event = "click" -- error: '"click"' is not assignable to '`on_${string}`'

---@generic T: string
---@param event `on_${T}`
---@return { name: T }
local function handler(event) return { name = --[[---@type T]] event:sub(4) } end
local handled = handler("on_click") -- { name: "click" }
```

`Uppercase<S>`, `Lowercase<S>`, `Capitalize<S>` and `Uncapitalize<S>` map a string type's
letters. A placeholder is a string, number, boolean or `nil` type, and a backslash escapes the
character after it. A backtick type without `${…}` is still a completion hint (v1).

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

A member may be `private` or `protected`, as in TypeScript, and LuaLS's `package` is visible in its
own file. A class's body is its methods, the functions stored on it, and the file that declares
the member, whose statements build the class. A `protected` member is also visible in a
subclass's methods.

```lua
-- account.lua
---@class Account
---@field private balance number
---@field protected owner string
local Account = {}

---@private
function Account.audit() end

return Account
```

```lua
-- bank.lua
local Account = require("account")

---@param account Account
local function steal(account)
    return account.balance -- error: 'balance' is private and only accessible within class 'Account'
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

When several arguments infer one type parameter, the candidates combine as in TypeScript.
Table literals, whether written in the call or built into a local, are unioned, as are
literals of one primitive type. Otherwise a candidate that is a supertype of the others is
the type, or else the first candidate, and an argument that it does not accept is an error:

```lua
---@generic T
---@param a T
---@param b T
---@return T
local function both(a, b) return a end

local tables = both({ a = 1 }, { b = 2 }) -- { a: 1 } | { b: 2 }
local counts = both(1, 2)                 -- number
local pet = both(dog, animal)             -- Animal, given a Dog and an Animal
both(1, "x") -- error: '"x"' is not assignable to '1'
```

A type parameter may have a default, after its constraint, as in TypeScript and EmmyLua Analyzer.
An omitted type argument is its default, and so is one that a call's arguments do not infer:

```lua
---@shape Box<T = string>
---@field value T

---@type Box -- Box<string>
local label = { value = "ready" }

---@generic T = string
---@param fallback? T
---@return T
local function pick(fallback) return --[[---@type T]] fallback end
local name = pick()   -- string
local count = pick(3) -- number
```

A constrained parameter's default follows its constraint, `---@generic T: Constraint = Default`.
`extends` may stand for the colon, as in TypeScript and EmmyLua Analyzer: `---@generic K extends string`,
`---@alias Result<F extends function>`.
As TypeScript checks them, parameters with defaults follow those without, a default refers only
to earlier parameters of its list, and it satisfies its constraint. A union default is
parenthesized, as a union constraint is: `---@generic T = (string | nil)`.

A call may give its type arguments, as TypeScript's `id<string>(x)` does, in EmmyLua Analyzer's
comment just before its arguments: `id--[[@<string>]](x)`, `box:put--[[@<number>]](1)`.

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

### Read-only fields

A field declared `readonly`, with `@field` or inline, can be read but not assigned.

```lua
---@shape Job
---@field readonly path string
---@field retries number

---@param job Job
---@param options { readonly verbose: boolean }
local function retry(job, options)
    job.retries = job.retries + 1
    job.path = "other" -- error: Cannot assign to readonly property "path"
    options.verbose = true -- error: Cannot assign to readonly property "verbose"
end
```

`readonly` comes before a field's name, so `---@field readonly boolean` and
`{ readonly: boolean }` still declare a field named `readonly`. Editors point this out;
`["readonly"]` names that field explicitly, and `readonly readonly` makes it read-only.

A field written `name?: T` in a table type may be absent, as in TypeScript and EmmyLua Analyzer:
`{ name?: string }` accepts `{}`, and reading `name` gives `nil | string`.

### Types from values

`typeof` gives the declared type of a value: a local or a global, or a member of one
(`typeof M.config`). `keyof T` is the union of a table type's keys, and `T[K]` the type of its
values at keys of type `K`. Together they derive a type from a table instead of repeating it:

```lua
---@alias Color typeof Colors[keyof typeof Colors]
local Colors = { Red = 1, Green = 2, Blue = 3 }

---@param color Color
local function paint(color) end

paint(Colors.Red)
paint(4) -- error: '4' is not assignable to '1 | 2 | 3'
```

Keys keep their Lua types: `keyof` of `{ [1]: string, active: boolean }` is `1 | "active"`, of
`string[]` is `number` and of `table<string, V>` is `string`. Generic functions can be typed by
key:

```lua
---@generic T, K: keyof T
---@param object T
---@param key K
---@return T[K]
local function get(object, key) return object[key] end

---@type Point
local point = { x = 3, y = 4 }
local x = get(point, "x") -- number
get(point, "z")           -- error: '"z"' is not assignable to '"x" | "y"'
```

`typeof` names the variable in scope where it is written. A type declaration attached to a
statement, as the alias above is, also sees what that statement declares. As in TypeScript, it is
the variable's declared type, not one narrowed where the annotation is.

lua-language-server's `---@enum Name` above a table is this alias, `typeof X[keyof typeof X]` for
the table `X` it assigns, and `---@enum (key) Name` is `keyof typeof X`.

### Conditional types

`T extends U and X or Y` is EmmyLua Analyzer's spelling of TypeScript's `T extends U ? X : Y`:
`X` where `T` is assignable to `U`, otherwise `Y`. A conditional type in the false branch chains,
`infer P` in the extends type declares a type parameter that the true branch sees, and a naked
type parameter distributes the test over a union:

```lua
---@alias ElementOf<T> T extends (infer U)[] and U or never
---@alias NonNil<T> T extends nil and never or T
---@alias Kind<T> T extends string and "text" or T extends number and "number" or "other"
---@alias Flatten<T> T extends (infer U)[] and Flatten<U> or T

---@type ElementOf<string[]>
local first = "ada"     -- string
---@type NonNil<string | nil>
local present = nil     -- error: 'nil' is not assignable to 'string'
---@type Kind<true>
local kind = "other"
---@type Flatten<number[][]>
local deep = 1          -- number

---@type fun<T>(value: T): (T extends string and number or boolean)
local measure
local length = measure("ada") -- number
```

As in TypeScript, the type is deferred while its check or extends type is generic, a branch may
refer to the alias whose body it is, and recursion that never ends is reported (TS2589).
`infer P extends C` constrains what is inferred. A `---@return` may be conditional too; without
`and` after its extends type, `extends` begins the description (v1).

### Mapped types

`{ [P in K]: T }` is a table with a property for each key `P` of `K`, of type `T`, as in
TypeScript and EmmyLua Analyzer. Over `keyof X` it keeps `X`'s optional and read-only
properties; `?`, `readonly`, `-?` and `-readonly` add or remove them, and `as` renames each key:

```lua
---@class Point
---@field x number
---@field y number

---@alias Optional<T> { [P in keyof T]?: T[P] }
---@alias Getters<T> { [P in keyof T as `get_${P & string}`]: fun(): T[P] }
---@alias DeepPartial<T> { [P in keyof T]?: DeepPartial<T[P]> }

---@type Optional<Point>
local partial = { x = 1 }   -- { x?: number, y?: number }
---@type Getters<Point>
local getters
local x = getters.get_x()   -- number
```

The template may refer to the alias whose body it is, as `DeepPartial` does. A Lua table has an
absent property as `nil`, so a property whose type admits `nil` need not be there. Inference
goes through a mapped type, as TypeScript's does: a function taking `Partial<T>` infers `T`.

TypeScript's utility types are declared with every version of Lua: `Partial`, `Required`,
`Readonly`, `Pick`, `Omit`, `Record`, `Exclude`, `Extract`, `NonNullable`, `ReturnType` and
`Parameters`. A
project's own declaration of one of those names is the type of that name. `NoInfer<T>` is `T`
where inference does not look for a type parameter:

```lua
---@generic T: string
---@param values T[]
---@param default NoInfer<T>
---@return T
local function pick(values, default) return default end
pick({ "a", "b" }, "c") -- error: '"c"' is not assignable to '"a" | "b"'
```

### Functions

Function types describe parameters, optional parameters, variadic parameters and multiple or
variadic results: `fun(name: string, greeting?: string, ...: any): boolean, string...`. A `fun`
with only some of that written is partially typed, which suits callbacks and event handlers.

An optional is a value that may be absent. It is not nil, which is a value: Lua tells them
apart, as `select("#", ...)` does.

- A parameter or field writes it on its name: `---@param name? T`, `fun(name?: T)`,
  `---@field name? T`, `{ name?: T }`. A caller may leave an optional parameter out, but an
  argument for it is a `T`, so `f(nil)` is an error unless `T` admits nil, as in Luanalysis for
  IntelliJ. Within its function, the parameter may be nil.
- A result or a tuple element has no name, so it is written `T?`. `---@return number?` is a
  `number` or no result: a bare `return`, or reaching the end of the function, satisfies it,
  and `return nil` does not. `---@return string?, boolean?` is
  `void | string | (string, boolean)`, and `[string, number?]` may lack its second element. A
  required result or element cannot follow an optional one.
- `nil | T` is a value, which the function must return: passing on a `T?` result where
  `nil | T` is declared is an error. Only a caller's local reads a missing result as nil, as Lua
  assigns it.

Elsewhere, `T?` is EmmyLua Analyzer syntax, which [Compatibility](#compatibility) describes.

Varargs written `...: T...` are a sequence of values, `T`, rather than each a `T`: TypeScript's
`...args: T` for a tuple `T`, in EmmyLua Analyzer's spelling. A call infers the sequence from its
arguments, or from a callback's parameter list, and `T` named elsewhere is that sequence. A tuple
spreads the same way, `fun(...: [number, string?]...)`.

```lua
---@generic A, R
---@param f fun(...: A...): R
---@param ... A...
---@return R
local function call(f, ...) return f(...) end

call(string.rep, "a", 3)   -- string
call(string.rep, "a", "b") -- error: '"b"' is not assignable to 'number'

---@generic T
---@param ... T...
---@return fun(...: T...)
local function bind(...) return function(...) end end
local bound = bind(1, "a") -- fun(...: (number, string))
```

`pcall` and `xpcall` check the arguments they pass on this way.

Overloads are declared with `@overload`. As in TypeScript, a function that has overloads is
called through them. A function that returns values may be passed where `fun: void` is expected,
as in TypeScript, but a call prefers an overload that takes those results to a `fun: void` one,
as in Luanalysis for IntelliJ.

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

A field written again as another function type declares an overload, as lua-language-server and
EmmyLua Analyzer read it, so overloads need no implementation. They are tried in the order they
are written, as an intersection's function types are, and as TypeScript tries an interface's.
Another declaration of the class, or a `---@field Codec.encode fun(...)` in another file, adds
overloads after them, in the order the declarations are.

```lua
---@class Codec
---@field encode fun(value: string): string
---@field encode fun(value: number): string
---@field encode fun(value: string, width: integer): string
```

A field declared again with another type is reported there, and the first declaration is used
(TypeScript's TS2717, and TS2687 for other modifiers). A field written again unchanged is the same
field.

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

An assertion function returns only when its argument has a type, or, with `asserts value` alone,
when it is truthy. After a call, the argument has that type:

```lua
---@param value any
---@return asserts value is string
local function checkString(value)
    if type(value) ~= "string" then error("expected a string") end
end

---@param value string | number
local function shout(value)
    checkString(value)
    return value:upper() -- string
end
```

As with a function that returns `never`, a call has this effect only through written types: a
documented function, or a variable, parameter or field declared with a type. A class's assertion
methods assert when called on a typed instance, not on the class's own table.

Unlike TypeScript, narrowing ends at a call that can visibly change the value: a call to a
function defined in the same file that assigns the variable or field, directly or through the
functions it calls, or a call given such a function as an argument. The value then has its
declared type. Other calls keep the narrowing.

```lua
local ready = false
local function enable() ready = true end
enable()
if ready then print("ready") end -- boolean, not false
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

A generic that a callback's results infer is all of those results, in every outcome that names it,
as in Luanalysis for IntelliJ. `---@return string | T` is a string or all of the callback's results.

```lua
---@generic T
---@param f fun(): T
---@return (true, T) | (false, string)
local function try(f) return true, f() end

---@return number, boolean
local function two() return 1, true end

local ok, value, flag = try(two)
if ok then
    print(value, flag) -- number, boolean
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

A field written as a literal has that literal's type, as in Luanalysis 1: `{ ready = false }`
has a field of type `false`, and assigning `true` to it is an error. When such a table is passed
to a parameter, or given to a variable, whose declared type has a wider field that can be
assigned, such as `{ ready: boolean }`, the field has that wider type everywhere, because that
code may change it. A parameter without a type widens the fields its function assigns. A
`readonly` field cannot be assigned, so it widens nothing.

```lua
local state = { ready = false }

---@param s { ready: boolean }
local function enable(s) s.ready = true end

enable(state)
if state.ready then print("ready") end -- state.ready is boolean
```

A loop's variables take the values its iterator gives. A `---@param name T` line above a
`for ... in` loop gives its variable `name` the type `T` instead, as Luanalysis for IntelliJ and
lua-language-server read it, and a `---@type T` line does the same for a numeric loop's variable.
Each value the loop gives it is checked against that type, as a typed local's value is.

```lua
---@param item Item
for _, item in pairs(registry) do
    print(item.name)
end
```

### Inspections

In editors, Luanalysis also points out code that type checks but is probably wrong: unreachable
code, conditions that are always true or always false, comparisons that can never match, unused
locals, locals and generic parameters that shadow others of the same name, and parameters
without types.

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
crosses files. Standard library declarations are included for Lua 5.0 through 5.4.

A file named `*.def.lua` or `*.def.ttslua` is a declaration file wherever it is found, as
TypeScript's `*.d.ts` files are. Its stubs describe what other code provides, so they are not
checked as code that runs: `local Hand` is the class `---@class Hand` declares, not `nil`, and
`function Hand.getValue() end` declares a function without returning anything. A namespace
nests through its parents' types, so `---@class ngx__req` on `ngx.req = {}` makes
`function ngx.req.read_body() end` a member of `ngx__req`. `--library` makes every file in a
path a declaration file, whatever its name.

A function a declaration file defines again declares another overload, as a `.d.ts` file's
function declarations do, so each overload keeps its own `---@param` and `---@return`
descriptions. A global's definitions in several declaration files are its overloads too; the
standard library's are not, and another declaration of one of its globals is checked against
it. A definition's `---@overload`s are its own, and its head is their implementation signature,
as in an implementation.

```lua
---@param text string the text to show
---@return integer
function notify(text) end

---@param text string the text to show
---@param color Color the text's color
---@return integer
function notify(text, color) end
```

A module that no module root provides is, as in Luanalysis v1 and LuaLS, the checked or library
file whose path ends with the module's path under a template, the shortest such path first: with
`src/` and `lib/` checked, `require("ge_tts.TableUtils")` is `lib/ge_tts/TableUtils.ttslua`.
`--path-strict` (or LuaLS's `runtime.pathStrict`) leaves it unresolved instead.

A checked directory's LuaLS configuration, `.luarc.json` (or `.luarc.jsonc`), applies where
options do not decide, in its flat (`"runtime.version"`) or nested form:

| Setting | Effect |
| --- | --- |
| `runtime.version` | `"Lua 5.1"` to `"Lua 5.4"`, as `--lua-version`; `"Lua 5.5"` uses 5.4's declarations, `"LuaJIT"` 5.1's. |
| `runtime.path` | `package.path` templates, relative to the directory; `.lua` templates also find `.ttslua` files. |
| `runtime.pathStrict` | Find modules only through templates, as `--path-strict`. |
| `workspace.library` | Declaration libraries, as `--library`. LuaLS's `${…}` paths are its own. |
| `workspace.ignoreDir` | Gitignore patterns for what directory discovery skips; by default `.vscode`. |
| `workspace.useGitIgnore` | Skip what `.gitignore` files and `.git/info/exclude` ignore (default true). |

Other settings are LuaLS's own. A path named explicitly is checked even where discovery skips it.

## Command line

```sh
luanalysis check --lua-version 5.4 src
```

Directories are searched for `.lua` and `.ttslua` files, skipping `.vscode` and what `.gitignore`
files ignore unless a `.luarc.json` says otherwise.

| Option | Effect |
| --- | --- |
| `--lua-version 5.0`…`5.4` | Use that Lua version's standard library declarations. |
| `--stdlib <list>` | Declare only these standard libraries and members, as a sandbox provides them (repeatable). |
| `--no-stdlib` | Declare none of the standard library, for a host that declares its own. |
| `--library <path>` | Include declaration files or directories, whatever their names. |
| `--module-root <dir>` | Search this directory for `require`d modules (repeatable, in order). |
| `--package-path <templates>` | `package.path` templates, such as `?.lua;?/init.lua`. |
| `--path-strict` | Leave a module that no module root provides unresolved, instead of finding it by its path. |
| `--module-map <manifest.json>` | Resolve `require`s with a host's own module map instead (below). |
| `--jobs <n>` | Worker threads; by default, one per core. |
| `--watch` | Check again whenever sources or declarations change. |
| `--no-implicit-any` | Require types for parameters and for locals whose type cannot be inferred. |
| `--no-unsafe-any` | Report reading a member of, indexing or calling a value of type `any`, as Luanalysis for IntelliJ did with "unknown callable/indexable" off. `any` stays assignable either way. |
| `--emmylua-syntax` | Read EmmyLua Analyzer's own annotation syntax (see [Compatibility](#compatibility)). |
| `--dangerously-decay-optionals` | Read absence as nil, as EmmyLua Analyzer does: every optional is `nil \| T`, and a final result or parameter that admits nil may be left out. |
| `--no-strict-nil` | Permit nil assignments for v1 compatibility; preserve runtime nil flow and physical result counts. |
| `--format text\|json` | Report format. JSON reports include exact source ranges. |

`luanalysis --version` prints the version and the commit it was built from, such as
`0.1.0+b2cff754f241` (`.dirty` for a build from changed sources). The language server reports the
same version in its `serverInfo`, and the library in `luanalysis_version()`.

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
  false, comparisons that can never match, unused names. After an edit, the server checks
  again only the edited files, as TypeScript's builder does. Then it checks the files that use
  something an edit changed: a module's results, a global, or a type and its members. It keeps
  going while what those files expose changes too. Every other file keeps its diagnostics,
  which stay correct.
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

Every workspace folder is checked, and a check in progress is shown as work-done progress.

### Settings

An editor configures the server with `initializationOptions` in its `initialize` request and
with `settings` in `workspace/didChangeConfiguration`, either directly or under a `luanalysis` key:

```json
{
  "luanalysis": {
    "luaVersion": "5.2",
    "libraries": ["lib/tts-types"],
    "inlayHints": { "parameterTypes": true }
  }
}
```

| Setting | Type | Default | Command line | `.luarc.json` | Effect |
| --- | --- | --- | --- | --- | --- |
| `luaVersion` | `"5.0"` to `"5.4"` | v1's compatible declarations | `--lua-version` | `runtime.version` | The Lua version whose standard library is declared. |
| `libraries` | paths | none | `--library` | `workspace.library` | Declaration files or directories, added to the others. |
| `moduleRoots` | paths | the workspace folders | `--module-root` | | Directories that `require` searches in order, added to the others. |
| `packagePath` | string | `?.lua;?/init.lua;?.ttslua;?/init.ttslua` | `--package-path` | `runtime.path` | `package.path` templates. |
| `pathStrict` | boolean | `false` | `--path-strict` | `runtime.pathStrict` | Leave a module that no module root provides unresolved, instead of finding it by its path. |
| `stdlib` | names | all of it | `--stdlib` | | Declare only these standard libraries and members, as a sandbox provides them. |
| `noStdlib` | boolean | `false` | `--no-stdlib` | | Declare none of the standard library, for a host that declares its own. |
| `noImplicitAny` | boolean | `false` | `--no-implicit-any` | | Require types for parameters and for locals whose type cannot be inferred. |
| `noUnsafeAny` | boolean | `false` | `--no-unsafe-any` | | Report reading a member of, indexing or calling a value of type `any`. |
| `emmyluaSyntax` | boolean | `false` | `--emmylua-syntax` | | Read EmmyLua Analyzer's own annotation syntax. |
| `dangerouslyDecayOptionals` | boolean | `false` | `--dangerously-decay-optionals` | | Read absence as nil, as EmmyLua Analyzer does. |
| `strictNil` | boolean | `true` | `--no-strict-nil` (`false`) | | `false` permits nil assignments, for v1 compatibility. |
| `inlayHints.types` | boolean | `true` | | | Show the inferred types of locals declared without one. |
| `inlayHints.parameterNames` | boolean | `true` | | | Show parameter names at literal arguments: `nil`, booleans, numbers and strings. |
| `inlayHints.expressionParameterNames` | boolean | `false` | | | Show parameter names at expression, table and function arguments too. |
| `inlayHints.parameterTypes` | boolean | `false` | | | Show the contextual types of unannotated parameters. |
| `inlayHints.returnTypes` | boolean | `false` | | | Show the inferred results of functions without `@return`. |
| `completion.callSnippets` | boolean | `true` | | | Complete a function as its call, with its required parameters as placeholders. |

Paths are a JSON array or a comma-separated string, and a relative path is the first workspace
folder's. `stdlib` takes the names `--stdlib` does; with `stdlib` or `noStdlib` and no Lua
version, the version is 5.4. Hints never repeat a written annotation or what the code already
says.

Each layer overrides the one before: the defaults, then the workspace's
[`.luarc.json`](#projects-and-modules), then the command line's options, then the editor's
settings. `libraries` and `moduleRoots` add to the paths before them instead of replacing them.
Every `workspace/didChangeConfiguration` replaces the editor's earlier settings,
`initializationOptions` included, so a setting it leaves out returns to the command line's or
`.luarc.json`'s. A changed `.luarc.json` applies when the editor reports it saved or changed. An
invalid setting is reported in the editor's log (`window/logMessage`), and the server keeps the
settings it had.

Formatting reads current unsaved text and respects negotiated UTF-8/UTF-16 positions. It changes
whitespace between native lexer tokens, preserving executable tokens, strings and casts.
Defaults follow authored `ge_tts` style: four spaces, one level for table fields and callback
bodies, compact simple anonymous functions, and existing table interior spacing. Standalone
long prose comments become Lua block comments; long documentation and type annotations wrap
onto `---` continuation lines. Wrapping preserves words, quoted contents, native annotation
structure and attachment, and leaves inline casts, directives and structured comments intact.
In every annotation, inline casts included, a union's written `nil` moves first:
`string|nil` becomes `nil|string`. Each member keeps its text and the union its separators,
and a union written across `---|` lines keeps its order. Hovers and messages print a union in
the order it is written. Full/range formatting declines invalid Lua; range edits stay within the selection, and wrapping
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

Diagnostics after an edit, from the change to the new diagnostics, are measured by
`docs/emmylua-comparison/bench/lsp_bench.py`. Each edit adds or removes a type error at the end
of the file, and the figures are medians of ten edits (29 October 2026):

| Project, edited file | Whole check | Edit to diagnostics |
| --- | --- | --- |
| lua-language-server, `script/files.lua` (999 lines) | 1.5 s | 0.42 s |
| Prototype Inertia, `src/model/CardBehavior.ttslua` (2,522 lines) | 6.1 s | 1.33 s |

An edit that changes what a widely used module or class exposes checks its users as well, and
costs up to one whole check.

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

`---@readonly` above a local, as EmmyLua Analyzer writes it, makes its table's fields read-only.
With `--emmylua-syntax` (the `emmyluaSyntax` setting), Luanalysis also reads EmmyLua Analyzer's
other additions:

- `A - B`, `A` without the members of `B` (`Exclude<A, B>`); `T - ?` is `T` without nil.
- `sync fun(...)`, an ordinary function type, as `async fun(...)` always is.
- `---@return_overload true, T` lines, result rows that a call returns one of.
- Signature tags (`---@param`, `---@return`, `---@overload`) above a local whose value is no
  function, which declare its type.
- `T?` as the type of a parameter, field or local, or within another type, where Luanalysis
  writes the `?` on a name or writes `nil | T`. Without the option it is reported. With it, a
  parameter's or field's `T?` is its name's, optional, and any other is `nil | T`.
- EmmyLua Analyzer's builtin types: `int`; `type`, `collectgarbage_opt` and `metatable`;
  `std.NotNull<T>` (`T` without nil), `std.Nullable<T>`, `std.RawGet<T, K>` (`T[K]`),
  `Language<T>` (a string), `ConstructorParameters<T>` and `new fun(...)`, a class's call
  signature. `---@return TypeGuard<T>` is the predicate that the first parameter is a `T`.
- `---@interface`, its `---@class`; `---@return_cast x T`, the predicate `x is T`; and
  `---@language`, `---@mapping`, `---@schema` and `---@[attribute]` lines, which type nothing.
- `---@generic const T`: literal arguments keep their literal types, as TypeScript's
  `<const T>`.
- Several class supers, including table types (`---@class A: B, {[string]: number}`), as
  lua-language-server reads them too. A class without fields whose super is a primitive
  (`---@class Key: integer`) is a name for that primitive, assignable to and from it.
- Several `---@class` tags in one comment: the one named as the variable the statement
  assigns, or else the first, is that variable's class, and the others are declared alone.
- `---@cast` of a field path (`---@cast obj.value +boolean`: a store of a `boolean` there is
  accepted to the end of the block); `--[[@cast -?]]` without a name, of the expression before
  it, also between a call and its next suffix (`a:f() --[[@cast -?]]:g()`), as `--[[@as T]]`
  there; and a trailing `expression ---@as T`.
- `---@diagnostic disable-next-line: assign-type-mismatch` and its kin, which disable
  diagnostics by Luanalysis's code or lua-language-server's name.
- Lua 5.5's `global` declarations and LuaJIT's numerals (`1LL`, `0x10ULL`, `0b101`, `3i`).

Without the option each of these is reported as EmmyLua Analyzer's syntax, except
`---@diagnostic`, which lua-language-server also writes and Luanalysis then leaves to the editor.

`---@namespace Game`, as EmmyLua Analyzer writes it, needs no option either: the file's named
types are `Game.Unit` and so on, whichever line it is on, and a name the file writes is looked up
as `Game.Name` first. `---@using Game` looks a file's names up there too.

A string-template generic, lua-language-server's ``---@param class `T` `` with `---@generic T`,
needs no option: a call's string names the declared type `T` is, `new("Unit")` a `Unit`.
`---@module "name"` above a local makes it that module, as `require("name")` would. And
`---@cast x +T` widens what the variable holds to `T` as well, to the end of its block.

EmmyLua Analyzer and lua-language-server read every optional as `nil | T` and a missing value as
nil, conflating a value that may be absent with nil. `--dangerously-decay-optionals` (the
`dangerouslyDecayOptionals` setting) reads them that way, absence as nil:

- A `T?` is `nil | T`, so `---@return string?, boolean` is `nil | string, boolean`.
- A final result that admits nil may be left out: a bare `return`, or none, satisfies
  `---@return string?` and `---@return string | nil`, and `function() end` is a
  `fun(): string?`.
- A final parameter that admits nil may be omitted, as an optional parameter may, and an optional
  parameter takes nil: `f()` calls `---@param x string | nil`.

It is separate from `--emmylua-syntax`, which does not enable it, because it is incorrect.

Some checks deliberately differ from Luanalysis for IntelliJ's. Stricter, as TypeScript's are:

- A `private`, `protected` or `package` member is accessed only where it is visible. Luanalysis
  for IntelliJ used a member's visibility only to choose completions. See
  [Shapes and classes](#shapes-and-classes).
- A type parameter inferred from several unrelated arguments is not their union, except for
  table literals and literals of one primitive type. `both(1, "x")` is an error, where
  Luanalysis for IntelliJ inferred `1 | "x"`. See [Generics](#generics).
- A function's inferred result widens its literals, as TypeScript's does:
  `function() return 1 end` returns `number`, where Luanalysis for IntelliJ inferred `1`.

More permissive:

- A `table<K, V>` may be a base type, as TypeScript's `Record<K, V>` may: `---@class Strings:
  table<integer, string>`. Luanalysis for IntelliJ reported it as inheriting a primitive type.
- Assigning `nil` to an element of a `T[]` deletes it, as Lua deletes a dictionary's entry.
- A dictionary type is a view of the keys it names, as TypeScript's
  `Partial<Record<"a", V>>` is: a `table<string, V>` is a `table<"a", V>`.
- A generic parameter may shadow one of an enclosing function or class, as in TypeScript; an
  editor's inspections point it out.

And as lua-language-server reads them, more dashes than `---` comment an annotation out:
`----@type T` is a plain comment.

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
