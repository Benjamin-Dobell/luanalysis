---@class Type1
local obj = { name = "name" }

---@return number, Type1
local function getMultipleResults()
end

local a, b = getMultipleResults()
b.--[[caret]]