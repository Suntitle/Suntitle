# Default extensions

Python payload for the default audio source, Whisper transcriber, translator, and
subtitle display. Diamant downloads this package from OCI before launching a node.
All entry points share this directory and its Python project and lock file.

Run locally with `uv run --project default-extensions python default-extensions/suntitle.py --help`.
From the repository root, publish the complete directory directly with ORAS:

```bash
oras push ghcr.io/suntitle/suntitle/default-extensions:<tag> \
  --artifact-type application/vnd.suntitle.extension.v1 \
  default-extensions:application/vnd.oci.image.layer.v1.tar+gzip
```

No generation, staging, or modification of this directory is required.
