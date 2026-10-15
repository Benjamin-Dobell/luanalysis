---@class Utility
local Utility = {}
---@overload fun<V,A:V>(arr:V[],a:A[]):void
---@generic K,V
---@param tab table<K,V>
---@vararg table<K,V>
---@return void
function Utility.consume(tab,...)
    local tables={...}
    ---@type nil|boolean
    local isArray=nil
    local isPacked=false
    for _,other in ipairs(tables) do
        if isArray==nil and next(other) then
            isArray=false
            if (--[[---@type std__Packed<V>]] other).n then
                isPacked=true
            end
        end
        for key,value in pairs(other) do tab[key]=value end
    end
    if isPacked then (--[[---@type std__Packed<V>]] tab).n=0 end
end
local labels={number='number',string='string'}
---@param key 'number'|'string'
local function unrelated(key)
    local label=labels[key]
end
