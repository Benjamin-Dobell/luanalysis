--- LuaJIT's JIT compiler control, the global `jit` (also `require("jit")`), and its `jit.opt`,
--- `jit.profile` and `jit.util` modules.
---@class jitlib
---@field version string The LuaJIT version string.
---@field version_num number The LuaJIT core's version number, xxyyzz for version xx.yy.zz.
---@field os "Windows" | "Linux" | "OSX" | "BSD" | "POSIX" | "Other" The target OS name.
---@field arch string The target architecture name, such as `"x64"` or `"arm64"`.
jit = {}

---
--- Turns the JIT compiler on, or enables JIT compilation for a function: `true` is the calling
--- function. With `recursive` true, its sub-functions too; false, only its sub-functions.
---@overload fun()
---@overload fun(func: function | true, recursive?: boolean)
---@param func? function | true
---@param recursive? boolean
function jit.on(func, recursive) end

---
--- Turns the JIT compiler off, or disables JIT compilation for a function and flushes its
--- compiled code: `true` is the calling function. With `recursive` true, its sub-functions too;
--- false, only its sub-functions.
---@overload fun()
---@overload fun(func: function | true, recursive?: boolean)
---@param func? function | true
---@param recursive? boolean
function jit.off(func, recursive) end

---
--- Flushes the whole cache of compiled code, a function's code (`true` is the calling
--- function), or a root trace by its number and its side traces.
---@overload fun()
---@overload fun(func: function | true, recursive?: boolean)
---@overload fun(trace: integer)
---@param func? function | true | integer
---@param recursive? boolean
function jit.flush(func, recursive) end

---
--- Whether the JIT compiler is on, then the CPU-specific features and optimizations enabled.
---@return boolean, string...
function jit.status() end

---
--- The JIT compiler's optimizations, also `require("jit.opt")`. `start` sets the optimization
--- level (`0` to `3`) or turns optimizations and parameters on and off, as the `-O`
--- command-line option does: `jit.opt.start(2)`, `jit.opt.start("-dce")`,
--- `jit.opt.start("hotloop=10", "hotexit=2")`.
---@class jit.optlib
---@field start fun(...: number | string)
jit.opt = {}

---
--- LuaJIT's sampling profiler, `require("jit.profile")`.
---@class jit.profilelib
local profile = {}

---
--- Starts the profiler with the options in `mode` (`"f"`, `"l"`, `"i<ms>"` and so on), calling
--- `callback` with the thread, the number of samples and the VM state for each sample.
---@param mode string
---@param callback fun(thread: thread, samples: integer, vmstate: string)
function profile.start(mode, callback) end

--- Stops the profiler.
function profile.stop() end

---
--- Dumps the stack of `thread`, or the profiled one, in the format `fmt` to `depth` frames.
---@overload fun(fmt: string, depth: integer): string
---@overload fun(thread: thread, fmt: string, depth: integer): string
---@param thread thread | string
---@param fmt string | integer
---@param depth? integer
---@return string
function profile.dumpstack(thread, fmt, depth) end

---
--- LuaJIT's compiler introspection, `require("jit.util")`, for its own tools. The manual leaves
--- it undocumented while it changes, so each member is a function of anything.
---@class jit.utillib
---@field [string] fun(...: any): any...
