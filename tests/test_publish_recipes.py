"""Verify publication ordering and digest propagation without pushing remote artifacts."""

import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]
DIGEST = "sha256:" + "a" * 64


@unittest.skipUnless(shutil.which("just"), "just is required for publication recipe tests")
class PublishRecipeTests(unittest.TestCase):
    """Run the real recipes with registry and container commands replaced by local probes."""

    def setUp(self):
        """Prepare command probes and the unmodified recipes in an isolated workspace."""
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        shutil.copy(ROOT / "justfile", self.root / "justfile")
        shutil.copy(ROOT / ".env.example", self.root / ".env.example")
        (self.root / "pki").mkdir()
        for name in ["ca.crt", "apiserver.crt", "apiserver.key", "sa.pub", "sa.key", "admin.crt", "admin.key"]:
            (self.root / "pki" / name).write_text("fixture")
        (self.root / "scripts").mkdir()
        (self.root / "scripts/generate-kubeconfigs.sh").write_text("exit 0\n")
        binaries = self.root / "bin"
        binaries.mkdir()
        probe = "#!" + sys.executable + "\n" + r'''
import json, os, pathlib, sys
tool = pathlib.Path(sys.argv[0]).name
args = sys.argv[1:]
with open(os.environ['PUBLISH_LOG'], 'a') as log:
    log.write(json.dumps({'tool':tool, 'args':args, 'reference':os.environ.get('DEFAULT_EXTENSIONS_REFERENCE')}) + '\n')
if tool == 'git':
    print('2026-01-01T00:00:00Z')
elif tool == 'oras':
    if args[0] == os.environ.get('PUBLISH_FAIL'):
        sys.exit(27)
    if args[0] == 'resolve':
        print(args[-1].rsplit(':', 1)[0] + '@' + os.environ['PUBLISH_DIGEST'])
elif tool == 'docker' and 'config' in args:
    print(json.dumps({'services':{'ws-proxy':{'environment':{'EXTENSION_REGISTRY':'registry.test:5000/suntitle/extensions'}}}}))
'''
        for name in ["docker", "oras", "curl", "git"]:
            binary = binaries / name
            binary.write_text(probe)
            binary.chmod(0o755)
        self.log = self.root / "calls.jsonl"
        self.environment = os.environ | {
            "PATH": str(binaries) + os.pathsep + os.environ["PATH"],
            "PUBLISH_LOG": str(self.log), "PUBLISH_DIGEST": DIGEST,
            "DEFAULT_EXTENSIONS_REFERENCE": "",
        }

    def run_recipe(self, *arguments, failure=""):
        """Return recorded external calls and the recipe result, including failures."""
        self.log.write_text("")
        result = subprocess.run(["just", *arguments], cwd=self.root,
                                env=self.environment | {"PUBLISH_FAIL": failure},
                                capture_output=True, text=True)
        calls = [json.loads(line) for line in self.log.read_text().splitlines()]
        return result, calls

    def test_images_publish_before_build_and_pin_the_artifact_digest(self):
        """Component builds MUST reference the payload published under the selected repository and tag."""
        result, calls = self.run_recipe("images-push", "ghcr.io/suntitle/suntitle", "dev", "linux/amd64")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual([(call["tool"], call["args"][0]) for call in calls if call["tool"] != "git"],
                         [("oras", "push"), ("oras", "resolve"), ("docker", "buildx"),
                          ("docker", "buildx"), ("docker", "buildx")])
        push = next(call for call in calls if call["tool"] == "oras")
        self.assertIn("ghcr.io/suntitle/suntitle/default-extensions:dev", push["args"])
        self.assertIn("default-extensions:application/vnd.oci.image.layer.v1.tar+gzip", push["args"])
        self.assertIn("org.opencontainers.image.created=2026-01-01T00:00:00Z", push["args"])
        builds = [call for call in calls if call["tool"] == "docker"]
        for build in builds[:2]:
            self.assertIn("DEFAULT_EXTENSIONS_REFERENCE=ghcr.io/suntitle/suntitle/default-extensions@" + DIGEST,
                          build["args"])

    def test_publication_failure_prevents_image_builds(self):
        """A failed push or digest lookup MUST NOT produce images referencing a missing payload."""
        for failure in ["push", "resolve"]:
            with self.subTest(failure=failure):
                result, calls = self.run_recipe("images-push", "registry.test/suntitle", "dev", failure=failure)
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse(any(call["tool"] == "docker" for call in calls))

    def test_local_startup_publishes_before_starting_the_proxy(self):
        """Local startup MUST publish the checkout and configure the proxy with its immutable reference."""
        result, calls = self.run_recipe("_local-infra-up")
        self.assertEqual(result.returncode, 0, result.stderr)
        push = next(call for call in calls if call["tool"] == "oras" and call["args"][0] == "push")
        self.assertIn("--plain-http", push["args"])
        self.assertIn("registry.test:5000/suntitle/default-extensions:dev", push["args"])
        registry_start = calls[0]
        self.assertIn("--no-deps", registry_start["args"])
        self.assertIn("distribution", registry_start["args"])
        self.assertIn("traefik", registry_start["args"])
        configured = [call for call in calls if call["tool"] == "docker" and "config" not in call["args"]][1:]
        self.assertEqual(len(configured), 2)
        for call in configured:
            self.assertEqual(call["reference"], "registry.test:5000/suntitle/default-extensions@" + DIGEST)

    def test_local_lookup_failure_prevents_proxy_build_and_start(self):
        """Digest lookup failure MUST stop local startup before building or starting applications."""
        result, calls = self.run_recipe("_local-infra-up", failure="resolve")
        self.assertNotEqual(result.returncode, 0)
        docker = [call for call in calls if call["tool"] == "docker"]
        self.assertEqual(len(docker), 2)
        self.assertFalse(any("build" in call["args"] for call in docker))


@unittest.skipUnless(shutil.which("docker"), "Docker Compose is required for configuration tests")
class ComposeReferenceTests(unittest.TestCase):
    """Check rendered development and deployment configuration without starting containers."""

    def test_reference_override_reaches_build_and_runtime(self):
        """An explicit digest MUST override both paths; empty runtime values MUST retain the embedded default."""
        for reference in ["", "registry.test/suntitle/default-extensions@" + DIGEST]:
            with self.subTest(reference=reference):
                environment = os.environ | {"DEFAULT_EXTENSIONS_REFERENCE": reference}
                output = subprocess.check_output([
                    "docker", "compose", "--env-file", str(ROOT / ".env.example"),
                    "-f", str(ROOT / "compose.yaml"), "config", "--format", "json",
                ], cwd=ROOT, env=environment)
                proxy = json.loads(output)["services"]["ws-proxy"]
                self.assertEqual(proxy["environment"]["DEFAULT_EXTENSIONS_REFERENCE"], reference)
                self.assertEqual(proxy["build"]["args"]["DEFAULT_EXTENSIONS_REFERENCE"],
                                 reference or "ghcr.io/suntitle/suntitle/default-extensions:latest")

    def test_release_configuration_preserves_the_embedded_digest(self):
        """Deployment MUST pull the selected proxy image without overriding its embedded digest by default."""
        environment = os.environ | {
            "DEFAULT_EXTENSIONS_REFERENCE": "", "API_IMAGE": "example/api:test",
            "PROXY_IMAGE": "example/proxy:test", "UI_IMAGE": "example/ui:test",
        }
        output = subprocess.check_output([
            "docker", "compose", "--env-file", str(ROOT / ".env.example"),
            "-f", str(ROOT / "compose.yaml"), "-f", str(ROOT / "compose.release.yaml"),
            "config", "--format", "json",
        ], cwd=ROOT, env=environment)
        proxy = json.loads(output)["services"]["ws-proxy"]
        self.assertNotIn("build", proxy)
        self.assertEqual(proxy["environment"]["DEFAULT_EXTENSIONS_REFERENCE"], "")


if __name__ == "__main__":
    unittest.main()
