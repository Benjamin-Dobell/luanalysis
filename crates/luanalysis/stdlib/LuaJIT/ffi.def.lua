--- LuaJIT's FFI library, `require("ffi")`: C declarations, C data and calls into C. What C data
--- holds is declared in C (`ffi.cdef`), which Luanalysis does not read, so its fields, elements
--- and arithmetic are `any` (see `cdata`). The names are lua-language-server's.

---
--- A C library namespace: `ffi.C`, or a library `ffi.load` loaded. Indexing it binds the C
--- function or variable of that name, which `ffi.cdef` declared.
---@class ffi.namespace*: userdata
---@field [string] any

---
--- A C type: what `ffi.typeof` returns, and C data itself. Called, it creates C data of that
--- type, as `ffi.new` does.
---@class ffi.ctype*: cdata
---@overload fun(...: any): ffi.cdata*

---@alias ffi.cdata* cdata

--- A C declaration: a type's C text, such as `"struct foo"` or `"int[?]"`.
---@alias ffi.cdecl* string

--- A C type, given as a C declaration, a C type or C data of that type.
---@alias ffi.ct* ffi.ctype* | ffi.cdecl* | ffi.cdata*

---
--- A callback: C data for a Lua function `ffi.cast` converted to a C function pointer.
---@class ffi.cb*: cdata
local cb = {}

--- Frees the callback's resources. The function pointer is then invalid.
function cb:free() end

--- Associates a new Lua function with the callback. The C type and the function pointer stay.
---@param func function
function cb:set(func) end

---@class ffilib
---@field C ffi.namespace* The default C library namespace.
---@field os "Windows" | "Linux" | "OSX" | "BSD" | "POSIX" | "Other" The target OS name, as `jit.os`.
---@field arch string The target architecture name, as `jit.arch`.
local ffi = {}

---
--- Adds C declarations for types or external symbols (named variables or functions). Each
--- `$` in the declaration is replaced by the next parameter, a C type or a name.
---@param def string
---@vararg any
function ffi.cdef(def, ...) end

---
--- Loads the dynamic library `name` and returns its C library namespace. With `global` true,
--- its symbols are loaded into the global namespace, `ffi.C`, too (on POSIX systems).
---@param name string
---@param global? boolean
---@return ffi.namespace*
function ffi.load(name, global) end

---
--- Creates C data of the given type. A variable-length array or struct takes its number of
--- elements, `nelem`, first. The data is initialized with the remaining arguments, or zero.
---@param ct ffi.ct*
---@vararg any
---@return ffi.cdata*
function ffi.new(ct, ...) end

---
--- Creates a C type object for the given type, replacing each `$` in a C declaration with the
--- next parameter. Use it to create C data of a type repeatedly.
---@param ct ffi.ct*
---@vararg any
---@return ffi.ctype*
function ffi.typeof(ct, ...) end

---
--- Creates C data of the given type from `init`, as a C type cast does: a number, a string, C
--- data or a Lua function (a callback, for a C function pointer type).
---@param ct ffi.ct*
---@param init any
---@return ffi.cdata*
function ffi.cast(ct, init) end

---
--- Sets the metatable of a C type (a struct or union) once and for all, and returns it.
---@param ct ffi.ct*
---@param metatable table
---@return ffi.ctype*
function ffi.metatype(ct, metatable) end

---
--- Associates a finalizer with pointer or aggregate C data, called with it when it is
--- collected, and returns it. A `nil` finalizer removes it.
---@param cdata ffi.cdata*
---@param finalizer nil | fun(cdata: ffi.cdata*): any
---@return ffi.cdata*
function ffi.gc(cdata, finalizer) end

---
--- The size of `ct` in bytes, or **nil** if its size is not known (`void`, a function type).
--- A variable-length type takes its number of elements.
---@param ct ffi.ct*
---@param nelem? integer
---@return integer | nil
function ffi.sizeof(ct, nelem) end

---
--- The minimum alignment of `ct` in bytes.
---@param ct ffi.ct*
---@return integer
function ffi.alignof(ct) end

---
--- The offset in bytes of `field` from the start of a struct or union, and for a bit field
--- its bit position and size.
---@param ct ffi.ct*
---@param field string
---@return integer, integer?, integer?
function ffi.offsetof(ct, field) end

---
--- Whether `obj` has the type `ct`. Qualifiers are ignored, and a pointer is of the type of
--- its target struct as well as its own.
---@param ct ffi.ct*
---@param obj any
---@return boolean
function ffi.istype(ct, obj) end

---
--- The `errno` of the last C call that set it, setting it to `newerr` when given.
---@param newerr? integer
---@return integer
function ffi.errno(newerr) end

---
--- A Lua string of the data `ptr` points to: `len` bytes, or without `len`, up to its first
--- zero byte.
---@param ptr any
---@param len? integer
---@return string
function ffi.string(ptr, len) end

---
--- Copies `len` bytes from `src` to `dst`, or a Lua string `src` and its terminating zero byte.
---@overload fun(dst: any, src: any, len: integer)
---@overload fun(dst: any, str: string)
---@param dst any
---@param src any
---@param len? integer
function ffi.copy(dst, src, len) end

---
--- Fills `len` bytes at `dst` with the byte `c`, or zeros.
---@param dst any
---@param len integer
---@param c? integer
function ffi.fill(dst, len, c) end

---
--- Whether the target ABI has the property `param`: `"32bit"`, `"64bit"`, `"le"`, `"be"`,
--- `"fpu"`, `"softfp"`, `"hardfp"`, `"eabi"`, `"win"`, `"pauth"`, `"uwp"` or `"gc64"`.
---@param param string
---@return boolean
function ffi.abi(param) end

return ffi
