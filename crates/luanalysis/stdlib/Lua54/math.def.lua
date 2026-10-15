-- Copyright (c) 2018. tangzx(love.tangzx@qq.com)
--
-- Licensed under the Apache License, Version 2.0 (the "License"); you may not
-- use this file except in compliance with the License. You may obtain a copy of
-- the License at
--
-- http://www.apache.org/licenses/LICENSE-2.0
--
-- Unless required by applicable law or agreed to in writing, software
-- distributed under the License is distributed on an "AS IS" BASIS, WITHOUT
-- WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied. See the
-- License for the specific language governing permissions and limitations under
-- the License.

-- Modified for standalone Luanalysis: reviewed Lua 5.4 declaration contracts.
-- Original source and review scope are recorded in README.md.

---@shape std__Math
---@field huge number
---@field pi number
---@field maxinteger number
---@field mininteger number
math = {}

---@param x number
---@return number
function math.abs(x) end
---@param x number
---@return number
function math.acos(x) end
---@param x number
---@return number
function math.asin(x) end
---@overload fun(y:number):number
---@overload fun(y:number,x:number):number
function math.atan(...) end
---@param x number
---@return number
function math.ceil(x) end
---@param x number
---@return number
function math.cos(x) end
---@param x number
---@return number
function math.deg(x) end
---@param x number
---@return number
function math.exp(x) end
---@param x number
---@return number
function math.floor(x) end
---@param x number
---@param y number
---@return number
function math.fmod(x, y) end
---@overload fun(x:number):number
---@overload fun(x:number,base:number):number
function math.log(...) end
---@param x number
---@vararg number
---@return number
function math.max(x, ...) end
---@param x number
---@vararg number
---@return number
function math.min(x, ...) end
---@param x number
---@return number, number
function math.modf(x) end
---@param x number
---@return number
function math.rad(x) end
---@overload fun():number
---@overload fun(upper:number):number
---@overload fun(lower:number,upper:number):number
function math.random(...) end
---@overload fun():number,number
---@overload fun(x:number):number,number
---@overload fun(x:number,y:number):number,number
function math.randomseed(...) end
---@param x number
---@return number
function math.sin(x) end
---@param x number
---@return number
function math.sqrt(x) end
---@param x number
---@return number
function math.tan(x) end
---@param value any
---@return number|nil
function math.tointeger(value) end
---@param value any
---@return 'integer'|'float'|nil
function math.type(value) end
---@param m number
---@param n number
---@return boolean
function math.ult(m, n) end
