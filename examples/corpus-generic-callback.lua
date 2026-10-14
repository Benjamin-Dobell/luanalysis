-- Future acceptance fixture, NOT part of the initial supported checker slice.
-- Adapted from ge_tts/TableUtils.ttslua:70-84.
-- Uses pairs instead of the project's iterate helper to remove that dependency.
-- Its checking requires native generic/callback inference, table index types,
-- loops, calls and a standard-library environment.

local TableUtils = {}

---@generic K, V, MappedV
---@param tab table<K, V>
---@param func fun(value: V, key: K): MappedV
---@return table<K, MappedV>
function TableUtils.map(tab, func)
    ---@type table<K, MappedV>
    local mapped = {}
    for key, value in pairs(tab) do
        mapped[key] = func(value, key)
    end
    return mapped
end

---@type table<string, {id: number}>
local cards = {alpha = {id = 7}, beta = {id = 9}}

local ids = TableUtils.map(cards, function(card)
    return card.id
end)

-- Target inference: table<string, number>, using the shared checker scheduler.
---@type table<string, number>
local checkedIds = ids
