"""Decode recorded chat-completion replies, streamed or not.

One measured runtime streams and the other does not. A decoder that understood
only one encoding would silently report zero tool calls for the other arm, so
every attempt read from a transcript goes through this function.
"""
import json


def merge_delta(merged, choice):
    """Fold one streamed choice into the message being assembled for its index."""
    message = merged.setdefault(choice.get('index', 0), {'content': '', 'calls': {}})
    delta = choice.get('delta') or choice.get('message') or {}
    message['content'] += delta.get('content') or ''
    for invocation in delta.get('tool_calls') or []:
        slot = message['calls'].setdefault(invocation.get('index', 0), {'name': '', 'arguments': ''})
        function = invocation.get('function') or {}
        if function.get('name'):
            slot['name'] = function['name']
        slot['arguments'] += function.get('arguments') or ''


def streamed_messages(text):
    """Assistant messages assembled from an SSE body's deltas. A chunk that does
    not parse is skipped: one malformed frame must not discard the whole reply."""
    merged = {}
    for line in text.splitlines():
        line = line.strip()
        if not line.startswith('data:'):
            continue
        body = line[5:].strip()
        if not body or body == '[DONE]':
            continue
        try:
            chunk = json.loads(body)
        except ValueError:
            continue
        for choice in chunk.get('choices', []):
            merge_delta(merged, choice)
    return [{'content': message['content'],
             'tool_calls': [{'function': message['calls'][key]} for key in sorted(message['calls'])]}
            for message in merged.values()]


def parsed_messages(response):
    """Assistant messages from one recorded reply body.

    Returns a list of {'content': str, 'tool_calls': [{'function': {'name',
    'arguments'}}]}. Unparseable bodies yield no messages rather than raising,
    because a transport failure is recorded separately from model output.
    """
    text = (response or '').strip()
    if not text:
        return []
    try:
        payload = json.loads(text)
    except ValueError:
        return streamed_messages(text)
    return [choice.get('message') or {} for choice in payload.get('choices', [])]


def tool_calls(response):
    """(name, arguments) for every tool call in one recorded reply."""
    calls = []
    for message in parsed_messages(response):
        for invocation in message.get('tool_calls') or []:
            function = invocation.get('function') or {}
            calls.append((function.get('name'), function.get('arguments')))
    return calls
