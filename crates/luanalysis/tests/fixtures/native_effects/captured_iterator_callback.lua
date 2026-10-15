---@param values number[]
---@return (fun(values: number[], index: number): nil | (number, number)), number[], number
local function iterate(values)
    return ipairs(values)
end

---@param arrays number[][]
---@param predicate fun(value: number, index: number): boolean
local function visit(arrays, predicate)
    for _, array in ipairs(arrays) do
        for index, value in iterate(array) do
            if predicate(value, index) then
                ---@type number
                local checked = value
            end
        end
    end
end
