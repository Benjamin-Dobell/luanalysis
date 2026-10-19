#!/usr/bin/env python3
"""Exercise editor features through a real `luanalysis lsp` stdio process.

Usage: python3 scripts/test-editor-lsp.py /absolute/path/to/luanalysis
The runner uses temporary workspaces, current unsaved buffers and both negotiated
position encodings. It applies every returned edit and uses diagnostics carrying
the current document version as a check barrier; it never sleeps or retries a
request to hide a server failure. No build is performed by this script.
"""
from __future__ import annotations

import argparse
from collections import deque
from dataclasses import dataclass
import json
from pathlib import Path
import queue
import re
import subprocess
import sys
import tempfile
import threading
import time
from urllib.parse import unquote, urlparse


class Failure(AssertionError):
    pass


def check(condition, message):
    if not condition:
        raise Failure(message)


def line_starts(text):
    return [0] + [match.end() for match in re.finditer(r'\r\n|\r|\n', text)]


def position(text, offset, encoding):
    check(0 <= offset <= len(text), f'invalid source offset {offset}')
    starts = line_starts(text)
    line = max(index for index, start in enumerate(starts) if start <= offset)
    prefix = text[starts[line]:offset]
    units = len(prefix.encode('utf-8')) if encoding == 'utf-8' else len(prefix.encode('utf-16-le')) // 2
    return {'line': line, 'character': units}


def offset(text, point, encoding):
    starts = line_starts(text)
    line, wanted = point['line'], point['character']
    check(0 <= line < len(starts), f'edit has out-of-buffer line {point}')
    end = starts[line + 1] if line + 1 < len(starts) else len(text)
    content = text[starts[line]:end].rstrip('\r\n')
    units = 0
    for index, char in enumerate(content):
        if units == wanted:
            return starts[line] + index
        units += len(char.encode('utf-8')) if encoding == 'utf-8' else len(char.encode('utf-16-le')) // 2
        check(units <= wanted, f'position splits a Unicode code point: {point}')
    check(units == wanted, f'edit has out-of-buffer character {point}')
    return starts[line] + len(content)


def span(text, start, end, encoding):
    return {'start': position(text, start, encoding), 'end': position(text, end, encoding)}


def apply_edits(text, edits, encoding, bounds=None):
    check(isinstance(edits, list), f'expected TextEdit[], got {edits!r}')
    replacements = []
    for edit in edits:
        check(isinstance(edit.get('newText'), str), f'invalid text edit: {edit!r}')
        start = offset(text, edit['range']['start'], encoding)
        end = offset(text, edit['range']['end'], encoding)
        check(start <= end, f'reversed edit range: {edit!r}')
        if bounds is not None:
            check(bounds[0] <= start <= end <= bounds[1], f'range formatting escaped requested bounds: {edit!r}')
        replacements.append((start, end, edit['newText']))
    replacements.sort(key=lambda item: (item[0], item[1]))
    for left, right in zip(replacements, replacements[1:]):
        check(left[1] <= right[0] and left[:2] != right[:2], f'overlapping or ambiguous edits: {edits!r}')
    for start, end, replacement in reversed(replacements):
        text = text[:start] + replacement + text[end:]
    return text


def lexical_tokens(text):
    """Compare Lua token text, retaining literal/comment bytes verbatim."""
    result = []
    index = 0
    while index < len(text):
        if text[index].isspace():
            index += 1
            continue
        start = index
        comment = text.startswith('--', index)
        if comment:
            index += 2
        long_open = re.match(r'\[(=*)\[', text[index:])
        if long_open:
            close = ']' + long_open[1] + ']'
            body = index + len(long_open[0])
            end = text.find(close, body)
            check(end >= 0, 'fixture contains unterminated long literal/comment')
            index = end + len(close)
        elif comment:
            end = re.search(r'[\r\n]', text[index:])
            index = index + end.start() if end else len(text)
        elif text[index] in "\"'":
            quote = text[index]
            index += 1
            while index < len(text):
                if text[index] == '\\':
                    index += 2
                elif text[index] == quote:
                    index += 1
                    break
                else:
                    index += 1
        elif text[index].isalpha() or text[index] == '_':
            index += 1
            while index < len(text) and (text[index].isalnum() or text[index] == '_'):
                index += 1
        elif text[index].isdigit():
            number = re.match(r'(?:0[xX][0-9a-fA-F]+(?:\.[0-9a-fA-F]*)?(?:[pP][+-]?\d+)?|\d+(?:\.(?!\.)\d*)?(?:[eE][+-]?\d+)?)', text[index:])
            index += len(number[0])
        else:
            operator = next((operator for operator in ('...', '..', '==', '~=', '<=', '>=', '//', '<<', '>>', '::') if text.startswith(operator, index)), None)
            index += len(operator) if operator else 1
        result.append(text[start:index])
    return result


@dataclass
class Document:
    uri: str
    text: str
    version: int = 1


class Client:
    def __init__(self, binary, root, encoding, timeout, file_operations=True, document_changes=True):
        self.binary = binary
        self.encoding = encoding
        self.root = root
        self.timeout = timeout
        self.incoming = queue.Queue()
        self.backlog = []
        self.stderr = deque(maxlen=100)
        self.documents = {}
        self.settings = {'luaVersion': '5.4', 'moduleRoots': [str(root / 'first'), str(root / 'second')]}
        self.next_id = 0
        self.process = subprocess.Popen([str(binary), 'lsp', '--lua-version', '5.4', '--jobs', '2'], cwd=root,
                                        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        threading.Thread(target=self._read, daemon=True).start()
        threading.Thread(target=self._read_stderr, daemon=True).start()
        self.capabilities = self.request('initialize', {
            'processId': None, 'rootUri': root.as_uri(),
            'workspaceFolders': [{'uri': root.as_uri(), 'name': 'editor regressions'}],
            'capabilities': {
                'general': {'positionEncodings': [encoding]},
                'textDocument': {
                    'documentSymbol': {'hierarchicalDocumentSymbolSupport': True},
                    'codeAction': {'codeActionLiteralSupport': {'codeActionKind': {'valueSet': ['quickfix', 'refactor.rewrite']}}},
                },
                'workspace': {
                    'workspaceEdit': {'documentChanges': document_changes, 'resourceOperations': ['rename']},
                    'fileOperations': {'willRename': file_operations, 'didRename': file_operations},
                },
            },
            'initializationOptions': self.settings,
        })['capabilities']
        check(self.capabilities.get('positionEncoding') == encoding, f'encoding not negotiated: {self.capabilities!r}')
        self.notify('initialized', {})

    def _read_stderr(self):
        for line in self.process.stderr:
            self.stderr.append(line.decode('utf-8', errors='replace').rstrip())

    def _read(self):
        try:
            while True:
                headers = {}
                while True:
                    line = self.process.stdout.readline()
                    if not line:
                        raise EOFError('server stdout closed')
                    if line in (b'\n', b'\r\n'):
                        break
                    name, value = line.decode('ascii').split(':', 1)
                    headers[name.lower()] = value.strip()
                remaining = int(headers['content-length'])
                pieces = []
                while remaining:
                    part = self.process.stdout.read(remaining)
                    if not part:
                        raise EOFError('server ended inside a framed message')
                    pieces.append(part)
                    remaining -= len(part)
                self.incoming.put(json.loads(b''.join(pieces)))
        except Exception as error:
            self.incoming.put(error)

    def send(self, message):
        data = json.dumps(message, ensure_ascii=False, separators=(',', ':')).encode('utf-8')
        self.process.stdin.write(f'Content-Length: {len(data)}\r\n\r\n'.encode('ascii') + data)
        self.process.stdin.flush()

    def notify(self, method, params):
        self.send({'jsonrpc': '2.0', 'method': method, 'params': params})

    def wait(self, predicate, description):
        for index, message in enumerate(self.backlog):
            if predicate(message):
                return self.backlog.pop(index)
        deadline = time.monotonic() + self.timeout
        while True:
            try:
                message = self.incoming.get(timeout=max(0, deadline - time.monotonic()))
            except queue.Empty:
                raise Failure(f'timed out waiting for {description}; stderr: {list(self.stderr)!r}') from None
            if isinstance(message, Exception):
                raise Failure(f'{description}: {message}; stderr: {list(self.stderr)!r}')
            if 'method' in message and 'id' in message:
                # No user interaction is needed in these workspaces.
                self.send({'jsonrpc': '2.0', 'id': message['id'], 'result': None})
            elif predicate(message):
                return message
            else:
                self.backlog.append(message)

    def request(self, method, params):
        self.next_id += 1
        identifier = self.next_id
        self.send({'jsonrpc': '2.0', 'id': identifier, 'method': method, 'params': params})
        message = self.wait(lambda item: item.get('id') == identifier, method)
        check('error' not in message, f'{method} failed: {message.get("error")}')
        return message.get('result')

    def open(self, name, text, version=1):
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        if not path.exists():
            path.write_text('-- disk text deliberately differs from the unsaved buffer\n', encoding='utf-8')
        doc = Document(path.as_uri(), text, version)
        self.documents[doc.uri] = doc
        self.notify('textDocument/didOpen', {'textDocument': {'uri': doc.uri, 'languageId': 'lua', 'version': doc.version, 'text': text}})
        return doc

    def change(self, doc, text):
        doc.version += 1
        doc.text = text
        self.notify('textDocument/didChange', {'textDocument': {'uri': doc.uri, 'version': doc.version}, 'contentChanges': [{'text': text}]})

    def incremental(self, doc, start, end, replacement):
        change_range = span(doc.text, start, end, self.encoding)
        doc.version += 1
        doc.text = doc.text[:start] + replacement + doc.text[end:]
        self.notify('textDocument/didChange', {'textDocument': {'uri': doc.uri, 'version': doc.version}, 'contentChanges': [{'range': change_range, 'text': replacement}]})

    def configure(self, **settings):
        self.settings.update(settings)
        self.notify('workspace/didChangeConfiguration', {'settings': self.settings})

    def checked(self, doc):
        return self.wait(lambda item: item.get('method') == 'textDocument/publishDiagnostics'
                         and item.get('params', {}).get('uri') == doc.uri
                         and item['params'].get('version') == doc.version,
                         f'diagnostics for {doc.uri} version {doc.version}')['params']['diagnostics']

    def at(self, doc, needle, delta=0, last=False):
        start = doc.text.rindex(needle) if last else doc.text.index(needle)
        return {'textDocument': {'uri': doc.uri}, 'position': position(doc.text, start + delta, self.encoding)}

    def range(self, doc, start=0, end=None):
        return {'textDocument': {'uri': doc.uri}, 'range': span(doc.text, start, len(doc.text) if end is None else end, self.encoding)}

    def workspace_edit(self, edit):
        check(isinstance(edit, dict), f'expected WorkspaceEdit, got {edit!r}')
        changes = {}
        for uri, edits in edit.get('changes', {}).items():
            changes.setdefault(uri, []).extend(edits)
        for item in edit.get('documentChanges', []):
            check('textDocument' in item, f'unexpected resource operation in semantic edit: {item!r}')
            identifier = item['textDocument']
            uri = identifier['uri']
            if uri in self.documents:
                check(identifier.get('version') in (None, self.documents[uri].version), f'edit targets stale document version: {identifier!r}')
            changes.setdefault(uri, []).extend(item['edits'])
        check(changes, f'WorkspaceEdit contains no edits: {edit!r}')
        output = {}
        for uri, edits in changes.items():
            if uri in self.documents:
                doc = self.documents[uri]
                text = apply_edits(doc.text, edits, self.encoding)
                self.change(doc, text)
            else:
                parsed = urlparse(uri)
                check(parsed.scheme == 'file', f'edit targets unsupported URI: {uri}')
                path = Path(unquote(parsed.path))
                text = apply_edits(path.read_text(encoding='utf-8'), edits, self.encoding)
                path.write_text(text, encoding='utf-8')
            output[uri] = text
        return output

    def close(self):
        try:
            if self.process.poll() is None:
                self.request('shutdown', None)
                self.notify('exit', None)
                self.process.wait(timeout=10)
            check(self.process.returncode == 0, f'server exited {self.process.returncode}; stderr: {list(self.stderr)!r}')
        finally:
            if self.process.poll() is None:
                self.process.kill()
                self.process.wait()
            for pipe in (self.process.stdin, self.process.stdout, self.process.stderr):
                pipe.close()


def formatting(client):
    check(client.capabilities.get('documentFormattingProvider'), 'full formatting capability missing')
    check(client.capabilities.get('documentRangeFormattingProvider'), 'range formatting capability missing')
    options = {'tabSize': 2, 'insertSpaces': True, 'trimTrailingWhitespace': True,
               'insertFinalNewline': True, 'trimFinalNewlines': True}
    original = (
        '-- 😀 formatting fixture\r\n'
        '---@alias Choice\r\n---| "a"\r\n---| "b"\r\n'
        '---@param x number\r\n---@return number\r\n'
        'local function tidy( x )\r\n'
        'local result=x+1   \r\n'
        'local escaped="😀 \\"quoted\\" and end"\r\n'
        'local long=[=[literal   text\r\n  do not indent me\r\n]=]\r\n'
        '--[=[ long comment\r\n untouched body\r\n]=]\r\n'
        'local asserted=result --[[@as number]]\r\n'
        '---@cast asserted number\r\n'
        'return asserted\r\nend\r\n\r\n')
    doc = client.open('formatting.ttslua', original)
    edits = client.request('textDocument/formatting', {'textDocument': {'uri': doc.uri}, 'options': options})
    result = apply_edits(doc.text, edits, client.encoding)
    check(result != original, 'full formatter returned no useful edits for unformatted input')
    check(lexical_tokens(result) == lexical_tokens(original), 'formatting changed Lua tokens, literal bytes or comments')
    check('\n' not in result.replace('\r\n', ''), 'formatting changed CRLF convention')
    check(result.endswith('\r\n') and not result.endswith('\r\n\r\n'), 'final newline options were not applied')
    check('  local result = x + 1\r\n' in result, f'tabSize/insertSpaces or operator spacing ignored: {result!r}')
    client.change(doc, result)
    second = client.request('textDocument/formatting', {'textDocument': {'uri': doc.uri}, 'options': options})
    check(apply_edits(result, second, client.encoding) == result, 'full formatting is not idempotent')
    check(not second, 'idempotent formatting must return no redundant edits')

    # An incremental edit after a non-BMP character exercises negotiated columns.
    quoted = doc.text.index('quoted')
    client.incremental(doc, quoted, quoted + len('quoted'), 'current quote')
    addition = doc.text.index('x + 1')
    client.incremental(doc, addition, addition + len('x + 1'), 'x+2')
    current = doc.text
    edits = client.request('textDocument/formatting', {'textDocument': {'uri': doc.uri}, 'options': options})
    result = apply_edits(current, edits, client.encoding)
    check('x + 2' in result and 'x + 1' not in result and 'current quote' in result, 'formatting used stale checked/disk contents')
    client.change(doc, result)

    # A lower or duplicate version must not roll a buffer back.
    for stale_version in (doc.version - 1, doc.version):
        client.notify('textDocument/didChange', {'textDocument': {'uri': doc.uri, 'version': stale_version},
                                             'contentChanges': [{'text': 'local stale=99\n'}]})
        stale_check = client.request('textDocument/formatting', {'textDocument': {'uri': doc.uri}, 'options': options})
        check(apply_edits(doc.text, stale_check, client.encoding) == doc.text, 'stale didChange replaced the current document')

    tabs = client.open('format-tabs.lua', 'local function tabs()\nlocal result=1\nreturn result\nend\n')
    tab_options = dict(options, tabSize=4, insertSpaces=False)
    result = apply_edits(tabs.text, client.request('textDocument/formatting', {'textDocument': {'uri': tabs.uri}, 'options': tab_options}), client.encoding)
    check('\n\tlocal result = 1\n' in result, f'insertSpaces=false ignored: {result!r}')

    ranged = client.open('format-range.lua', 'local untouched=1\nlocal function selected()\nlocal s="😀";local value=2\nreturn value\nend\nlocal untouchedAfter=3\n')
    start = ranged.text.index('local function selected')
    end = ranged.text.index('local untouchedAfter')
    edits = client.request('textDocument/rangeFormatting', dict(client.range(ranged, start, end), options=options))
    result = apply_edits(ranged.text, edits, client.encoding, (start, end))
    check(result != ranged.text, 'range formatter returned no useful edits')
    check(result[:start] == ranged.text[:start] and result.endswith(ranged.text[end:]), 'range formatting changed surrounding source')
    check(lexical_tokens(result) == lexical_tokens(ranged.text), 'range formatting changed tokens')
    client.change(ranged, result)
    new_end = result.index('local untouchedAfter')
    edits = client.request('textDocument/rangeFormatting', dict(client.range(ranged, start, new_end), options=options))
    check(not edits, 'range formatting is not idempotent')

    for index, invalid in enumerate(('local function unfinished(\n', 'local broken="unterminated\n', 'local = 1\n')):
        malformed = client.open(f'format-invalid-{index}.lua', invalid)
        edits = client.request('textDocument/formatting', {'textDocument': {'uri': malformed.uri}, 'options': options})
        check(not edits, f'invalid buffer received full formatting edits: {invalid!r}: {edits!r}')

    on_type = client.capabilities.get('documentOnTypeFormattingProvider')
    check(on_type and on_type.get('firstTriggerCharacter'), 'on-type formatting capability missing')
    triggers = [on_type['firstTriggerCharacter']] + on_type.get('moreTriggerCharacter', [])
    check('\n' in triggers, f'newline on-type formatting not advertised: {triggers!r}')
    typed = client.open('format-on-type.lua', 'local function typed()\nlocal result=1\nreturn result\nend\n')
    params = dict(client.at(typed, 'local result'), ch='\n', options=options)
    edits = client.request('textDocument/onTypeFormatting', params)
    result = apply_edits(typed.text, edits, client.encoding)
    check(lexical_tokens(result) == lexical_tokens(typed.text), 'on-type formatting inserted/deleted tokens')
    check(result.count('end') == 1 and '  local result' in result, 'on-type newline did not indent or duplicated end')
    client.change(typed, result)
    params = dict(client.at(typed, 'local result'), ch='\n', options=options)
    check(not client.request('textDocument/onTypeFormatting', params), 'on-type formatting is not idempotent')

    repeated = client.open('format-on-type-repeat.lua', 'repeat\nlocal value=1\nuntil value>0\n')
    edits = client.request('textDocument/onTypeFormatting', dict(client.at(repeated, 'local value'), ch='\n', options=options))
    result = apply_edits(repeated.text, edits, client.encoding)
    check(lexical_tokens(result) == lexical_tokens(repeated.text) and result.count('until') == 1 and '\n  local value' in result, f'on-type formatting duplicated until or failed indentation: {result!r}')

    endings = client.open('format-newline.lua', 'local value=1  \r\n\r\n')
    lf_options = dict(options, newline='lf')
    result = apply_edits(endings.text, client.request('textDocument/formatting', {'textDocument': {'uri': endings.uri}, 'options': lf_options}), client.encoding)
    check(result == 'local value = 1\n', f'explicit LF/final newline/whitespace options ignored: {result!r}')
    client.change(endings, result)
    crlf_options = dict(options, newline='crlf')
    result = apply_edits(endings.text, client.request('textDocument/formatting', {'textDocument': {'uri': endings.uri}, 'options': crlf_options}), client.encoding)
    check(result == 'local value = 1\r\n', f'explicit CRLF option ignored: {result!r}')

    cr_only = client.open('format-cr.lua', 'local function cr()\rlocal value=1\rreturn value\rend\r')
    result = apply_edits(cr_only.text, client.request('textDocument/formatting', {'textDocument': {'uri': cr_only.uri}, 'options': options}), client.encoding)
    check('\n' not in result and '\r  local value = 1\r' in result, f'CR-only positions/newlines were corrupted: {result!r}')

    empty_start = ranged.text.index('value')
    edits = client.request('textDocument/rangeFormatting', dict(client.range(ranged, empty_start, empty_start), options=options))
    check(not edits, f'empty range changed source: {edits!r}')

    incomplete = client.open('format-on-type-incomplete.lua', 'if true then\n')
    edits = client.request('textDocument/onTypeFormatting', dict(client.at(incomplete, '', len(incomplete.text)), ch='\n', options=options))
    result = apply_edits(incomplete.text, edits, client.encoding)
    check(result == 'if true then\n  ', f'on-type incomplete block indentation failed or inserted end: {result!r}')
    for index, source in enumerate(('local function header(\n', 'run(function(\n', 'local function header(first,\n')):
        header = client.open(f'format-on-type-header-{index}.lua', source)
        edits = client.request('textDocument/onTypeFormatting', dict(client.at(header, '', len(source)), ch='\n', options=options))
        result = apply_edits(source, edits, client.encoding)
        check(result == source + '  ', f'on-type parameter list counted the function body twice: {result!r}')
    long_literal = client.open('format-on-type-string.lua', 'local text=[=[first\n  keep me\nlast]=]\n')
    edits = client.request('textDocument/onTypeFormatting', dict(client.at(long_literal, '  keep me'), ch='\n', options=options))
    check(not edits, f'on-type formatting touched a multiline literal: {edits!r}')

    versions = {
        '5.0': 'local function version(x)\nreturn x+1\nend\n',
        '5.1': 'local function version(...)\nreturn {...}\nend\n',
        '5.2': 'local function version()\n::label::\nlocal x=1\nreturn x\nend\n',
        '5.3': 'local function version(x)\nreturn (x<<1)|2\nend\n',
        '5.4': 'local function version()\nlocal x <const> =1\nreturn x\nend\n',
    }
    for version, source in versions.items():
        client.configure(luaVersion=version)
        version_doc = client.open(f'format-version-{version}.lua', source)
        edits = client.request('textDocument/formatting', {'textDocument': {'uri': version_doc.uri}, 'options': options})
        result = apply_edits(source, edits, client.encoding)
        check(result != source and lexical_tokens(source) == lexical_tokens(result), f'formatting failed or changed Lua {version} syntax: {edits!r}')
        client.change(version_doc, result)
        check(not client.request('textDocument/formatting', {'textDocument': {'uri': version_doc.uri}, 'options': options}), f'Lua {version} formatting is not idempotent')
    client.configure(luaVersion='5.4')
    formatting_comments(client)


def formatting_comments(client):
    source = (
        '-- This 😀 ordinary prose comment explains that every callback receives the current value and must preserve its original meaning when this long sentence is wrapped across several readable lines.\r\n'
        '---@alias FormatRecord { firstValue: number, secondValue: number, thirdValue: number, fourthValue: number, fifthValue: number, sixthValue: number }\r\n'
        '---@param value FormatRecord\r\n'
        '---@return number\r\n'
        'local function measure(value)\r\n'
        '    return value.firstValue + value.secondValue\r\n'
        'end\r\n'
        'local result = measure({firstValue = 1, secondValue = 2, thirdValue = 3, fourthValue = 4, fifthValue = 5, sixthValue = 6})\r\n'
        'return result\r\n')
    doc = client.open('format-comments.ttslua', source)
    check(not client.checked(doc), 'comment wrapping fixture has diagnostics before formatting')
    before = client.request('textDocument/hover', client.at(doc, 'value.firstValue'))
    options = {'tabSize': 4, 'insertSpaces': True, 'maxLineLength': 72}
    # Selecting only part of a comment must not rewrite the surrounding token.
    start = source.index('firstValue')
    stop = source.index('sixthValue')
    edits = client.request('textDocument/rangeFormatting', dict(client.range(doc, start, stop), options=options))
    check(not edits, f'partial comment range was reflowed: {edits!r}')
    disabled = dict(options, maxLineLength=0)
    edits = client.request('textDocument/formatting', {'textDocument': {'uri': doc.uri}, 'options': disabled})
    check(apply_edits(source, edits, client.encoding) == source, 'maxLineLength=0 did not preserve comments')
    edits = client.request('textDocument/formatting', {'textDocument': {'uri': doc.uri}, 'options': options})
    result = apply_edits(source, edits, client.encoding)
    check(result != source and '\r\n---' in result, 'long annotation was not wrapped')
    check('--[[' in result or '--[=[' in result, 'long ordinary prose was not made multiline')
    check('\n' not in result.replace('\r\n', ''), 'comment wrapping changed CRLF convention')
    check(all(len(line) <= 72 for line in result.splitlines() if line.startswith('---')), 'wrapped annotation exceeds configured width')
    check('    return value.firstValue + value.secondValue\r\n' in result, 'wrapping changed function body')
    client.change(doc, result)
    check(not client.checked(doc), 'wrapped annotation introduced diagnostics')
    after = client.request('textDocument/hover', client.at(doc, 'value.firstValue'))
    check(before and after and before['contents'] == after['contents'], f'wrapping changed native inferred type: {before!r} -> {after!r}')
    check(not client.request('textDocument/formatting', {'textDocument': {'uri': doc.uri}, 'options': options}), 'comment wrapping is not idempotent')


def signature(client):
    source = ('---@param x number\n---@param label string\n---@return number\n'
              'local function move(x, label) return x end\n\n'
              "local moved = move(1, 'a')\n"
              "local escaped = move(1, 'a\\\'b')\n"
              'local long = move(1, [=[line one\nline two]=])\n'
              "local nested = move(move(1, 'inner'), 'outer')\nreturn moved\n")
    doc = client.open('signature.lua', source)
    client.checked(doc)

    def expect(needle, delta, active, last=False):
        result = client.request('textDocument/signatureHelp', client.at(doc, needle, delta, last))
        check(result and result.get('signatures'), f'null/empty signature at {needle!r}+{delta}')
        sig = result['signatures'][result.get('activeSignature', 0)]
        label = sig['label']
        check('x: number' in label and 'label: string' in label and 'number' in label, f'untyped/wrong signature: {result!r}')
        parameter = sig.get('activeParameter', result.get('activeParameter'))
        check(parameter == active, f'wrong activeParameter at {needle!r}+{delta}: {result!r}')
        parameters = sig.get('parameters', [])
        check(len(parameters) == 2, f'signature parameter list lost: {result!r}')

    expect("'a'", 0, 1)
    expect("'a'", 1, 1)
    expect("'a\\\'b'", 4, 1)
    expect('line two', 4, 1)
    expect("'inner'", 2, 1)
    expect("'outer'", 2, 1)
    expect("move(move(1, 'inner')", len("move(move(1, 'inner')"), 0)
    # Query directly after didChange, without waiting for an old snapshot.
    client.change(doc, source.replace("move(1, 'a')", "move(1, '😀 current')"))
    expect('😀 current', 2, 1)

    for unfinished in ("local incomplete = move(1, 'unterminated", 'local incomplete = move(1, [==[unterminated'):
        client.change(doc, source + unfinished)
        result = client.request('textDocument/signatureHelp', {'textDocument': {'uri': doc.uri}, 'position': position(doc.text, len(doc.text), client.encoding)})
        check(result and result.get('signatures'), f'unfinished string lost enclosing signature: {result!r}')
        selected = result['signatures'][result.get('activeSignature', 0)]
        check(selected.get('activeParameter', result.get('activeParameter')) == 1 and 'label: string' in selected['label'], f'unfinished string selected wrong parameter/signature: {result!r}')

    shorthand = client.open('signature-shorthand.lua', '---@param label string\nlocal function say(label) return label end\nsay "😀 hello"\nsay [=[long hello]=]\n')
    for needle in ('😀 hello', 'long hello'):
        result = client.request('textDocument/signatureHelp', client.at(shorthand, needle, 3))
        check(result and result.get('signatures'), f'shorthand string call signature missing: {result!r}')
        selected = result['signatures'][result.get('activeSignature', 0)]
        check(selected.get('activeParameter', result.get('activeParameter')) == 0 and 'label: string' in selected['label'], f'shorthand string call selected wrong signature: {result!r}')


def folding(client):
    source = ('-- {{{ title\nlocal visible = 1\n'
              'local text = [==[first 😀\nsecond\nthird]==]\n'
              '-- }}} title\n')
    doc = client.open('folding.lua', source)
    folds = client.request('textDocument/foldingRange', {'textDocument': {'uri': doc.uri}})
    check(any(item.get('kind') == 'region' and item['startLine'] == 0 and item['endLine'] == 5 for item in folds), f'whitespace region markers not folded: {folds!r}')
    check(any(item['startLine'] == 2 and item['endLine'] == 4 and item.get('kind') != 'comment' for item in folds), f'long string not folded: {folds!r}')


def symbols(client):
    source = ('-- 😀 outline\n---@class Widget\n---@field size number\n'
              "local Widget = { name = 'x' }\n"
              'function Widget:run() end\n'
              'local function outer()\n  local count = 1\n  local function inner() return count end\nend\n')
    doc = client.open('symbols.lua', source)
    tree = client.request('textDocument/documentSymbol', {'textDocument': {'uri': doc.uri}})
    check(isinstance(tree, list), f'invalid DocumentSymbol[]: {tree!r}')

    def validate(items, parent=None):
        for item in items:
            start = offset(source, item['range']['start'], client.encoding)
            end = offset(source, item['range']['end'], client.encoding)
            selected_start = offset(source, item['selectionRange']['start'], client.encoding)
            selected_end = offset(source, item['selectionRange']['end'], client.encoding)
            check(start <= selected_start < selected_end <= end, f'invalid symbol range/selection: {item!r}')
            if parent:
                check(parent[0] <= start <= end <= parent[1], f'symbol outside parent range: {item!r}')
            selection = source[selected_start:selected_end]
            check(selection in item['name'] or item['name'] in selection, f'symbol selection does not select its name: {item!r} -> {selection!r}')
            validate(item.get('children', []), (start, end))

    validate(tree)
    by_name = {item['name']: item for item in tree}
    widget = by_name.get('Widget')
    check(widget and widget['kind'] == 5, f'class missing from outline: {tree!r}')
    members = {item['name']: item for item in widget.get('children', [])}
    for name, kind in [('size', 8), ('name', 8), ('run', 6)]:
        member = members.get(name) or next((item for key, item in members.items() if key.endswith(':' + name) or key.endswith('.' + name)), None)
        check(member and member['kind'] == kind, f'class member {name!r} has wrong/missing grouping or kind: {tree!r}')
    outer = by_name.get('outer')
    check(outer and outer['kind'] == 12, f'outer function missing: {tree!r}')
    children = {item['name']: item for item in outer.get('children', [])}
    check(children.get('count', {}).get('kind') == 13 and children.get('inner', {}).get('kind') == 12, f'nested local/function outline missing: {tree!r}')


def code_actions(client):
    source = "-- 😀 unsaved LuaDoc\nlocal function greet(name)\n    return 'hello'\nend\n"
    doc = client.open('code-actions.lua', source, version=-1)
    client.checked(doc)
    start = doc.text.index('greet')
    params = dict(client.range(doc, start, start + len('greet')), context={'diagnostics': [], 'only': ['refactor.rewrite']})
    actions = client.request('textDocument/codeAction', params)
    generate = next((action for action in actions or [] if action.get('title') == 'Create LuaDoc'), None)
    check(generate and generate.get('kind') == 'refactor.rewrite' and generate.get('edit'), f'Create LuaDoc action missing: {actions!r}')
    edit = generate['edit']
    check(edit.get('documentChanges'), f'LuaDoc edits must carry open-buffer versions: {edit!r}')
    for change in edit['documentChanges']:
        check(change.get('textDocument', {}).get('version') == doc.version, f'LuaDoc edit lost revision guard: {edit!r}')
    original_tokens = lexical_tokens(source)
    client.workspace_edit(edit)
    check(doc.version == 0, 'signed revision did not advance from -1 to zero')
    check(re.search(r'---@param name\?? any', doc.text), f'generated parameter annotation missing: {doc.text!r}')
    check(re.search(r'---@return (?:string|[\"\']hello[\"\'])', doc.text), f'generated inferred return annotation missing: {doc.text!r}')
    check([token for token in lexical_tokens(doc.text) if not token.startswith('--')] == [token for token in original_tokens if not token.startswith('--')], 'LuaDoc generation changed executable Lua')
    start = doc.text.index('greet(name)')
    actions = client.request('textDocument/codeAction', dict(client.range(doc, start, start + len('greet')), context={'diagnostics': [], 'only': ['refactor.rewrite']}))
    check(not any(action.get('title') == 'Create LuaDoc' for action in actions or []), 'LuaDoc generation duplicates existing annotations')


def hint_label(hint):
    label = hint['label']
    return ''.join(piece['value'] for piece in label) if isinstance(label, list) else label


def inlays(client):
    source = ('---@param width number\n---@param height number\n---@return number\n'
              'local function area(width, height) return width * height end\n'
              'local base = 2\nlocal result = area(base + 1, 4 * 2)\n'
              '---@param fn fun(value: number): string\n'
              'local function use(fn) return fn(1) end\n'
              "use(function(value) return 'yes' end)\nreturn result\n")
    doc = client.open('inlays.lua', source)
    client.checked(doc)
    hints = client.request('textDocument/inlayHint', client.range(doc))
    labels = [hint_label(hint) for hint in hints]
    check(not any(label.startswith('width:') or label.startswith('height:') for label in labels), f'expression hints should require explicit opt-in: {hints!r}')

    client.configure(inlayHints={'types': True, 'parameterNames': True, 'expressionParameterNames': True,
                                'parameterTypes': True, 'returnTypes': True})
    hints = client.request('textDocument/inlayHint', client.range(doc))
    labels = [hint_label(hint) for hint in hints]
    check(any(label.startswith('width:') for label in labels) and any(label.startswith('height:') for label in labels), f'expression argument hints missing: {hints!r}')
    parameter_end = doc.text.index('value) return') + len('value')
    callback_return = parameter_end + 1
    check(any(offset(doc.text, hint['position'], client.encoding) == parameter_end and 'number' in hint_label(hint) for hint in hints), f'inferred callback parameter type hint missing: {hints!r}')
    check(any(offset(doc.text, hint['position'], client.encoding) == callback_return and 'string' in hint_label(hint) for hint in hints), f'inferred callback return type hint missing: {hints!r}')
    for hint in hints:
        offset(doc.text, hint['position'], client.encoding)
    client.configure(inlayHints={'types': False, 'parameterNames': False, 'expressionParameterNames': False,
                                'parameterTypes': False, 'returnTypes': False})
    hints = client.request('textDocument/inlayHint', client.range(doc))
    check(not hints, f'disabled inlay hints still returned: {hints!r}')


def file_operations(client):
    advertised = client.capabilities.get('workspace', {}).get('fileOperations', {})
    check(advertised.get('willRename') and advertised.get('didRename'), f'file rename capabilities missing: {advertised!r}')
    # No document has been opened in this session. The moved source itself
    # imports another file in the same batch and must retain its disk contents.
    batch_a = client.root / 'first' / 'batch_a.lua'
    batch_b = client.root / 'first' / 'batch_b.lua'
    moved_a = batch_a.with_name('batch_new_a.lua')
    moved_b = batch_b.with_name('batch_new_b.lua')
    batch = {'files': [{'oldUri': batch_a.as_uri(), 'newUri': moved_a.as_uri()},
                       {'oldUri': batch_b.as_uri(), 'newUri': moved_b.as_uri()}]}
    outputs = client.workspace_edit(client.request('workspace/willRenameFiles', batch))
    check(batch_a.as_uri() in outputs and "require('batch_new_b')" in outputs[batch_a.as_uri()], f'unopened moved source lost semantic edits: {outputs!r}')
    unopened = client.root / 'unopened.lua'
    check(unopened.as_uri() in outputs and "require('batch_new_a')" in outputs[unopened.as_uri()], f'unopened external importer was not updated: {outputs!r}')
    batch_a.rename(moved_a)
    batch_b.rename(moved_b)
    client.notify('workspace/didRenameFiles', batch)

    old = client.root / 'first' / 'old.lua'
    new = client.root / 'first' / 'nested' / 'renamed.lua'
    source = ("local prefix = '😀'; local module = require('old')\n"
              "local ordinary = 'old' -- require('old') in a comment\nreturn module.value\n")
    doc = client.open('importer.lua', source)
    module = client.open('first/old.lua', "return { value = 'unsaved 😀' }\n")
    client.checked(doc)
    client.checked(module)

    # The second root's old.lua is shadowed by first/old.lua. Its move has no
    # semantic references here even though the textual module name is identical.
    shadow = client.root / 'second' / 'old.lua'
    shadow_edit = client.request('workspace/willRenameFiles', {'files': [{'oldUri': shadow.as_uri(), 'newUri': (shadow.parent / 'shadow-moved.lua').as_uri()}]})
    check(not shadow_edit or not any(shadow_edit.get(key) for key in ('changes', 'documentChanges')), f'shadowed module rename edited unrelated require: {shadow_edit!r}')

    # This refusal comes from the file-operation worker. It must keep its own
    # error contract when symbol renames gain structured rename-error data.
    client.next_id += 1
    refused_id = client.next_id
    client.send({'jsonrpc': '2.0', 'id': refused_id, 'method': 'workspace/willRenameFiles',
                 'params': {'files': [{'oldUri': old.as_uri(), 'newUri': shadow.as_uri()}]}})
    refusal = client.wait(lambda message: message.get('id') == refused_id, 'occupied file rename destination')
    error = refusal.get('error', {})
    check(error.get('code') == -32803 and error.get('message') == f'rename destination already exists: {shadow.resolve()}',
          f'file rename refusal changed: {refusal!r}')
    check('data' not in error, f'file rename received symbol-rename metadata: {refusal!r}')

    params = {'files': [{'oldUri': old.as_uri(), 'newUri': new.as_uri()}]}
    edit = client.request('workspace/willRenameFiles', params)
    outputs = client.workspace_edit(edit)
    check(doc.uri in outputs and "require('nested.renamed')" in doc.text, f'resolved require reference not rewritten: {edit!r}')
    check("'😀'" in doc.text and "local ordinary = 'old' -- require('old') in a comment" in doc.text, 'file rename edited ordinary literals/comments or damaged Unicode')
    check('second/old.lua' not in json.dumps(edit), 'shadowed module file was edited')
    new.parent.mkdir(parents=True, exist_ok=True)
    old.rename(new)
    client.notify('workspace/didRenameFiles', params)
    del client.documents[module.uri]
    module.uri = new.as_uri()
    client.documents[module.uri] = module
    client.checked(module)
    definition = client.request('textDocument/definition', client.at(doc, 'value', last=True))
    locations = definition if isinstance(definition, list) else [definition] if definition else []
    check(any(item.get('uri', item.get('targetUri')) == new.as_uri() for item in locations), f'renamed module identity not refreshed: {definition!r}')
    hover = client.request('textDocument/hover', client.at(doc, 'value', last=True))
    check(hover and ('unsaved' in json.dumps(hover) or 'string' in json.dumps(hover)), f'renamed open module lost unsaved contents: {hover!r}')


    # The folder operation and open document can use equivalent authorities.
    # Preserve the exact destination authority and the unsaved moved overlay.
    moved_folder = new.parent.with_name('moved 😀 folder')
    folder_old_uri = 'file://localhost' + new.parent.as_uri().removeprefix('file://')
    folder_new_uri = 'file://localhost' + moved_folder.as_uri().removeprefix('file://')
    folder_operation = {'files': [{'oldUri': folder_old_uri, 'newUri': folder_new_uri}]}
    client.workspace_edit(client.request('workspace/willRenameFiles', folder_operation))
    new.parent.rename(moved_folder)
    client.notify('workspace/didRenameFiles', folder_operation)
    del client.documents[module.uri]
    module.uri = folder_new_uri + '/' + new.name
    client.documents[module.uri] = module
    definition = client.request('textDocument/definition', client.at(doc, 'value', last=True))
    locations = definition if isinstance(definition, list) else [definition] if definition else []
    check(any(item.get('uri', item.get('targetUri')) == module.uri for item in locations),
          f'directory rename lost client destination URI: {definition!r}; expected {module.uri!r}')
    # Hover types use the last completed check; wait for this moved buffer's
    # actual versioned diagnostics before checking its inferred overlay type.
    client.checked(module)
    hover = client.request('textDocument/hover', client.at(doc, 'value', last=True))
    check(hover and ('unsaved' in json.dumps(hover) or 'string' in json.dumps(hover)),
          f'directory rename lost the unsaved module: {hover!r}')

    # Legacy clients receive no opted-in file-operation registration and can still
    # make an explicit standard request using the WorkspaceEdit.changes fallback.
    fallback_root = client.root / 'legacy-client'
    for directory in ('first', 'second'):
        (fallback_root / directory).mkdir(parents=True)
    fallback_old = fallback_root / 'first' / 'old.lua'
    fallback_old.write_text('return { value = 1 }\n', encoding='utf-8')
    fallback = Client(client.binary, fallback_root, client.encoding, client.timeout,
                      file_operations=False, document_changes=False)
    try:
        registration = fallback.capabilities.get('workspace', {}).get('fileOperations', {})
        check(not registration.get('willRename') and not registration.get('didRename'),
              f'file operations registered without client opt-in: {registration!r}')
        importer = fallback.open('importer.lua', "return require('old')\n")
        edit = fallback.request('workspace/willRenameFiles', {'files': [
            {'oldUri': fallback_old.as_uri(), 'newUri': fallback_old.with_name('new.lua').as_uri()}]})
        check(edit and edit.get('changes') and not edit.get('documentChanges'),
              f'legacy client did not receive changes fallback: {edit!r}')
        fallback.workspace_edit(edit)
        check(importer.text == "return require('new')\n", 'legacy fallback edits changed module identity')
    finally:
        fallback.close()


SUITES = {'formatting': formatting, 'signature': signature, 'folding': folding, 'symbols': symbols,
          'code-actions': code_actions, 'inlays': inlays, 'file-operations': file_operations}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('binary', type=Path, help='existing luanalysis executable; no build is run')
    parser.add_argument('--suite', action='append', choices=list(SUITES), help='run selected suite(s); default: all')
    parser.add_argument('--encoding', choices=['utf-8', 'utf-16', 'both'], default='both')
    parser.add_argument('--timeout', type=float, default=90, help='deadline per response/check in seconds (default: 90)')
    parser.add_argument('--baseline', action='store_true', help='report every selected suite failure and continue for before/after comparison; assertions stay unchanged')
    args = parser.parse_args()
    binary = args.binary.expanduser().resolve()
    check(binary.is_file(), f'server binary does not exist: {binary}')
    encodings = ['utf-16', 'utf-8'] if args.encoding == 'both' else [args.encoding]
    total = 0
    failures = []
    for encoding in encodings:
        for name in args.suite or SUITES:
            with tempfile.TemporaryDirectory(prefix='luanalysis-editor 😀 ') as temporary:
                root = Path(temporary).resolve()
                for directory in ('first', 'second'):
                    (root / directory).mkdir()
                    (root / directory / 'old.lua').write_text('return { value = 42 }\n', encoding='utf-8')
                (root / 'first' / 'batch_a.lua').write_text("local b = require('batch_b')\nreturn b\n", encoding='utf-8')
                (root / 'first' / 'batch_b.lua').write_text('return {value = 12}\n', encoding='utf-8')
                (root / 'unopened.lua').write_text("return require('batch_a')\n", encoding='utf-8')
                client = Client(binary, root, encoding, args.timeout)
                try:
                    SUITES[name](client)
                    print(f'PASS {encoding} {name}', flush=True)
                    total += 1
                except (Failure, OSError, ValueError, KeyError) as error:
                    if not args.baseline:
                        raise
                    failures.append((encoding, name, str(error)))
                    print(f'FAIL {encoding} {name}: {error}', flush=True)
                finally:
                    client.close()
    print(f'{total} passed, {len(failures)} failed real stdio editor suites', flush=True)
    return 1 if failures else 0


if __name__ == '__main__':
    try:
        sys.exit(main())
    except (Failure, OSError, ValueError, KeyError) as error:
        print(f'FAIL: {error}', file=sys.stderr)
        sys.exit(1)
