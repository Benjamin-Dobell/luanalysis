---@class std__Table
table = {}

---
--- Given a list where all elements are strings or numbers, returns the string
--- `list[i]..sep..list[i+1] ... sep..list[j]`. The default value for
--- `sep` is the empty string, the default for `i` is 1, and the default for
--- `j` is #list. If `i` is greater than `j`, returns the empty string.
---@overload fun(list:string[]|number[]|(string|number)[]):string
---@overload fun(list:string[]|number[]|(string|number)[], sep:string):string
---@overload fun(list:string[]|number[]|(string|number)[], sep:string, i:number):string
---@overload fun(list:string[]|number[]|(string|number)[],sep:string,i:number,j:number):string
---@param list string[]|number[]|(string|number)[]
---@param sep? string
---@param i? number
---@param j? number
---@return string
function table.concat(list, sep, i, j) end

---
--- Inserts element `value` at position `pos` in `list`, shifting up the
--- elements to `list[pos]`, `list[pos+1]`, `···`, `list[#list]`. The default
--- value for `pos` is ``#list+1`, so that a call `table.insert(t,x)`` inserts
--- `x` at the end of list `t`.
---@overload fun<T>(list:T[], value:T):void
---@overload fun<T>(list:T[],pos:number,value:T):void
---@generic T
---@param list T[]
---@param pos number | T
---@param value? T
function table.insert(list, pos, value) end

--- Returns the largest positive numerical index of the given table, or zero
--- if the table has no positive numerical indices. (To do its job this function
--- does a linear traversal of the whole table.)
---@param table table
---@return number
function table.maxn(table) end

---
--- Moves elements from table a1 to table `a2`, performing the equivalent to
--- the following multiple assignment: `a2[t]`,`··· = a1[f]`,`···,a1[e]`. The
--- default for `a2` is `a1`. The destination range can overlap with the source
--- range. The number of elements to be moved must fit in a Lua integer.
---
--- Returns the destination table `a2`.
---@overload fun(a1:table, f:number, e:number, t:number):table
---@overload fun(a1:table,f:number,e:number,t:number,a2:table):table
---@param a1 table
---@param f number
---@param e number
---@param t number
---@param a2? table
---@return table
function table.move(a1, f, e, t, a2) end

---
--- Creates a table with room for `narray` array elements and `nhash` other
--- keys, as the C API's `lua_createtable` does. LuaJIT defines it once
--- `require("table.new")` has run, which returns it too.
---@param narray integer
---@param nhash integer
---@return table
function table.new(narray, nhash) end

---
--- Removes every key and value from `tab`, keeping its allocated sizes. LuaJIT
--- defines it once `require("table.clear")` has run, which returns it too.
---@param tab table
function table.clear(tab) end

---
--- Removes from `list` the element at position `pos`, returning the value of
--- the removed element. When `pos` is an integer between 1 and `#list`, it
--- shifts down the elements `list[pos+1]`, `list[pos+2]`, `···`,
--- `list[#list]` and erases element `list[#list]`; The index pos can also be 0
--- when `#list` is 0, or `#list` + 1; in those cases, the function erases
--- the element `list[pos]`.
---
--- The default value for `pos` is `#list`, so that a call `table.remove(l)`
--- removes the last element of list `l`.
---@overload fun<V>(list:V[]):V|nil
---@overload fun<V>(list:V[],pos:number):V|nil
---@generic V
---@param list V[]
---@param pos? number
---@return V|nil
function table.remove(list, pos) end

---
--- Sorts list elements in a given order, *in-place*, from `list[1]` to
--- `list[#list]`. If `comp` is given, then it must be a function that receives
--- two list elements and returns true when the first element must come before
--- the second in the final order (so that, after the sort, `i < j` implies not
--- `comp(list[j],list[i]))`. If `comp` is not given, then the standard Lua
--- operator `<` is used instead.
---
--- Note that the `comp` function must define a strict partial order over the
--- elements in the list; that is, it must be asymmetric and transitive.
--- Otherwise, no valid sort may be possible.
---
--- The sort algorithm is not stable: elements considered equal by the given
--- order may have their relative positions changed by the sort.
---@generic V
---@param list V[]
---@param comp? nil | fun(a:V, b:V):boolean
function table.sort(list, comp) end

return table
