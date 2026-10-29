-- A declaration file's functions are declarations: their bodies are stubs, never run.
---@class ExampleCounter
---@field value number
local ExampleCounter = {}

---@param amount number
---@return number
function ExampleCounter:add(amount) end

---@return self
function ExampleCounter:copy() end
