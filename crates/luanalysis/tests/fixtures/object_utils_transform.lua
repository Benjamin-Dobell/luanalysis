
---@shape tts__CharVectorShape
---@field x number
---@field y number
---@field z number

---@shape tts__NumVectorShape
---@field [1] number
---@field [2] number
---@field [3] number

---@alias tts__VectorShape tts__CharVectorShape|tts__NumVectorShape

---@shape tts__ObjectState_Transform
---@field posX nil|number
---@field posY nil|number
---@field posZ nil|number
---@field rotX nil|number
---@field rotY nil|number
---@field rotZ nil|number
---@field scaleX nil|number
---@field scaleY nil|number
---@field scaleZ nil|number

---@class ObjectUtils
local ObjectUtils={}
---@overload fun(position: nil | tts__VectorShape): tts__ObjectState_Transform
---@overload fun(position: nil | tts__VectorShape, rotation: nil | tts__VectorShape): tts__ObjectState_Transform
---@overload fun(position: nil | tts__VectorShape, rotation: nil | tts__VectorShape, scale: nil | tts__VectorShape): tts__ObjectState_Transform
---@overload fun(transform: {position: nil | tts__VectorShape, rotation: nil | tts__VectorShape, scale: nil | tts__VectorShape}): tts__ObjectState_Transform
---@param positionOrTransform? tts__VectorShape | {position: nil | tts__VectorShape, rotation: nil | tts__VectorShape, scale: nil | tts__VectorShape}
---@param rotationArg? tts__VectorShape
---@param scaleArg? tts__VectorShape
---@return tts__ObjectState_Transform
function ObjectUtils.transformState(positionOrTransform, rotationArg, scaleArg)
    ---@type tts__ObjectState_Transform
    local state = {}

    ---@type nil | tts__VectorShape
    local position

    ---@type nil | tts__VectorShape
    local rotation = rotationArg

    ---@type nil | tts__VectorShape
    local scale = scaleArg

    if positionOrTransform and not (positionOrTransform.x or positionOrTransform[1]) then
        position = positionOrTransform.position
        rotation = positionOrTransform.rotation
        scale = positionOrTransform.scale
    else
        position = positionOrTransform
    end

    if position then
        state.posX = (--[[---@type tts__CharVectorShape]] position).x or (--[[---@type tts__NumVectorShape]] position)[1]
        state.posY = (--[[---@type tts__CharVectorShape]] position).y or (--[[---@type tts__NumVectorShape]] position)[2]
        state.posZ = (--[[---@type tts__CharVectorShape]] position).z or (--[[---@type tts__NumVectorShape]] position)[3]
    end

    if rotation then
        state.rotX = (--[[---@type tts__CharVectorShape]] rotation).x or (--[[---@type tts__NumVectorShape]] rotation)[1]
        state.rotY = (--[[---@type tts__CharVectorShape]] rotation).y or (--[[---@type tts__NumVectorShape]] rotation)[2]
        state.rotZ = (--[[---@type tts__CharVectorShape]] rotation).z or (--[[---@type tts__NumVectorShape]] rotation)[3]
    end

    if scale then
        state.scaleX = (--[[---@type tts__CharVectorShape]] scale).x or (--[[---@type tts__NumVectorShape]] scale)[1]
        state.scaleY = (--[[---@type tts__CharVectorShape]] scale).y or (--[[---@type tts__NumVectorShape]] scale)[2]
        state.scaleZ = (--[[---@type tts__CharVectorShape]] scale).z or (--[[---@type tts__NumVectorShape]] scale)[3]
    end

    return state
end
local one=ObjectUtils.transformState({x=1,y=2,z=3})
local two=ObjectUtils.transformState({1,2,3},{x=4,y=5,z=6})
local three=ObjectUtils.transformState(nil,nil,{1,2,3})
local record=ObjectUtils.transformState({position={x=1,y=2,z=3}})
