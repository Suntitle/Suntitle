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