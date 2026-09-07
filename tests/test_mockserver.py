import unittest
from unittest import mock

from mockserver import server


class MockServerTests(unittest.TestCase):
    @mock.patch.object(server, "ThreadingHTTPServer")
    def test_ctrl_c_stops_server_and_closes_socket(self, server_class):
        http_server = server_class.return_value
        http_server.serve_forever.side_effect = KeyboardInterrupt

        with self.assertLogs(server.LOGGER, level="INFO") as logs:
            server.main()

        server_class.assert_called_once_with(server.SERVER_ADDRESS, server.Handler)
        http_server.serve_forever.assert_called_once_with()
        http_server.server_close.assert_called_once_with()
        self.assertEqual(
            logs.output,
            [
                "INFO:mockserver.server:Mock server is running at http://127.0.0.1:8000",
                "INFO:mockserver.server:Mock server shutdown requested",
                "INFO:mockserver.server:Mock server has stopped",
            ],
        )


if __name__ == "__main__":
    unittest.main()
