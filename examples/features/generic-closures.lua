-- Each call retains its own generic substitution in the returned closure.
---@generic T
---@param value T
local function makeReader(value)
    ---@return T
    return function()
        return value
    end
end

local readNumber = makeReader(42)
local readString = makeReader('Lua')

---@type number
local numeric = readNumber()
---@type string
local textual = readString()

-- Changing textual's annotation to number must produce a type error.
