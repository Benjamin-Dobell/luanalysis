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

---@shape std__Table
table = {}

---@overload fun(list:(string|number)[]):string
---@overload fun(list:(string|number)[],sep:string):string
---@overload fun(list:(string|number)[],sep:string,first:number):string
---@overload fun(list:(string|number)[],sep:string,first:number,last:number):string
function table.concat(...) end
---@overload fun<V>(list:V[],value:V):void
---@overload fun<V>(list:V[],position:number,value:V):void
function table.insert(...) end
---@overload fun<V>(list:V[],first:number,last:number,target:number):V[]
---@overload fun<V>(list:V[],first:number,last:number,target:number,destination:V[]):V[]
function table.move(...) end
---@overload fun<V>(list:V[]):V|nil
---@overload fun<V>(list:V[],position:number):V|nil
function table.remove(...) end
---@overload fun<V>(list:V[]):void
---@overload fun<V>(list:V[],comparator:fun(a:V,b:V):boolean):void
function table.sort(...) end
