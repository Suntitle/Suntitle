import importlib.util
import pathlib
import subprocess
import sys
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[1] / "default-extensions"


def load_script(name):
    path = ROOT / name
    spec = importlib.util.spec_from_file_location(name.replace("-", "_"), path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class NodeCliTests(unittest.TestCase):
    def test_lightweight_entry_points_expose_help(self):
        for script in ("suntitle.py", "suntitle-translator.py", "suntitle-listener.py", "whisper_online_client.py"):
            result = subprocess.run(
                [sys.executable, str(ROOT / script), "--help"],
                check=False,
                capture_output=True,
                text=True,
            )
            self.assertEqual(result.returncode, 0, result.stderr)

    def test_display_message_format_controls_original_text(self):
        display = load_script("suntitle.py")
        message = '{"original":"Hello","translated":"Ahoj"}'
        self.assertEqual(display.display_text(message, False), "Ahoj")
        self.assertEqual(display.display_text(message, True), "Hello\nAhoj")
        self.assertEqual(display.display_text("plain subtitle", True), "plain subtitle")

    def test_translator_model_and_formatting_options(self):
        translator = load_script("suntitle-translator.py")
        args = translator.parse_args([
            "--source-language", "uk",
            "--target-language", "cs",
            "--provider", "huggingface",
        ])
        self.assertEqual(translator.model_name(args), "Helsinki-NLP/opus-mt-uk-cs")
        self.assertEqual(translator.preserve_surrounding_whitespace("  text\n", "překlad"), "  překlad\n")


if __name__ == "__main__":
    unittest.main()
