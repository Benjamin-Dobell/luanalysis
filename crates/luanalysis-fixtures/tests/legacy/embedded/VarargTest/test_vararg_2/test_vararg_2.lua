---@vararg Emmy
local function test(...)
    local t = {...}
    t[1].--[[caret]]
end