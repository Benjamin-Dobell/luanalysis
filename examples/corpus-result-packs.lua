-- Future acceptance fixture, NOT part of the initial supported checker slice.
-- Inspired by ge_tts/TableUtils.ttslua:7-18 and CardState.ttslua:85-115.
-- The signature preserves alternatives of result lists, not a union of tuples
-- stored in a runtime table. The body is simplified to be self-contained.

---@param found boolean
---@return (number, string) | nil
local function lookup(found)
    if found then
        return 7, "card"
    end
    return nil
end

local id, title = lookup(true)

-- Parentheses suppress result expansion, so titleFromScalar is nil.
local scalarId, titleFromScalar = (lookup(true))

---@type nil
local checkedTitle = titleFromScalar

---@return number, string
local function forward()
    return 1, "forwarded"
end

-- The final call expands into the second and third slots.
local first, second, third = 0, forward()

---@type string
local checkedThird = third
