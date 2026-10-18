#!/usr/bin/env bash
set -euo pipefail
destination=${1:?Usage: bash generate-workloads.sh OUTPUT_DIRECTORY}
mkdir -p -- "$destination"

generate_workload() {
    local count=$1 path=$2 function_index condition_index
    {
        printf '%s\n' '-- Synthetic boolean-call control; predicate copy changes only the guard return annotation.'
        printf '%s\n' '---@param value string | number' '---@return boolean' 'local function isString(value)' "    return type(value) == 'string'" 'end' ''
        for ((function_index=0; function_index<count; function_index++)); do
            # Separate scopes keep this valid Lua beyond the 200-active-local limit.
            printf '%s\n' 'do' '---@param value string | number' '---@return number'
            printf 'local function inspect_%d(value)\n' "$function_index"
            printf '    local total = 0\n'
            for ((condition_index=0; condition_index<16; condition_index++)); do
                printf '%s\n' '    if isString(value) then' '        local first, second, third = value, value, value' '        total = total + 1' '    else' '        local first, second, third = value, value, value' '        total = total + 2' '    end'
            done
            printf '%s\n' '    return total' 'end' 'end' ''
        done
    } > "$path"
}
for size in 128 256; do
    generate_workload "$size" "$destination/boolean$size.lua"
    sed 's/^---@return boolean$/---@return value is string/' "$destination/boolean$size.lua" > "$destination/predicate$size.lua"
done
