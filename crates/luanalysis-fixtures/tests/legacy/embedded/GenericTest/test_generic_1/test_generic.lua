---@generic T
---@param p1 T
---@return T
local function test(p1)
    return p1
end

local value = test(emmy)
value.--[[caret]]