---@class std__Debug
debug = {}

---
--- Enters an interactive mode with the user, running each string that the user
--- enters. Using simple commands and other debug facilities, the user can
--- inspect global and local variables, change their values, evaluate
--- expressions, and so on. A line containing only the word `cont` finishes this
--- function, so that the caller continues its execution.
---
--- Note that commands for `debug.debug` are not lexically nested within any
--- function, and so have no direct access to local variables.
function debug.debug() end

---
--- Returns the current hook settings of the thread, as three values: the
--- current hook function, the current hook mask, and the current hook count
--- (as set by the `debug.sethook` function).
---@overload fun(): (fun(...): any) | nil, string, number
---@overload fun(thread: thread): (fun(...): any) | nil, string, number
---@param thread? thread
---@return (fun(...): any) | nil, string, number
function debug.gethook(thread) end

---@shape std__DebugInfo
---@field activelines nil | {[number]: boolean}
---@field currentline number
---@field func nil | function
---@field istailcall boolean
---@field isvararg boolean
---@field lastlinedefined number
---@field linedefined number
---@field name nil | string
---@field namewhat "global" | "local" | "field" | "method"
---@field nparams number
---@field nups number
---@field short_src string
---@field source string
---@field what "C" | "main" | "lua"

---
--- This function returns a table with information about a function. You can
--- give the function directly, or you can give a number as the value of `f`,
--- which means the function running at level `f` of the call stack of the given
--- thread : Level 0 is the current function (`getinfo` itself); level 1 is the
--- function that called `getinfo` (except for tail calls, which do not count on
--- the stack); and so on. If `f` is a number larger than the number of active
--- functions, then `getinfo` returns **nil**.
---
--- The returned table contains all the fields returned by `lua_getinfo`, with
--- the string `what` describing which fields to fill in. The default for `what`
--- is to get all information available. If present, the option "f" adds a field
--- named `func` with the function itself. If present, the option "L" adds a
--- field named `activelines` with the table of valid lines.
---
--- For instance, the expression `debug.getinfo(1,"n").name` returns the name of
--- the current function, if a reasonable name can be found, and
--- `debug.getinfo(print)` returns a table with all available information about
--- the `print` function.
---@overload fun(f: function | number): std__DebugInfo
---@overload fun(f: function | number, what: string): std__DebugInfo
---@overload fun(thread: thread, f: function | number): std__DebugInfo
---@overload fun(thread:thread,f:function | number,what:string):std__DebugInfo
---@param thread thread | function | number
---@param f? function | number | string
---@param what? string
---@return std__DebugInfo
function debug.getinfo(thread, f, what) end

---
--- This function returns the name and the value of the local variable with
--- index `local` of the function at level `level f` of the stack. This function
--- accesses not only explicit local variables, but also parameters,
--- temporaries, etc.
---
--- The first parameter or local variable has index 1, and so on, following the
--- order that they are declared in the code, counting only the variables that
--- are active in the current scope of the function. Negative indices refer to
--- vararg parameters; -1 is the first vararg parameter. The function returns
--- **nil** if there is no variable with the given index, and raises an error
--- when called with a level out of range. (You can call `debug.getinfo` to
--- check whether the level is valid.)
---
--- Variable names starting with '(' (open parenthesis) represent variables with
--- no known names (internal variables such as loop control variables, and
--- variables from chunks saved without debug information).
---
--- The parameter `f` may also be a function. In that case, `getlocal` returns
--- only the name of function parameters.
---@overload fun(f: number | function, index: number): nil | (string, any)
---@overload fun(thread: thread, f: number | function, index: number): nil | (string, any)
---@param thread thread | number | function
---@param f number | function
---@param index? number
---@return nil | (string, any)
function debug.getlocal(thread, f, index) end

---
--- Returns the metatable of the given `value` or **nil** if it does not have
--- a metatable.
---@param value any
---@return table | nil
function debug.getmetatable(value) end

---
--- Returns the registry table.
---@return table
function debug.getregistry() end

---
--- This function returns the name and the value of the upvalue with index
--- `up` of the function `f`. The function returns **nil** if there is no
--- upvalue with the given index.
---
--- Variable names starting with '(' (open parenthesis) represent variables with
--- no known names (variables from chunks saved without debug information).
---@param f function
---@param up number
---@return nil | (string, any)
function debug.getupvalue(f, up) end

---
--- Returns the `n`-th user value associated to the userdata `u` plus a boolean,
--- **false** if the userdata does not have that value.
---@param u userdata
---@param n number
---@return boolean
function debug.getuservalue(u, n) end

---
--- Sets the given function as a hook. The string `mask` and the number `count`
--- describe when the hook will be called. The string mask may have any
--- combination of the following characters, with the given meaning:
---
--- * `"c"`: the hook is called every time Lua calls a function;
--- * `"r"`: the hook is called every time Lua returns from a function;
--- * `"l"`: the hook is called every time Lua enters a new line of code.
---
--- Moreover, with a `count` different from zero, the hook is called after every
--- `count` instructions.
---
--- When called without arguments, `debug.sethook` turns off the hook.
---
--- When the hook is called, its first parameter is a string describing
--- the event that has triggered its call: `"call"`, (or `"tail
--- call"`), `"return"`, `"line"`, and `"count"`. For line events, the hook also
--- gets the new line number as its second parameter. Inside a hook, you can
--- call `getinfo` with level 2 to get more information about the running
--- function (level 0 is the `getinfo` function, and level 1 is the hook
--- function)
---@overload fun():void
---@overload fun(hook:(fun():any),mask:string):void
---@overload fun(hook:(fun():any),mask:string,count:number):void
---@overload fun(thread:thread,hook:(fun():any),mask:string):void
---@overload fun(thread:thread,hook:(fun():any),mask:string,count:number):void
---@param thread? thread | (fun():any)
---@param hook? (fun():any) | string
---@param mask? string | number
---@param count? number
function debug.sethook(thread, hook, mask, count) end

---
--- This function assigns the value `value` to the local variable with
--- index `local` of the function at level `level` of the stack. The function
--- returns **nil** if there is no local variable with the given index, and
--- raises an error when called with a `level` out of range. (You can call
--- `getinfo` to check whether the level is valid.) Otherwise, it returns the
--- name of the local variable.
---@overload fun(level: number, index: number, value: any): string | nil
---@overload fun(thread: thread, level: number, index: number, value: any): string | nil
---@param thread thread | number
---@param level number
---@param index number | any
---@param value? any
---@return string | nil
function debug.setlocal(thread, level, index, value) end

---
--- Sets the metatable for the given `object` to the given `table` (which
--- can be **nil**). Returns value.
---@generic T
---@param value T
---@param table table | nil
---@return T
function debug.setmetatable(value, table) end

---
--- This function assigns the value `value` to the upvalue with index `up`
--- of the function `f`. The function returns **nil** if there is no upvalue
--- with the given index. Otherwise, it returns the name of the upvalue.
---@param f function
---@param up number
---@param value any
---@return string | nil
function debug.setupvalue(f, up, value) end

--- Sets the given `value` as the `n`-th associated to the given `udata`.
--- `udata` must be a full userdata.
---
--- Returns `udata`, or **nil** if the userdata does not have that value.
---@param udata userdata
---@param value any
---@param n number
---@return userdata
function debug.setuservalue(udata, value, n) end

--- If `message` is present but is neither a string nor **nil**, this function
--- returns `message` without further processing. Otherwise, it returns a string
--- with a traceback of the call stack. The optional `message` string is
--- appended at the beginning of the traceback. An optional `level` number
--- tells at which level to start the traceback (default is 1, the function
--- calling `traceback`).
---@overload fun(message?: string | nil, level?: number): string
---@overload fun(thread: thread, message?: string | nil, level?: number): string
---@overload fun(message: any, level?: number): any
---@param thread? thread | any
---@param message? any
---@param level? number
---@return any
function debug.traceback(thread, message, level) end

--- Returns a unique identifier (as a light userdata) for the upvalue numbered
--- `n` from the given function.
---
--- These unique identifiers allow a program to check whether different
--- closures share upvalues. Lua closures that share an upvalue (that is, that
--- access a same external local variable) will return identical ids for those
--- upvalue indices.
---@param f function
---@param n number
---@return userdata
function debug.upvalueid(f, n) end

---
--- Make the `n1`-th upvalue of the Lua closure f1 refer to the `n2`-th upvalue
--- of the Lua closure f2.
---@param f1 function
---@param n1 number
---@param f2 function
---@param n2 number
function debug.upvaluejoin(f1, n1, f2, n2) end
