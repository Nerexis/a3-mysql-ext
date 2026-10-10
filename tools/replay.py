import argparse
import json
import re
import time
from pathlib import Path
from abi_probe import Extension


def collect(extension, command, size):
    response = extension.invoke(command, size)
    match = re.fullmatch(r'\[2,"?(\d+)"?\]', response)
    if not match:
        return response, []
    ticket = match.group(1)
    deadline = time.monotonic() + 15
    polls = [response]
    while True:
        response = extension.invoke('4:' + ticket, size)
        polls.append(response)
        if response != '[3]':
            break
        assert time.monotonic() < deadline, 'poll deadline exceeded'
        time.sleep(0.001)
    if response == '[5]':
        parts = []
        while True:
            chunk = extension.invoke('5:' + ticket, size)
            if chunk == '':
                break
            assert chunk != '[3]', 'ready result became pending'
            assert time.monotonic() < deadline, 'multipart deadline exceeded'
            parts.append(chunk)
        response = ''.join(parts)
        polls.append({'fragments': len(parts)})
    assert extension.invoke('4:' + ticket, size) == '', 'consumed result remains'
    assert extension.invoke('5:' + ticket, size) == '', 'consumed multipart remains'
    return response, polls


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('library')
    parser.add_argument('fixture', type=Path)
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    extension = Extension(args.library)
    fixtures = json.loads(args.fixture.read_text(encoding='utf-8'))
    rows = []
    for fixture in fixtures:
        if 'command' in fixture:
            command = fixture['command']
        else:
            command = fixture['prefix']
            call_args = fixture.get('args')
            if 'args_file' in fixture:
                call_args = json.loads(
                    (args.fixture.parent / fixture['args_file']).read_text(encoding='utf-8')
                )
            if call_args:
                command += ':' + ':'.join(call_args)
        response, polls = collect(extension, command, fixture.get('size', 10240))
        if 'expected' in fixture:
            assert response == fixture['expected'], (command, response, fixture['expected'])
        if 'expected_regex' in fixture:
            assert re.fullmatch(fixture['expected_regex'], response), (command, response)
        if 'expected_prefix' in fixture:
            assert response.startswith(fixture['expected_prefix']), (command, response)
        if 'minimum_bytes' in fixture:
            assert len(response.encode('utf-8')) >= fixture['minimum_bytes'], (command, len(response))
        if fixture.get('expect_wait'):
            assert '[3]' in polls, (command, polls)
        if fixture.get('expect_multipart'):
            assert '[5]' in polls and any(
                isinstance(item, dict) and item.get('fragments', 0) > 0 for item in polls
            ), (command, polls)
        rows.append({'command': command, 'response': response, 'polls': polls})
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(rows, indent=2, ensure_ascii=False), encoding='utf-8')
    print('Replayed {} calls into {}'.format(len(rows), args.output))


if __name__ == '__main__':
    main()
