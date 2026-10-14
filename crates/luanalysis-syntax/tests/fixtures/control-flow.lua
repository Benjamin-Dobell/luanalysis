---@param value number | nil
---@return number
local function choose(value)
    if value ~= nil then
        return value
    elseif true then
        local fallback = 1
        return fallback
    else
        return 0
    end
end

local count = 0
while count < 3 do
    count = count + 1
    if count == 2 then
        break
    end
end

repeat
    local finished = count > 0
    count = count - 1
until finished

for index = 1, 4, 2 do
    count = count + index
end

for key, value in iterator(state), initial do
    local copy = value
end
