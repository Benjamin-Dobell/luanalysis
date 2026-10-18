-- A written predicate is a boolean return contract that also narrows its argument.
---@param value string | number
---@return value is string
local function isString(value)
    return type(value) == 'string'
end

---@param value string | number
---@return string
local function describe(value)
    if isString(value) then
        return value:upper()
    end
    return tostring(value + 1) -- the false branch contains only number
end

local text = describe('Lua')
local numeric = describe(42)

-- A nominal base class is not a discriminated union: use an explicit predicate.
---@class Object
---@field type string

---@class Bag : Object
---@field capacity number
---@field getQuantity fun(self: Bag): number

---@param obj Object
---@return obj is Bag
local function isBag(obj)
    return obj.type == 'Bag'
end

---@param obj Bag
---@return number
local function acceptsBag(obj)
    return obj:getQuantity()
end

---@param obj Object
---@return number
local function capacity(obj)
    if isBag(obj) then
        acceptsBag(obj)
        return obj.capacity -- obj is Bag here
    end
    return 0 -- obj remains Object here
end
