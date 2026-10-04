"""Deterministic HTTP harness checks; no Porta binary or OS sandbox required."""
import socket
import unittest
from unittest.mock import Mock, call, patch

from job_server_checks import read_status_line


class StatusLineTests(unittest.TestCase):
    def stream(self, chunks):
        stream = Mock(spec=socket.socket)
        stream.gettimeout.return_value = 10
        stream.recv.side_effect = chunks
        return stream

    def test_status_split_after_protocol(self):
        # A single recv saw only this prefix in the Ubuntu job-service CI run.
        stream = self.stream([b'HTTP/1.1 ', b'413 Payload Too Large\r\n'])
        self.assertEqual(read_status_line(stream), 'HTTP/1.1 413 Payload Too Large')
        self.assertEqual(stream.recv.call_count, 2)
        self.assertEqual(stream.settimeout.call_args, call(10))

    def test_status_and_crlf_split_one_byte_at_a_time(self):
        line = b'HTTP/1.1 413 Payload Too Large\r\n'
        stream = self.stream([bytes([byte]) for byte in line])
        self.assertEqual(read_status_line(stream), line[:-2].decode())
        self.assertEqual(stream.recv.call_count, len(line))

    def test_stops_at_status_without_waiting_for_headers_body_or_eof(self):
        stream = self.stream([b'HTTP/1.1 413 Payload Too Large\r\n', socket.timeout()])
        self.assertEqual(read_status_line(stream), 'HTTP/1.1 413 Payload Too Large')
        stream.recv.assert_called_once_with(4096)

    def test_ignores_headers_and_body_received_with_status(self):
        stream = self.stream([b'HTTP/1.1 413 Payload Too Large\r\nContent-Length: 2\r\n\r\n{}'])
        self.assertEqual(read_status_line(stream), 'HTTP/1.1 413 Payload Too Large')

    def test_unexpected_status_is_preserved(self):
        stream = self.stream([b'HTTP/1.1 ', b'200 OK\r\n'])
        self.assertEqual(read_status_line(stream), 'HTTP/1.1 200 OK')

    def test_eof_before_complete_status_fails(self):
        for prefix in (b'', b'HTTP/1.1 ', b'HTTP/1.1 413 Payload Too Large\r'):
            with self.subTest(prefix=prefix):
                stream = self.stream([prefix, b''] if prefix else [b''])
                with self.assertRaisesRegex(AssertionError, 'EOF before complete HTTP status line'):
                    read_status_line(stream)
                self.assertEqual(stream.settimeout.call_args, call(10))

    def test_socket_timeout_fails(self):
        stream = self.stream([b'HTTP/1.1 ', socket.timeout()])
        with self.assertRaisesRegex(AssertionError, 'timed out reading HTTP status line'):
            read_status_line(stream)
        self.assertEqual(stream.settimeout.call_args, call(10))

    def test_fragments_share_one_deadline(self):
        stream = self.stream([b'HTTP/1.1 ', b'4'])
        with patch('job_server_checks.time.monotonic', side_effect=[100, 100, 104, 110]):
            with self.assertRaisesRegex(AssertionError, 'timed out reading HTTP status line'):
                read_status_line(stream)
        self.assertEqual(stream.recv.call_count, 2)
        self.assertEqual(stream.settimeout.call_args_list, [call(10), call(6), call(10)])

    def test_unterminated_status_cannot_exceed_size_budget(self):
        stream = self.stream([b'HTTP/1.1 ', b'x' * (4096 - 9)])
        with self.assertRaisesRegex(AssertionError, 'HTTP status line exceeds 4096 bytes'):
            read_status_line(stream)
        self.assertEqual(stream.recv.call_args_list, [call(4096), call(4096 - 9)])
        self.assertEqual(stream.settimeout.call_args, call(10))

    def test_complete_status_at_size_limit_succeeds(self):
        line = b'HTTP/1.1 413 Payload Too Large\r\n'
        stream = self.stream([line])
        self.assertEqual(read_status_line(stream, max_bytes=len(line)), line[:-2].decode())
        stream.recv.assert_called_once_with(len(line))


if __name__ == '__main__':
    unittest.main()
