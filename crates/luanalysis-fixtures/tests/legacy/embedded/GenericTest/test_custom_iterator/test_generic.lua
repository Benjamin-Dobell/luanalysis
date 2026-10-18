---@generic T
---@param list T[]
---@return fun():number, T
local function myIterator(list)
    local idx = 0
    return nil -- todo
end

---@type Emmy[]
local emmyList = {}

for i, em in myIterator(emmyList) do
    em.--[[caret]]
end