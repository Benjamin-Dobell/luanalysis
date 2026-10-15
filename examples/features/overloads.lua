-- Explicit overloads are public. Long-form annotations type the implementation.
---@overload fun(value: number): number
---@overload fun(value: string): number
---@param value number|string|boolean
---@return number
local function measure(value)
    if type(value) == 'number' then
        return value
    elseif type(value) == 'string' then
        return #value
    else
        return 0
    end
end

---@type number
local numeric = measure(42)
---@type number
local textual = measure('Lua')

-- measure(true) is private to the implementation in v2. It is accepted when
-- --legacy-overloads explicitly exposes the implementation signature.
