---@param value any
---@return any
local function ordinary(value) return value end
---@param value number|nil
---@param again boolean
local function read(value, again)
 ---@type fun(value:any):any
 local guard=assert
 while again do
  guard(value)
  ---@type number
  local narrowed=value
  guard=ordinary
  value=nil
 end
end
local result=read(42, true)
