---@alias MyKey "key1"|"key2"|"key3"

---@shape MyShape
---@field a string
---@field b number

local shapeTable = {
    key1 = {a = "1", b = 1},
    key2 = {a = "2", b = 2},
    key3 = {a = "3", b = 3},
} ---@type table<MyKey, MyShape>

local shapeTableWithEqualValues = {
    key1 = {a = "1", b = 1},
    key2 = {a = "1", b = 1},
    key3 = {a = "1", b = 1},
} ---@type table<MyKey, MyShape>