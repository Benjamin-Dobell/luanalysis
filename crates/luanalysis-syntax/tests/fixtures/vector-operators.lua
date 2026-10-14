---@param left {x: number, y: number}
---@param right {x: number, y: number}
---@return {x: number, y: number}
local function add(left, right)
    return {x = left.x + right.x, y = left.y + right.y}
end

---@param vector {x: number, y: number}
---@return number
local function squaredLength(vector)
    return vector.x ^ 2 + vector.y ^ 2
end

local vector = {x = 1, y = 2}
local component = vector["x"]
vector["y"] = 3
local combined = add(vector, {x = 3, y = 4})
local magnitude = squaredLength(combined)
