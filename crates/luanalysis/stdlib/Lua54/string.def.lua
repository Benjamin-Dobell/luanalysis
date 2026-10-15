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

---@shape std__StringLibrary
string = {}

---@param value string
---@return number
function string.len(value) end
---@param value string
---@return string
function string.lower(value) end
---@param value string
---@return string
function string.upper(value) end
---@overload fun(value:string,first:number):string
---@overload fun(value:string,first:number,last:number|nil):string
function string.sub(...) end
---@overload fun(value:string):number...
---@overload fun(value:string,first:number|nil):number...
---@overload fun(value:string,first:number|nil,last:number|nil):number...
function string.byte(...) end
---@overload fun(value:string,pattern:string):nil|(number,number,(string|number)...)
---@overload fun(value:string,pattern:string,first:number|nil):nil|(number,number,(string|number)...)
---@overload fun(value:string,pattern:string,first:number|nil,plain:boolean):nil|(number,number,(string|number)...)
---@overload fun(value:string,pattern:string,first:number|nil,plain:true):nil|(number,number)
function string.find(...) end
---@overload fun(value:string,pattern:string):nil|(string|number,(string|number)...)
---@overload fun(value:string,pattern:string,first:number|nil):nil|(string|number,(string|number)...)
function string.match(...) end
---@alias std__StringReplacement string|number|table<string|number,string|number|false|nil>|(fun(...:string|number):string|number|false|nil)

---@overload fun(value:string,pattern:string,replacement:std__StringReplacement):string,number
---@overload fun(value:string,pattern:string,replacement:std__StringReplacement,limit:number|nil):string,number
function string.gsub(...) end
---@overload fun(value:string,count:number):string
---@overload fun(value:string,count:number,separator:string|nil):string
function string.rep(...) end
---@param value string
---@return string
function string.reverse(value) end
---@vararg number
---@return string
function string.char(...) end
-- Dynamic formatting checks its format string, without inventing pack outputs.
---@param format string
---@vararg any
---@return string
function string.format(format, ...) end
