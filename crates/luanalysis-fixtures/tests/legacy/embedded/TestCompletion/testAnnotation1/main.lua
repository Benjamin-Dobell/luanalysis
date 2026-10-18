---@class MyClass
---@field public name string
local s = {}
function s:method()end
function s.staticMethod()end
---@type MyClass
local instance
instance:<caret>