-- Check with --library examples/features/counter.def.lua.
---@param counter ExampleCounter
---@return number
local function advance(counter)
    local copy = counter:copy()
    return copy:add(1)
end

-- Changing the argument to copy:add('wrong') must produce a type error.
