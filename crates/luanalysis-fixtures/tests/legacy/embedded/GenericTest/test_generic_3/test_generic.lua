---@class EmmyExt : Emmy
local ext = {}

function ext:reading() end

---@generic T
---@param p1 T
---@param func fun(value: T):void
local function test(p1, func)
end

test(ext, function(value)
    value.--[[caret]]
end)