local function test()
    local v = {}
    v.pp = 123
    return v
end
local v = test()
v.--[[caret]]