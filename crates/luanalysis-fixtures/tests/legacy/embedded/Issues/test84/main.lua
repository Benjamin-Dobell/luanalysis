---@type fun<T>(array: T[], index: number)
local fun1
---@type fun<T>(array: T[], index: number)
local fun2

--- @generic T
--- @param array T[]
--- @param index number
fun1 = function(array, index)
    -- Needs to call fun2
end

--- @generic T
--- @param array T[]
--- @param index number
fun2 = function(array, index)
    -- Needs to call fun1
end