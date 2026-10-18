---@alias Handler fun(emmy: Emmy):void

---@param func Handler
local function test(func)
end

test(function(value)
    value.--[[caret]]
end)