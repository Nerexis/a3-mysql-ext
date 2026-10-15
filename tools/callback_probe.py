import argparse
import ctypes
import os
import queue
import re
import time

from abi_probe import Extension


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('library')
    args = parser.parse_args()

    extension = Extension(args.library)
    register = extension.lib.RVExtensionRegisterCallback
    callback_type = ctypes.WINFUNCTYPE if os.name == 'nt' else ctypes.CFUNCTYPE
    callback_type = callback_type(
        ctypes.c_int, ctypes.c_char_p, ctypes.c_char_p, ctypes.c_char_p
    )
    events = queue.Queue()
    attempts = []

    def receive(name, function, data):
        event = (name.decode(), function.decode(), data.decode())
        attempts.append(event)
        if len(attempts) == 1:
            return -1
        events.put(event)
        return 99

    callback = callback_type(receive)
    register.argtypes = [callback_type]
    register.restype = None

    unavailable = extension.invoke('6:SQL:slow:async')
    assert unavailable == '[0,"Error Callback Unavailable"]', unavailable

    register(callback)
    try:
        assert extension.invoke('9:ADD_DATABASE:test') == '[1]'
        assert extension.invoke(
            '9:ADD_DATABASE_PROTOCOL:test:SQL_CUSTOM:SQL:probe.ini'
        ) == '[1]'

        for command, expected in [
            ('6:SQL:slow:async', '[1,[[0,"async"]]]'),
            ('6:SQL:large', None),
        ]:
            started = extension.invoke(command, 1024)
            match = re.fullmatch(r'\[2,"?(\d+)"?\]', started)
            assert match, (command, started)
            request_id = match.group(1)

            deadline = time.monotonic() + 15
            while True:
                event = events.get(timeout=max(0.01, deadline - time.monotonic()))
                assert time.monotonic() < deadline, 'callback deadline exceeded'
                if event[0] == 'extDB3' and event[1] == 'asyncResultReady':
                    assert event[2] == '[' + request_id + ']', (event, request_id)
                    break

            result = extension.invoke('4:' + request_id, 1024)
            assert result != '[3]', 'callback arrived before result was ready'
            if result == '[5]':
                parts = []
                while True:
                    part = extension.invoke('5:' + request_id, 1024)
                    if not part:
                        break
                    assert part != '[3]', 'multipart result became pending'
                    parts.append(part)
                result = ''.join(parts)
            if expected is not None:
                assert result == expected, (command, result, expected)
            else:
                assert result.startswith('[1,[["x'), result[:80]
                assert len(result.encode('utf-8')) > 10000, len(result)
            assert extension.invoke('4:' + request_id) == '', 'result was not consumed'

        assert attempts[0][0:2] == ('extDB3', 'asyncResultReady')
        assert attempts[1] == attempts[0], attempts[:2]
        print('Callback probe passed: unavailable guard, retry, async results, and multipart polling')
    finally:
        register(ctypes.cast(None, callback_type))


if __name__ == '__main__':
    main()
