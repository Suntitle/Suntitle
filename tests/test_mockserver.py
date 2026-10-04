import unittest
from unittest import mock

from mockserver import server


class MockServerTests(unittest.TestCase):
    def test_resolves_method_specific_fixture_and_status(self):
        file, status = server.resolve_response("POST", "/api/v1/extensions")

        self.assertEqual(file, server.ROOT / "api/v1/extensions.post.201.json")
        self.assertEqual(status, 201)

    def test_falls_back_to_path_fixture(self):
        file, status = server.resolve_response("GET", "/api/v1/extensions?fresh=true")

        self.assertEqual(file, server.ROOT / "api/v1/extensions.json")
        self.assertEqual(status, 200)

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
