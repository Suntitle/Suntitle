# Default extensions

Python payload for the default audio source, Whisper transcriber, translator, and
subtitle display. Diamant downloads this package from OCI before launching a node.
All entry points share this directory and its Python project and lock file.

To skip translation, select a transcriber as the display's input in the UI and
save. The UI sets `inputStream=transcription`; the display listens on `/translate`,
which already broadcasts raw transcription. Selecting a translator sets
`inputStream=translation` and uses `/display`. Saving a changed stream restarts
the display with the selected endpoint. Translation is optional: a direct display
does not require an installed translator. Connected translators continue receiving
transcription independently.

These endpoints broadcast by stage, so multiple producers are not isolated by
`inputRef`. API and Kubernetes clients MUST set `settings.display.inputStream`
explicitly for direct transcription; omission keeps translated output. Deploy
the updated UI, CRD schema, Diamant, and default extension payload together.
The existing WebSocket proxy requires no changes.

Run locally with `uv run --project default-extensions python default-extensions/suntitle.py --help`.
From the repository root, publish the complete directory directly with ORAS:

```bash
oras push ghcr.io/suntitle/suntitle/default-extensions:<tag> \
  --artifact-type application/vnd.suntitle.extension.v1 \
  default-extensions:application/vnd.oci.image.layer.v1.tar+gzip
```

No generation, staging, or modification of this directory is required.
