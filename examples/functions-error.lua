-- Deliberately invalid companion to functions.lua.
-- The corpus's annotated getter/record idioms are reduced to local functions.
-- These intentional mutations are not findings about production source.

---@param id number
---@return number
local function identityId(id)
    return id
end

-- Expected: call argument error at this original Lua string literal.
local invalidArgument = identityId("card")

---@param source {title: string, id: number}
---@return {title: string, id: number}
local function invalidSnapshot(source)
    -- Expected: annotated return mismatch at this record's `id` field.
    return {title = source.title, id = "seven"}
end
