import argparse
import json
import sys
from typing import Optional


def parse_args(argv: Optional[list[str]] = None):
    parser = argparse.ArgumentParser(description="Websocket subtitle display.")
    parser.add_argument("--host", default="localhost:3030", help="Websocket server authority")
    parser.add_argument("--input-stream", choices=["transcription", "translation"], default="translation")
    parser.add_argument("--max-lines", type=int, choices=range(1, 11), default=4)
    parser.add_argument("--font-scale", type=float, default=1.0)
    parser.add_argument("--show-original", action="store_true")
    parser.add_argument("--theme", choices=["light", "dark", "transparent"], default="dark")
    args = parser.parse_args(argv)
    if not 0.5 <= args.font_scale <= 4.0:
        parser.error("--font-scale must be between 0.5 and 4.0")
    return args


def websocket_url(args) -> str:
    """Select a broadcast stage. The display MUST only receive from this socket."""
    endpoint = "translate" if args.input_stream == "transcription" else "display"
    return f"ws://{args.host}/{endpoint}"


def display_text(message: str, show_original: bool) -> str:
    """JSON messages may contain `original` and `translated`; plain text is translated text."""
    try:
        value = json.loads(message)
    except (json.JSONDecodeError, TypeError):
        return message
    if not isinstance(value, dict) or "translated" not in value:
        return message
    translated = str(value["translated"])
    original = value.get("original")
    return f"{original}\n{translated}" if show_original and original is not None else translated


def main(argv: Optional[list[str]] = None):
    args = parse_args(argv)
    from PyQt5.QtCore import Qt, QThread, pyqtSignal
    from PyQt5.QtWidgets import QApplication, QFrame, QLabel, QMainWindow
    import time
    import websocket

    class WebSocketClientThread(QThread):
        message_received = pyqtSignal(str)

        def __init__(self, ws_url):
            super().__init__()
            self.ws_url = ws_url
            self.ws = None
            self.running = True

        def run(self):
            self.ws = websocket.WebSocketApp(
                self.ws_url,
                on_message=lambda _ws, message: self.message_received.emit(message),
                on_error=lambda _ws, error: self.message_received.emit(f"WebSocket error: {error}"),
            )
            while self.running:
                self.ws.run_forever()
                time.sleep(1)

        def stop(self):
            self.running = False
            if self.ws:
                self.ws.close()

    class SubtitleWindow(QMainWindow):
        def __init__(self):
            super().__init__(None, Qt.WindowStaysOnTopHint)
            self.setWindowFlags(self.windowFlags() | Qt.FramelessWindowHint)
            self.setAttribute(Qt.WA_TranslucentBackground)
            self.panel = QFrame(self)
            backgrounds = {"dark": "rgba(0,0,0,180)", "light": "rgba(255,255,255,220)", "transparent": "rgba(0,0,0,0)"}
            foreground = "black" if args.theme == "light" else "white"
            font_size = round(18 * args.font_scale)
            self.panel.setStyleSheet(f"background-color: {backgrounds[args.theme]};")
            self.panel.setFixedSize(700, max(60, args.max_lines * round(font_size * 1.5)))
            self.label = QLabel("Waiting for WebSocket messages...", self.panel)
            self.label.setWordWrap(True)
            self.label.setFixedSize(self.panel.size())
            self.label.setAlignment(Qt.AlignBottom | Qt.AlignLeft)
            self.label.setStyleSheet(f"color: {foreground}; font-size: {font_size}px;")
            self.old_pos = None
            self.ws_thread = WebSocketClientThread(websocket_url(args))
            self.ws_thread.message_received.connect(
                lambda message: self.label.setText(display_text(message, args.show_original))
            )
            self.ws_thread.start()

        def mousePressEvent(self, event):
            if event.button() == Qt.LeftButton:
                self.old_pos = event.globalPos()

        def mouseReleaseEvent(self, event):
            self.old_pos = None

        def mouseMoveEvent(self, event):
            if self.old_pos:
                delta = event.globalPos() - self.old_pos
                self.move(self.pos() + delta)
                self.old_pos = event.globalPos()

        def keyPressEvent(self, event):
            if event.key() == Qt.Key_Escape:
                self.close()
            else:
                super().keyPressEvent(event)

        def closeEvent(self, event):
            self.ws_thread.stop()
            self.ws_thread.wait()
            event.accept()

    app = QApplication(sys.argv)
    window = SubtitleWindow()
    window.show()
    app.exec_()


if __name__ == "__main__":
    main()
