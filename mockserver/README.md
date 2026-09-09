# Mock server

```bash
python mockserver/server.py
```

The URL path maps literally to a file in this folder with `.json` appended:

```text
/api/v1/hosts?status=online -> mockserver/api/v1/hosts.json
```

GET, POST, and PUT behave identically. Query strings and request bodies are
ignored. The server listens at `http://127.0.0.1:8000`.

Phase 2 resource-shaped API fixtures are in `fixtures/kubernetes`: host/node
lists, create/update requests, and an API-server conflict response. Restart has
no request body. Lists expose resource names and opaque versions; UIDs are fixture values. Nodes omit status to represent
persisted desired state awaiting the controller. The existing literal-path server
fixtures remain for the UI until the phase 5 client integration.
