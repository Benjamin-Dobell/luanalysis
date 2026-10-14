---@param value number
---@return number
local function identity(value)
    do
        local copied = value
    end
    return value
end

---@param flag boolean @An annotated function expression
---@return string
local format = function(flag)
    return "yes"
end

local boxed = {callback = identity}
local result = boxed.callback(identity(1))
format(true)
