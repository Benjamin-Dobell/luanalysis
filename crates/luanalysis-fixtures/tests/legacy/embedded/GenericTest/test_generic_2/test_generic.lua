---@generic T
---@param p1 T
---@param func fun(value: T):void
local function test(p1, func)
end

test(emmy, function(value)
    value.--[[caret]]
end)