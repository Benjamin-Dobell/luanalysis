local count = 42
---@type number
local checked = count
---@type {title: string, count: number | nil}
local item = {title = "native Lua", count = 1}
local title = item.title
---@type number | nil
local maybe = nil
maybe = 2
