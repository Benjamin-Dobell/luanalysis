#!/usr/bin/env python3
"""Export the pinned IntelliJ suite, its runnable cases and a complete inventory.

Reads Git objects only; staged deletions and working files in the source checkout
are never consulted or changed. Run with --verify to check a fresh import.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import textwrap

REVISION = 'd796bb67d5135d152b9f627b69e57377c83d1a66'
ROOT = Path(__file__).resolve().parent.parent
DEST = ROOT / 'crates/luanalysis-fixtures/tests/legacy'
p = argparse.ArgumentParser(description=__doc__)
p.add_argument('repository', type=Path)
p.add_argument('--verify', action='store_true')
args = p.parse_args()

def git(*arguments):
    return subprocess.check_output(['git', '-C', str(args.repository), *arguments])

outputs = {}
ledger = []
for source in git('ls-tree', '-r', '--name-only', REVISION, 'src/test').decode().splitlines():
    data = git('show', f'{REVISION}:{source}')
    path = source.removeprefix('src/test/')
    outputs[path] = data
    ledger.append((source, hashlib.sha256(data).hexdigest()))

cases = []
inventory = []

def case(identifier, kind, inputs, origin, *, version='5.3', strict=True,
         unknown=False, weak=False, required=(), forbidden=(), notes=''):
    if kind in ('completion', 'document-symbols'):
        # LuaSettings defaults, not TypeSafetyTest.check's stricter overrides.
        strict, unknown = False, True
    row = [identifier, kind, version, str(strict).lower(), str(unknown).lower(),
           str(weak).lower(), '|'.join(required), '|'.join(forbidden), '|'.join(inputs), origin, notes]
    assert all('\t' not in field and '\n' not in field for field in row)
    assert identifier not in [c[0] for c in cases], identifier
    cases.append(row)
    return identifier


def embedded(path, index, code, tree=False, trim=True):
    # Apply Kotlin trimIndent only where the original harness applies it.
    # Raw checkByText strings keep whitespace; FileTree trims each file.
    # No Lua token or IntelliJ marker is rewritten.
    if trim:
        code = textwrap.dedent(code).strip('\n')
    if tree:
        parts = re.split(r'^\s*--- (\S+\.lua)\s*$', code, flags=re.M)
        assert not parts[0].strip(), (path, index, parts[0])
        files = [(parts[i], textwrap.dedent(parts[i+1]).strip('\n')) for i in range(1, len(parts), 2)]
    else:
        files = [('main.lua', code)]
    inputs = []
    for name, source in files:
        dest = f'embedded/{path.stem}/{index}/{name}'
        # Attribution is kept in ORIGIN.md and original Kotlin. Adding a Lua
        # comment here would shift the original ranges, so retain source only.
        outputs[dest] = source.encode()
        inputs.append(dest)
    return inputs


def strings(text):
    return [json.loads(m.group()) for m in re.finditer(r'"(?:[^"\\]|\\.)*"', text)]

ktbase = 'kotlin/com/tang/intellij/test/'
for original in sorted(k for k in outputs if k.endswith('.kt')):
    path = Path(original.removeprefix(ktbase))
    text = outputs[original].decode()
    methods = list(re.finditer(r'^    fun (`test[^`]+`|test\w+)\(', text, re.M))
    for index, method in enumerate(methods):
        name = method[1].strip('`')
        block = text[method.start():methods[index+1].start() if index+1 < len(methods) else len(text)]
        origin = 'src/test/' + original + ':' + str(text.count('\n', 0, method.start()) + 1) + '#' + name
        ids = []
        note = ''
        kind = 'diagnostics'
        slug = re.sub('[^a-zA-Z0-9_]+', '_', name).strip('_')
        base = f'{path.stem}/{slug}'
        if path.stem == 'TypeSafetyTest':
            # Resource relationships and non-default options from TypeSafetyTest.
            if name == 'testModules':
                files = ['moduleA.lua', 'moduleA_reference.lua']
            elif name == 'testModuleSupportUnavailableLua53':
                files = ['module_unavailable.lua']
            elif name == 'testUnknown':
                files = ['unknown.lua']
            else:
                call = re.search(r'check\((?:arrayOf\()?([^\n]+)', block)
                assert call, (name, block)
                files = re.findall(r'"([^"]+\.lua)"', call[1])
                assert files, name
            ids.append(case(base, kind, ['resources/inspections/' + f for f in files], origin,
                            version='5.1' if name == 'testModules' else '5.3',
                            strict=name not in ('testNonStrictNil', 'testUnknown'), unknown=name == 'testUnknown',
                            weak=name == 'testFunctionMultipleReturns'))
        elif path.stem in ('IllegalAliasTest', 'IllegalInheritanceTest'):
            file = re.search(r'checkByFile\("([^"]+)"', block)[1]
            ids.append(case(base, kind, ['resources/inspections/' + file], origin, strict=False, unknown=True,
                            notes='Only the named declaration inspection was enabled in the old harness.'))
        elif path.parts[0] == 'inspections' or (path.stem == 'Issues' and name != 'test54'):
            code = re.search(r'"""(.*?)"""', block, re.S)
            assert code, origin
            version = '5.3'
            if path.stem == 'LanguageLevelTest':
                version = {'testDisallowLua53Features':'5.2','testAllowLua53Features':'5.3',
                           'testDisallowLua54Features':'5.3','testAllowLua54Features':'5.4'}[name]
                kind = 'language-level'
            ids.append(case(base, kind, embedded(path, slug, code[1], trim='.trimIndent()' in block), origin, version=version,
                            strict=path.stem == 'Issues', unknown=path.stem != 'Issues',
                            notes='Only ' + path.stem + "'s inspections were enabled in the old harness."))
        elif path.parts[0] in ('completion', 'generic', 'vararg'):
            kind = 'completion'
            if path.stem == 'TestCompletion':
                table = {
                    'testLocalCompletion':(['testCompletion.lua'], ['a','b','func1']),
                    'testSelfCompletion':(['testSelf.lua'], ['self:aaa','self:abb']),
                    'testParamCompletion':(['testParam.lua'], ['param1','param2']),
                    'testAnnotationArray':(['class.lua','testAnnotationArray.lua'], ['name','age','sayHello']),
                    'testAnnotationFun':(['class.lua','testAnnotationFun.lua'], ['name','age','sayHello']),
                    'testAnnotationDict':(['class.lua','testAnnotationDict.lua'], ['name','age','sayHello']),
                    'testFieldCompletion':(['testFields.lua'], ['a','b']),
                }
                if name in table:
                    files, required = table[name]
                    ids.append(case(base,kind,['resources/completion/'+f for f in files],origin,required=required))
                elif name == 'testGlobalCompletion':
                    for n, (code,required) in enumerate([('<caret>',['gVar1','gVar2']),('gVar2.<caret>',['aaa','bbb','ccc'])]):
                        inputs=['resources/completion/globals.lua']+embedded(path,slug+str(n),code)
                        ids.append(case(base+str(n),kind,inputs,origin,required=required))
                elif name == 'testAnnotation':
                    # A normal Kotlin string concatenation rather than a triple-quoted one.
                    prefix = ''.join(strings(block.split('// fields and methods')[0].split('val code =',1)[1]))
                    for suffix, required in [('instance.<caret>',['name','method','staticMethod']),('instance:<caret>',['method'])]:
                        n=len(ids)
                        ids.append(case(base+str(n),kind,embedded(path,slug+str(n),prefix+suffix),origin,required=required))
                elif name == 'test table member non-completion':
                    kind = 'ide-only'
                    note = 'Asserts IntelliJ lookup element class and word-completion fallback; no LSP equivalent identity.'
            if not ids and kind != 'ide-only':
                code = re.search(r'"""(.*?)"""', block, re.S)
                assert code, origin
                inputs = embedded(path, slug, code[1], tree=True)
                if 'configureByFile("class.lua")' in block:
                    inputs.insert(0, 'resources/completion/class.lua')
                required = []
                for group in re.finditer(r'(?:it|strings!!|elementStrings!!)\.containsAll\((?:listOf|Arrays.asList)\((.*?)\)\)',block):
                    required += strings(group[1])
                for group in re.finditer(r'(?:it|strings!!|elementStrings!!)\.contains\(("(?:[^"\\]|\\.)*")\)',block):
                    required += strings(group[1])
                required += [json.loads(m[1]) for m in re.finditer(r'("(?:[^"\\]|\\.)*") in it',block)]
                forbidden=[]
                if path.stem=='TestModule' and name=='test module members visibility':
                    # Old assertFalse(containsAll) requires at least one absent,
                    # not both. Preserve this quantifier in the runner.
                    forbidden,required=required,[]
                ids.append(case(base,kind,inputs,origin,required=required,forbidden=forbidden))
        elif path.stem == 'StructureViewTest':
            kind = 'document-symbols'
            code = re.search(r'"""(.*?)"""',block,re.S)[1]
            ids.append(case(base,kind,embedded(path,slug,code),origin,required=['A','method']))
        elif path.stem in ('RenameTest','MoveFileTest'):
            kind='unsupported'
            note='File rename/move with require edits needs workspace/willRenameFiles; native symbol Rename is a separate operation.'
        elif path.stem=='CodeStyleTest' or (path.stem=='Issues' and name=='test54'):
            kind='ide-only'
            note='IntelliJ formatter settings/editor Enter action; no native formatter or editor action implementation.'
        else:
            raise AssertionError(origin)
        inventory.append([origin,kind,'|'.join(ids),note])

outputs['cases.tsv'] = ('# id\tkind\tlua\tstrict_nil\tunknown_callable_and_indexable\tweak_warnings\trequired_labels\tnot_all_labels\tinputs\torigin\tnotes\n'+ '\n'.join('\t'.join(r) for r in cases)+'\n').encode()
outputs['inventory.tsv'] = ('# origin\tkind\tcases\tnotes\n'+'\n'.join('\t'.join(r) for r in inventory)+'\n').encode()
outputs['SHA256SUMS.tsv'] = ('# source path\tsha256 (original bytes at '+REVISION+')\n'+'\n'.join('\t'.join(r) for r in ledger)+'\n').encode()
# Resources unused by any old automated test remain inventoried, not promoted
# to tests with invented expectations (typeSafety.lua has prose comments only).
used=set(f for c in cases for f in c[8].split('|'))
unused=[]
for file in sorted(k for k in outputs if k.startswith('resources/')):
    if file not in used:
        reason = ('before/after file move fixture; operation unavailable' if '/refactoring/' in file else
                  'unreferenced by baseline automated harness; preserved without invented assertions')
        unused.append(file+'\t'+reason)
outputs['unused-resources.tsv']=('# file\treason\n'+'\n'.join(unused)+'\n').encode()
for name,data in sorted(outputs.items()):
    destination=DEST/name
    if args.verify:
        if not destination.exists() or destination.read_bytes()!=data:
            raise SystemExit('Import differs: '+str(destination))
    else:
        destination.parent.mkdir(parents=True,exist_ok=True)
        destination.write_bytes(data)
print(f'{"Verified" if args.verify else "Exported"} {len(ledger)} original files, {len(inventory)} test methods, {len(cases)} runnable cases; {len(unused)} unused/unsupported resource files.')
