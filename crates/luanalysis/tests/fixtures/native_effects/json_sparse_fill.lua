---@class ge_tts__JsonNull
local NULL = {}

---@class ge_tts__Json
local Json = {}

---@return ge_tts__JsonNull
function Json.null()
    return NULL
end

--- Fills gaps (up to the specified length) in sparseArray with Json.null(), then returns it.
---@generic T
---@generic N : number
---@param sparseArray table<N, nil | T>
---@param length number
---@return (T | ge_tts__JsonNull)[]
function Json.nullFillSparseArray(sparseArray, length)
    for i = 1, length do
        if type((--[[---@type T[] ]] sparseArray)[i]) == 'nil' then
            (--[[---@type (T | ge_tts__JsonNull)[] ]] sparseArray)[i] = Json.null()
        end
    end

    return --[[---@type (T | ge_tts__JsonNull)[] ]] sparseArray
end

return Json
