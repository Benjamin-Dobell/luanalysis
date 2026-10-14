-- Reduced non-generic function/scope examples from the supplied corpus.
-- Record construction: ge_tts/Vector2.ttslua:88-91 (self.toData).
-- Captured getters: ge_tts/Instance.ttslua:123-126 (getInstanceGuid).
-- Annotated local function expression: ge_tts/Debug.ttslua:33-35.
-- Dependencies, nominal types and method attachment are removed. Named shapes
-- become inline records; the extra local shadow tests ordinary Lua bindings.

---@type number
local value = 42

---@param source {title: string, id: number}
---@return {title: string, id: number}
local function snapshot(source)
    local value = source.title

    ---@param id number
    ---@return {title: string, id: number}
    local function build(id)
        return {title = value, id = id}
    end

    return build(source.id)
end

---@return number
local function outerValue()
    return value
end

---@param value string
---@return string
local preserve = function(value)
    return value
end

---@type {title: string, id: number}
local checked = snapshot({title = "card title", id = 7})

---@type number
local checkedOuter = outerValue()

---@type string
local checkedExpression = preserve(checked.title)
