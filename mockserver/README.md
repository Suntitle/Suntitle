# Mock server

```bash
python mockserver/server.py
```

The URL path maps literally to a file in this folder with `.json` appended:

```text
/api/v1/hosts?status=online -> mockserver/api/v1/hosts.json
```

Method-specific fixtures use `.<method>.json`. To return a different successful
status, use `.<method>.<status>.json`; supported status suffixes are 200, 201,
202, and 204. Query strings and request bodies are ignored. The server listens
at `http://127.0.0.1:8000`.

`api/v1` contains flat HTTP responses: resource `name`, desired fields including
`displayName`, and unchanged resource `status`. Host routes use resource names.
`fixtures/kubernetes` contains internal host/node resource lists, flat create/update
requests, and an API-server conflict response. Restart has no request body.
Lists expose opaque versions. Nodes without status represent persisted desired
state awaiting the controller.
