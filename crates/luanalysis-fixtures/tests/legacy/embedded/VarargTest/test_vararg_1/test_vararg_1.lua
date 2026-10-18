---@generic T
---@param index number|string
---@vararg T
---@return T
local function select(index, ...)
end

---@type Emmy
local emmy = {}

local r = select(1, emmy, emmy, emmy)
r.--[[caret]]