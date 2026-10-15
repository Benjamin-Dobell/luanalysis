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
-- Derived from original global.def.lua; review scope is recorded in README.md.

---@overload fun(message:any):never
---@overload fun(message:any,level:number):never
function error(...) end

---@overload fun(value:any):number|nil
---@overload fun(value:string,base:number):number|nil
function tonumber(...) end

---@param value any
---@return string
function tostring(value) end

---@type fun(value:any):'nil'|'number'|'string'|'boolean'|'table'|'function'|'thread'|'userdata'
function type(value) end

---@type 'Lua 5.4'
_VERSION = nil

---@vararg any
---@return void
function print(...) end

---@overload fun<K,V>(value:table<K,V>):nil|(K,V)
---@overload fun<K,V>(value:table<K,V>,index:K|nil):nil|(K,V)
function next(...) end

-- Raw homogeneous-table contract; __pairs override policy remains separate.
---@generic K,V
---@param value table<K,V>
---@return (fun(value:table<K,V>,index:K|nil):nil|(K,V)),table<K,V>,nil
function pairs(value) end

---@generic V
---@param value V[]
---@return (fun(value:V[],index:number):nil|(number,V)),V[],0
function ipairs(value) end

-- Written first-result view; full argument forwarding requires trusted assert support.
---@type fun<T>(value?:T,...:unknown):T
function assert(value, ...) end
