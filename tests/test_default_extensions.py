"""Verify the default extension directory is directly publishable as an OCI payload."""

import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]


@unittest.skipUnless(shutil.which("oras"), "ORAS is required for OCI packaging verification")
class DefaultExtensionTests(unittest.TestCase):
    """Exercise direct ORAS publication without registry credentials."""

    def test_directory_round_trip(self):
        """The complete source directory MUST survive an unmodified ORAS push and pull."""
        with tempfile.TemporaryDirectory() as directory:
            layout = str(Path(directory) / "layout")
            publish = [
                "oras", "push", "--oci-layout", layout + ":test",
                "--artifact-type", "application/vnd.suntitle.extension.v1",
                "--annotation", "org.opencontainers.image.created=2026-01-01T00:00:00Z",
                "default-extensions:application/vnd.oci.image.layer.v1.tar+gzip",
            ]
            subprocess.run(publish, cwd=ROOT, check=True, capture_output=True)
            resolve = ["oras", "resolve", "--oci-layout", layout + ":test"]
            digest = subprocess.check_output(resolve)
            subprocess.run(publish, cwd=ROOT, check=True, capture_output=True)
            self.assertEqual(subprocess.check_output(resolve), digest)
            manifest = json.loads(subprocess.check_output([
                "oras", "manifest", "fetch", "--oci-layout", layout + ":test",
            ]))
            layer, = manifest["layers"]
            self.assertEqual(manifest["annotations"]["org.opencontainers.image.created"], "2026-01-01T00:00:00Z")
            self.assertEqual(layer["mediaType"], "application/vnd.oci.image.layer.v1.tar+gzip")
            self.assertEqual(layer["annotations"]["io.deis.oras.content.unpack"], "true")
            self.assertEqual(layer["annotations"]["org.opencontainers.image.title"], "default-extensions")
            output = Path(directory) / "pulled"
            subprocess.run([
                "oras", "pull", "--oci-layout", layout + ":test", "--output", str(output),
            ], check=True, capture_output=True)
            source = ROOT / "default-extensions"
            payload = output / "default-extensions"
            expected = {path.relative_to(source) for path in source.rglob("*") if path.is_file()}
            self.assertEqual({path.relative_to(payload) for path in payload.rglob("*") if path.is_file()}, expected)
            for name in expected:
                self.assertEqual((payload / name).read_bytes(), (source / name).read_bytes())


if __name__ == "__main__":
    unittest.main()
