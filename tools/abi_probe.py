import argparse
import ctypes
import json
import os
from pathlib import Path


class Extension:
    def __init__(self, path):
        loader = ctypes.WinDLL if os.name == 'nt' else ctypes.CDLL
        self.path = Path(path).resolve()
        search_paths = {self.path.parent, Path.cwd().resolve()}
        self.dll_directories = (
            [os.add_dll_directory(str(directory)) for directory in search_paths]
            if os.name == 'nt'
            else []
        )
        self.lib = loader(str(self.path))
        self.call = self.lib.RVExtension
        self.call.argtypes = [ctypes.c_void_p, ctypes.c_uint, ctypes.c_char_p]
        self.call.restype = None
        self.args_call = getattr(self.lib, 'RVExtensionArgs', None)
        if self.args_call:
            self.args_call.argtypes = [
                ctypes.c_void_p,
                ctypes.c_uint,
                ctypes.c_char_p,
                ctypes.POINTER(ctypes.c_char_p),
                ctypes.c_uint,
            ]
            self.args_call.restype = ctypes.c_int
        self.version = getattr(self.lib, 'RVExtensionVersion', None)
        if self.version:
            self.version.argtypes = [ctypes.c_void_p, ctypes.c_uint]
            self.version.restype = None

    def invoke(self, command, size=10240):
        allocation = ctypes.create_string_buffer(size + 16)
        ctypes.memset(allocation, 0xA5, size + 16)
        if size:
            ctypes.memset(allocation, 0, size)
        self.call(allocation, size, command.encode('utf-8'))
        raw = bytes(allocation)
        assert raw[size:] == bytes([0xA5]) * 16, 'ABI buffer overrun'
        if not size:
            return ''
        assert b'\0' in raw[:size], 'ABI missing NUL terminator'
        return raw[:size].split(b'\0', 1)[0].decode('utf-8')

    def invoke_args(self, function, args, size=10240):
        if not self.args_call:
            return None
        allocation = ctypes.create_string_buffer(size + 16)
        ctypes.memset(allocation, 0xA5, size + 16)
        if size:
            ctypes.memset(allocation, 0, size)
        encoded = [value.encode('utf-8') for value in args]
        argv = (ctypes.c_char_p * len(encoded))(*encoded) if encoded else None
        code = self.args_call(
            allocation,
            size,
            function.encode('utf-8'),
            argv,
            len(encoded),
        )
        raw = bytes(allocation)
        assert raw[size:] == bytes([0xA5]) * 16, 'RVExtensionArgs buffer overrun'
        if not size:
            return code, ''
        assert b'\0' in raw[:size], 'RVExtensionArgs missing NUL terminator'
        return code, raw[:size].split(b'\0', 1)[0].decode('utf-8')

    def invoke_version(self, size=32):
        if not self.version:
            return None
        allocation = ctypes.create_string_buffer(size + 16)
        ctypes.memset(allocation, 0xA5, size + 16)
        if size:
            ctypes.memset(allocation, 0, size)
        self.version(allocation, size)
        raw = bytes(allocation)
        assert raw[size:] == bytes([0xA5]) * 16, 'RVExtensionVersion buffer overrun'
        if not size:
            return ''
        assert b'\0' in raw[:size], 'RVExtensionVersion missing NUL terminator'
        return raw[:size].split(b'\0', 1)[0].decode('utf-8')


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('library')
    parser.add_argument('--calls', type=Path)
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    extension = Extension(args.library)
    version = extension.invoke_version()
    calls = json.loads(args.calls.read_text()) if args.calls else [
        '9:VERSION', '9:OUTPUTSIZE', '9:LOCK_STATUS', '4:99999',
        '5:99999', '0:missing:test', '1:missing:test', '2:missing:test',
        '9:UNKNOWN', 'bad', '9:LOCK', '9:LOCK_STATUS', '9:VERSION',
        '9:ADD_DATABASE:missing', '9:UNLOCK:wrong',
    ]
    rows = []
    for call in calls:
        if isinstance(call, str):
            call = {'command': call}
        actual = extension.invoke(call['command'], call.get('size', 10240))
        if 'expected' in call:
            assert actual == call['expected'], (call['command'], actual, call['expected'])
        rows.append({**call, 'actual': actual})
    if version is not None:
        rows.append({'export': 'RVExtensionVersion', 'actual': version})
    args_call = extension.invoke_args('9', ['VERSION'])
    if args_call is not None:
        args_code, args_result = args_call
        assert (args_code, args_result) == (0, '1.033'), (args_code, args_result)
        rows.append({
            'export': 'RVExtensionArgs',
            'function': '9',
            'args': ['VERSION'],
            'return_code': args_code,
            'actual': args_result,
        })
        escaped_args_call = extension.invoke_args('9', ['"VERSION"'])
        assert escaped_args_call == (0, '1.033'), escaped_args_call
        rows.append({
            'export': 'RVExtensionArgs',
            'function': '9',
            'args': ['"VERSION"'],
            'return_code': escaped_args_call[0],
            'actual': escaped_args_call[1],
        })
        max_args_call = extension.invoke_args('9', [''] * 2048)
        assert max_args_call == (0, '[0,"Error Invalid Format"]'), max_args_call
        rows.append({
            'export': 'RVExtensionArgs',
            'argument_count': 2048,
            'return_code': max_args_call[0],
            'actual': max_args_call[1],
        })
        over_limit_call = extension.invoke_args('9', [''] * 2049)
        assert over_limit_call == (1, '[0,"Error Invalid Format"]'), over_limit_call
        rows.append({
            'export': 'RVExtensionArgs',
            'argument_count': 2049,
            'return_code': over_limit_call[0],
            'actual': over_limit_call[1],
        })
    result = json.dumps(rows, indent=2, ensure_ascii=False)
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(result, encoding='utf-8')
    print(result)


if __name__ == '__main__':
    main()
