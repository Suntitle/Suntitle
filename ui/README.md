# Suntitle UI

## Local development

To run the UI with the backend in Docker Compose, follow the container stack
commands in the root README and open http://localhost. Traefik routes `/api` requests
to the API and other paths to the UI. Rebuild the UI image after source changes.

To run the UI on the host with the mock API instead:

Run the mock API from the repository root:

```bash
python mockserver/server.py
```

Then start the UI in a second terminal:

```bash
cd ui
npm install
npm run dev
```

During development, Vite proxies `/api/*` requests to
`http://127.0.0.1:8000`, so the UI and API remain on the same browser origin.

To use a mock server or backend at another address, copy the example environment
file and change the target:

```bash
cd ui
cp .env.example .env.local
# Edit API_PROXY_TARGET, then restart npm run dev.
```

## Cypress integration tests

Run the headless integration suite from the `ui` directory:

```bash
npm run test:e2e
```

The command starts Vite on a dedicated test port, runs Cypress, and shuts the
server down afterward. API calls are intercepted by the specs; the successful
auth and hosts responses use the JSON fixtures in `../mockserver`.

For interactive debugging, start the UI with `npm run dev:test`, then run
`npm run cypress:open` in a second terminal.
