---@class A
---@field aa string
local a = {}

---@return self
function a:create()end

---@class B : A
---@field bb string
local b = {}

local foo = b:create()
foo.--[[caret]]