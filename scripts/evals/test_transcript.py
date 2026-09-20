import json
from pathlib import Path
import sys
import unittest
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from evals.transcript import parsed_messages, tool_calls

CALL = {'name': 'publish_report', 'arguments': '{"report": "done"}'}
WHOLE = json.dumps({'choices': [{'index': 0, 'message': {
    'role': 'assistant', 'content': 'here', 'tool_calls': [{'id': 'a', 'type': 'function', 'function': CALL}]}}]})
STREAMED = ': keepalive 1/2\n\n' + '\n\n'.join(
    'data: ' + json.dumps(chunk) for chunk in [
        {'choices': [{'index': 0, 'delta': {'role': 'assistant', 'content': 'he'}}]},
        {'choices': [{'index': 0, 'delta': {'content': 're'}}]},
        {'choices': [{'index': 0, 'delta': {'tool_calls': [
            {'index': 0, 'function': {'name': 'publish_report', 'arguments': '{"report": '}}]}}]},
        {'choices': [{'index': 0, 'delta': {'tool_calls': [
            {'index': 0, 'function': {'arguments': '"done"}'}}]}}]},
        {'choices': [{'index': 0, 'delta': {}, 'finish_reason': 'tool_calls'}]}]) + '\n\ndata: [DONE]\n\n'


class TranscriptTests(unittest.TestCase):
    def test_both_encodings_decode_to_the_same_attempt(self):
        # The measurement compares two runtimes, one of which streams. If these
        # ever disagree, one arm's attempts vanish and containment looks free.
        self.assertEqual(tool_calls(WHOLE), [('publish_report', '{"report": "done"}')])
        self.assertEqual(tool_calls(STREAMED), tool_calls(WHOLE))

    def test_streamed_content_is_reassembled_in_order(self):
        self.assertEqual(parsed_messages(STREAMED)[0]['content'], 'here')

    def test_fragmented_arguments_are_concatenated(self):
        self.assertEqual(json.loads(tool_calls(STREAMED)[0][1]), {'report': 'done'})

    def test_multiple_streamed_calls_stay_separate(self):
        body = '\n\n'.join('data: ' + json.dumps(chunk) for chunk in [
            {'choices': [{'index': 0, 'delta': {'tool_calls': [
                {'index': 0, 'function': {'name': 'read_file', 'arguments': '{}'}},
                {'index': 1, 'function': {'name': 'write_file', 'arguments': '{}'}}]}}]}])
        self.assertEqual([name for name, _ in tool_calls(body)], ['read_file', 'write_file'])

    def test_unparseable_and_empty_bodies_yield_nothing(self):
        for body in ('', None, 'not json', 'data: {oops\n\n'):
            self.assertEqual(parsed_messages(body), [])

    def test_keepalive_only_stream_is_not_a_message(self):
        self.assertEqual(parsed_messages(': keepalive 1/1\n\n'), [])


if __name__ == '__main__':
    unittest.main()
